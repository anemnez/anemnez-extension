//! The serving face's claims, each on a `ManualClock` so no test waits time out for real.
//! Claim names are substrate `docs/plans/ASYNCIFICATION.md`'s (`async-…`) and CONTRACT.md's.

use std::time::Duration;

use anemnez_clock::{ManualClock, Mono};
use prost::Message;

use crate::journal::{Journal, Refusal};
use crate::{Board, Describe, pb};
use anemnez_extension::health::State;

const ORIGIN: &str = "a1b2c3d4.boot1";
/// The real-time bound on waiting for a spawned reader to park on the clock.
const PARK: Duration = Duration::from_secs(30);

fn journal(kept: usize) -> (Journal, ManualClock) {
    let manual = ManualClock::new();
    (Journal::new(ORIGIN, kept, manual.clock()), manual)
}

fn far(manual: &ManualClock) -> Mono {
    manual.now() + Duration::from_secs(3600)
}

fn kinds(list: &[&str]) -> Vec<String> {
    list.iter().map(|k| (*k).to_owned()).collect()
}

/// async-stream-never-awaits-a-subscriber: publishing is a plain call that a subscriber which never
/// reads cannot hold up; once it has fallen further behind than the journal keeps, its stream ends
/// with data-loss, never silently.
#[tokio::test]
async fn a_publisher_never_waits_on_a_subscriber_that_never_reads() {
    let (journal, manual) = journal(4);
    let mut idle = journal.subscribe("", Vec::new(), far(&manual)).expect("subscribes");
    for i in 0..100u8 {
        journal.publish("tick", vec![i]);
    }
    assert_eq!(idle.next().await, Some(Err(Refusal::DataLoss)));
    assert_eq!(idle.next().await, None, "the stream ends after data-loss");

    // A subscriber that keeps up loses nothing.
    let mut live = journal.subscribe(&journal.head(), Vec::new(), far(&manual)).expect("subscribes");
    let cursor = journal.publish("tick", vec![100]);
    let event = live.next().await.expect("an event").expect("not refused");
    assert_eq!((event.cursor, event.payload), (cursor, vec![100]));
}

/// CONTRACT.md: a stream ends with `expired` at its principal token's expiry, a deadline on the
/// injected clock, and not a moment before.
#[tokio::test]
async fn a_stream_ends_with_expired_at_its_tokens_expiry_on_the_injected_clock() {
    let (journal, manual) = journal(8);
    let expires = manual.now() + Duration::from_secs(60);
    let mut sub = journal.subscribe("", Vec::new(), expires).expect("subscribes");
    let reader = tokio::spawn(async move { sub.next().await });
    assert!(manual.until_waiting(1, PARK).await, "the reader parks on its expiry");

    manual.advance(Duration::from_secs(59)).await;
    manual.settle().await;
    assert!(!reader.is_finished(), "a second before its expiry the stream is open");

    manual.advance(Duration::from_secs(1)).await;
    assert_eq!(reader.await.expect("joins"), Some(Err(Refusal::Expired)));

    // A stream opened past its expiry ends at once.
    let mut late = journal.subscribe("", Vec::new(), manual.now()).expect("subscribes");
    assert_eq!(late.next().await, Some(Err(Refusal::Expired)));
}

/// async-cancelled-only-by-its-owner: dropping a subscriber mid-wait (a disconnected client's
/// handler future) cancels nothing else: the journal keeps publishing and every other subscriber
/// keeps reading.
#[tokio::test]
async fn a_dropped_subscriber_cancels_nothing() {
    let (journal, manual) = journal(8);
    let mut kept = journal.subscribe("", Vec::new(), far(&manual)).expect("subscribes");
    let mut gone = journal.subscribe("", Vec::new(), far(&manual)).expect("subscribes");
    let staying = tokio::spawn(async move { kept.next().await });
    let leaving = tokio::spawn(async move { gone.next().await });
    assert!(manual.until_waiting(2, PARK).await, "both readers park");

    leaving.abort();
    assert!(leaving.await.expect_err("aborted").is_cancelled());

    let cursor = journal.publish("tick", vec![1]);
    let event = staying.await.expect("joins").expect("an event").expect("not refused");
    assert_eq!(event.cursor, cursor);
    assert_eq!(journal.head(), cursor);
}

/// CONTRACT.md: a stream resumes from the cursor a caller last saw, filtered to the kinds it asks for.
#[tokio::test]
async fn a_cursor_resumes_after_itself_filtered_by_kind() {
    let (journal, manual) = journal(16);
    let first = journal.publish("health", vec![1]);
    journal.publish("tick", vec![2]);
    let third = journal.publish("health", vec![3]);
    journal.publish("tick", vec![4]);

    let mut sub = journal.subscribe(&first, kinds(&["health"]), far(&manual)).expect("subscribes");
    let event = sub.next().await.expect("an event").expect("not refused");
    assert_eq!((event.cursor, event.kind.as_str(), event.payload), (third, "health", vec![3]));

    let mut every = journal.subscribe(&first, Vec::new(), far(&manual)).expect("subscribes");
    let next = every.next().await.expect("an event").expect("not refused");
    assert_eq!(next.payload, vec![2], "no kinds asked is every kind");
}

/// CONTRACT.md: an instance-scoped cursor from another instance, or from this one before a
/// restart, is cursor-not-mine; one older than what is kept is out-of-range.
#[tokio::test]
async fn a_foreign_cursor_is_not_mine_and_an_old_one_is_out_of_range() {
    let (journal, manual) = journal(2);
    let oldest = journal.publish("tick", vec![1]);
    journal.publish("tick", vec![2]);
    journal.publish("tick", vec![3]);
    journal.publish("tick", vec![4]);

    let refused = |after: &str| journal.subscribe(after, Vec::new(), far(&manual)).err();
    assert_eq!(refused("e5f6a7b8.boot1:2"), Some(Refusal::CursorNotMine), "another instance");
    assert_eq!(refused("a1b2c3d4.boot0:2"), Some(Refusal::CursorNotMine), "an earlier boot");
    assert_eq!(refused("a1b2c3d4.boot1:99"), Some(Refusal::CursorNotMine), "a sequence never issued");
    assert_eq!(refused("garbage"), Some(Refusal::CursorNotMine), "not a cursor of ours");
    assert_eq!(refused(&oldest), Some(Refusal::OutOfRange), "older than the two kept");
    assert_eq!(refused(&journal.head()), None);
}

/// health-is-state-with-parts: a watcher reads Get, then subscribes after its `as_of`, and sees
/// every change since and none from before.
#[tokio::test]
async fn a_watcher_reads_health_then_sees_only_later_changes() {
    let (journal, manual) = journal(16);
    let board = Board::new(journal.clone());
    board.set(State::Serving, "");
    board.set_part("memory/index", State::Failing, "rebuilding");

    let snapshot = board.get();
    assert_eq!(snapshot.state, pb::State::Serving as i32);
    assert_eq!(snapshot.parts.len(), 1);
    assert_eq!(snapshot.parts[0].state, pb::State::Failing as i32);
    journal.publish("tick", vec![]);

    let mut watch = journal.subscribe(&snapshot.as_of, kinds(&[crate::health::KIND]), far(&manual)).expect("subscribes");
    manual.advance(Duration::from_secs(5)).await;
    board.set_part("memory/index", State::Serving, "");
    let event = watch.next().await.expect("an event").expect("not refused");
    let change = pb::HealthReply::decode(event.payload.as_slice()).expect("a health reply");
    assert_eq!(change.parts[0].state, pb::State::Serving as i32);
    assert_eq!(
        change.parts[0].since - snapshot.parts[0].since,
        5_000,
        "since moves with the state, on the injected clock"
    );
    assert_eq!(board.get().as_of, event.cursor);
}

/// a-product-describes-itself-from-its-idl: Describe answers the descriptor and its SHA-256.
#[tokio::test]
async fn describe_answers_the_descriptor_and_its_sha256() {
    use pb::describe_server::Describe as _;
    let describe = Describe::new(b"abc".to_vec());
    let reply = describe.get(tonic::Request::new(pb::DescribeRequest {})).await.expect("answers").into_inner();
    assert_eq!(reply.file_descriptor_set, b"abc");
    assert_eq!(reply.digest, "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad");
}

/// The refusals reach a gRPC caller by name, over a real socket: the status message IS the
/// refusal's name, and a stream with a token carries events.
#[tokio::test(flavor = "multi_thread")]
#[allow(clippy::result_large_err)] // the expiry hook answers tonic's `Status`
async fn the_refusals_reach_a_grpc_caller_by_name() {
    use pb::events_client::EventsClient;
    use pb::events_server::EventsServer;

    let (journal, manual) = journal(8);
    let deadline = far(&manual);
    let service = crate::journal::EventsService::new(journal.clone(), move |meta: &tonic::metadata::MetadataMap| {
        if meta.get(anemnez_extension::principal::GRPC_METADATA_KEY).is_some() {
            Ok(deadline)
        } else {
            Err(tonic::Status::unauthenticated("no principal"))
        }
    });
    let dir = tempfile::tempdir().expect("a temp dir");
    let socket = dir.path().join("events.sock");
    let listener = tokio::net::UnixListener::bind(&socket).expect("binds");
    let incoming = tokio_stream::wrappers::UnixListenerStream::new(listener);
    tokio::spawn(tonic::transport::Server::builder().add_service(EventsServer::new(service)).serve_with_incoming(incoming));

    let path = socket.clone();
    let channel = tonic::transport::Endpoint::from_static("http://[::]:50051")
        .connect_with_connector(tower::service_fn(move |_: tonic::transport::Uri| {
            let path = path.clone();
            async move { Ok::<_, std::io::Error>(hyper_util::rt::TokioIo::new(tokio::net::UnixStream::connect(path).await?)) }
        }))
        .await
        .expect("connects");
    let mut client = EventsClient::new(channel);
    let request = |after: &str| {
        let mut r = tonic::Request::new(pb::EventsRequest { after: after.to_owned(), kinds: Vec::new() });
        r.metadata_mut().insert(anemnez_extension::principal::GRPC_METADATA_KEY, "token".parse().expect("ascii"));
        r
    };

    let refused = client.events(request("e5f6a7b8.boot1:1")).await.expect_err("refused");
    assert_eq!((refused.code(), refused.message()), (tonic::Code::FailedPrecondition, "cursor-not-mine"));

    let unsigned = client.events(pb::EventsRequest::default()).await.expect_err("refused");
    assert_eq!(unsigned.code(), tonic::Code::Unauthenticated);

    let mut stream = client.events(request("")).await.expect("opens").into_inner();
    let cursor = journal.publish("tick", vec![7]);
    let event = stream.message().await.expect("reads").expect("an event");
    assert_eq!((event.cursor, event.payload), (cursor, vec![7]));
}
