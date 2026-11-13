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

    let mut watch =
        journal.subscribe(&snapshot.as_of, kinds(&[crate::health::KIND]), far(&manual)).expect("subscribes");
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
async fn the_refusals_reach_a_grpc_caller_by_name() {
    use pb::events_client::EventsClient;

    let (journal, manual) = journal(8);
    let service = crate::journal::EventsService::new(journal.clone(), crate::principal_tests::verifier(&manual, false));
    let (channel, _dir) = events_channel(service, true).await;
    let mut client = EventsClient::new(channel);
    // Each request carries its own token, minted over that request's own encoding.
    let mut nonce = 0u8;
    let mut request = |after: &str, life: u64| {
        let message = pb::EventsRequest { after: after.to_owned(), kinds: Vec::new() };
        let body = anemnez_extension::principal::grpc_body(&message.encode_to_vec());
        nonce += 1;
        let token = crate::principal_tests::mint(&manual, crate::journal::EVENTS_METHOD, body, life, nonce);
        let mut r = tonic::Request::new(message);
        r.metadata_mut().insert(anemnez_extension::principal::GRPC_METADATA_KEY, token.parse().expect("ascii"));
        r
    };
    let refused = client.events(request("e5f6a7b8.boot1:1", 60)).await.expect_err("refused");
    assert_eq!((refused.code(), refused.message()), (tonic::Code::FailedPrecondition, "cursor-not-mine"));

    let unsigned = client.events(pb::EventsRequest::default()).await.expect_err("refused");
    assert_eq!((unsigned.code(), unsigned.message()), (tonic::Code::InvalidArgument, "malformed"));

    // A token lifted onto another request (a different cursor) is call-mismatch.
    let mut lifted = request("", 60);
    *lifted.get_mut() = pb::EventsRequest { after: journal.head(), kinds: Vec::new() };
    let mismatch = client.events(lifted).await.expect_err("refused");
    assert_eq!((mismatch.code(), mismatch.message()), (tonic::Code::PermissionDenied, "call-mismatch"));

    let mut stream = client.events(request("", 60)).await.expect("opens").into_inner();
    let cursor = journal.publish("tick", vec![7]);
    let event = stream.message().await.expect("reads").expect("an event");
    assert_eq!((event.cursor, event.payload), (cursor, vec![7]));

    // The stream ends at the token's expiry, a deadline on the injected clock.
    assert!(manual.until_waiting(1, PARK).await, "the stream parks on its expiry");
    manual.advance(Duration::from_secs(60)).await;
    let ended = stream.message().await.expect_err("ended");
    assert_eq!((ended.code(), ended.message()), (tonic::Code::Unauthenticated, "expired"));
}

/// Serve `service` on a socket in a temp dir, with the request-digest layer or without it, and
/// answer a channel to it.
async fn events_channel(
    service: crate::journal::EventsService,
    layered: bool,
) -> (tonic::transport::Channel, tempfile::TempDir) {
    use pb::events_server::EventsServer;
    let dir = tempfile::tempdir().expect("a temp dir");
    let socket = dir.path().join("events.sock");
    let listener = tokio::net::UnixListener::bind(&socket).expect("binds");
    let incoming = tokio_stream::wrappers::UnixListenerStream::new(listener);
    if layered {
        let layer = crate::DigestLayer::for_paths([crate::journal::EVENTS_METHOD]);
        let server = tonic::transport::Server::builder().layer(layer).add_service(EventsServer::new(service));
        tokio::spawn(server.serve_with_incoming(incoming));
    } else {
        let server = tonic::transport::Server::builder().add_service(EventsServer::new(service));
        tokio::spawn(server.serve_with_incoming(incoming));
    }
    let path = socket.clone();
    let channel = tonic::transport::Endpoint::from_static("http://[::]:50051")
        .connect_with_connector(tower::service_fn(move |_: tonic::transport::Uri| {
            let path = path.clone();
            async move {
                Ok::<_, std::io::Error>(hyper_util::rt::TokioIo::new(tokio::net::UnixStream::connect(path).await?))
            }
        }))
        .await
        .expect("connects");
    (channel, dir)
}

/// A newer caller's `EventsRequest`: one field this build does not know.
#[derive(Clone, PartialEq, prost::Message)]
struct NewerEventsRequest {
    #[prost(string, tag = "1")]
    after: String,
    #[prost(string, repeated, tag = "2")]
    kinds: Vec<String>,
    #[prost(string, tag = "99")]
    priority: String,
}

/// The body digest is over the bytes as received (D): a request carrying a field this build does not
/// know still verifies, so a newer caller works against an older product during a rollout.
#[tokio::test(flavor = "multi_thread")]
async fn a_request_with_a_field_this_build_does_not_know_still_verifies() {
    let (journal, manual) = journal(8);
    let service = crate::journal::EventsService::new(journal.clone(), crate::principal_tests::verifier(&manual, false));
    let (channel, _dir) = events_channel(service, true).await;

    let message = NewerEventsRequest { after: String::new(), kinds: Vec::new(), priority: "high".into() };
    let body = anemnez_extension::principal::grpc_body(&message.encode_to_vec());
    let token = crate::principal_tests::mint(&manual, crate::journal::EVENTS_METHOD, body, 60, 1);
    let mut request = tonic::Request::new(message);
    request.metadata_mut().insert(anemnez_extension::principal::GRPC_METADATA_KEY, token.parse().expect("ascii"));

    let mut grpc = tonic::client::Grpc::new(channel);
    grpc.ready().await.expect("ready");
    let codec = tonic::codec::ProstCodec::<NewerEventsRequest, pb::Event>::default();
    let path = http::uri::PathAndQuery::from_static(crate::journal::EVENTS_METHOD);
    let mut stream = grpc.server_streaming(request, path, codec).await.expect("admitted").into_inner();
    let cursor = journal.publish("tick", vec![1]);
    let event = stream.message().await.expect("reads").expect("an event");
    assert_eq!(event.cursor, cursor);
}

/// Without the digest layer there is nothing to bind a token to, so the call is refused by name,
/// never verified against a re-encoding.
#[tokio::test(flavor = "multi_thread")]
async fn a_token_checked_method_served_without_the_layer_is_refused() {
    let (journal, manual) = journal(8);
    let service = crate::journal::EventsService::new(journal.clone(), crate::principal_tests::verifier(&manual, false));
    let (channel, _dir) = events_channel(service, false).await;
    let message = pb::EventsRequest::default();
    let body = anemnez_extension::principal::grpc_body(&message.encode_to_vec());
    let token = crate::principal_tests::mint(&manual, crate::journal::EVENTS_METHOD, body, 60, 1);
    let mut request = tonic::Request::new(message);
    request.metadata_mut().insert(anemnez_extension::principal::GRPC_METADATA_KEY, token.parse().expect("ascii"));
    let refused = pb::events_client::EventsClient::new(channel).events(request).await.expect_err("refused");
    assert_eq!((refused.code(), refused.message()), (tonic::Code::InvalidArgument, "malformed"));
}

/// One uncompressed frame is digested over its message bytes; anything else is not.
#[test]
fn only_one_uncompressed_frame_is_digested() {
    use sha2::Digest as _;
    let frame = |flag: u8, message: &[u8]| {
        let mut out = vec![flag];
        out.extend_from_slice(&u32::try_from(message.len()).expect("small").to_be_bytes());
        out.extend_from_slice(message);
        out
    };
    let want: [u8; 32] = sha2::Sha256::digest(b"hello").into();
    assert_eq!(crate::digest::digest_frame(&frame(0, b"hello")), Some(crate::RequestDigest(want)));
    assert_eq!(crate::digest::digest_frame(&frame(1, b"hello")), None, "compressed");
    let mut two = frame(0, b"a");
    two.extend(frame(0, b"b"));
    assert_eq!(crate::digest::digest_frame(&two), None, "two frames");
    assert_eq!(crate::digest::digest_frame(&[0, 0, 0]), None, "short");
    assert_eq!(crate::digest::digest_frame(&[]), None, "empty");
}

/// The kernel's hand-written options decoder reads exactly what the proto's generated code writes:
/// options encoded with `pb::MethodOptions` (from options.proto) parse to the same values.
#[test]
fn the_descriptor_check_reads_the_protos_own_encoding() {
    fn ld(field: u32, bytes: &[u8]) -> Vec<u8> {
        let mut out = Vec::new();
        prost::encoding::encode_key(field, prost::encoding::WireType::LengthDelimited, &mut out);
        prost::encoding::encode_varint(bytes.len() as u64, &mut out);
        out.extend_from_slice(bytes);
        out
    }
    let options = pb::MethodOptions {
        description: "Land a train.".into(),
        result: Some(pb::Result { shape: Some(pb::result::Shape::Bound(50)) }),
        effect: pb::Effect::Write as i32,
        route: Some(pb::Route { to: Some(pb::route::To::Leader("herald".into())) }),
        person_act: true,
        presence: pb::Presence::Minimal as i32,
        permission: pb::Permission::Ask as i32,
        callers: pb::Callers::Product as i32,
        session: pb::Session::Opens as i32,
        blocks: true,
        runs_to_completion: true,
        examples: vec![pb::Example { args_json: r#"{"train": "train/h-x"}"#.into(), caption: "Land one".into() }],
    };
    let mut method = prost_types::MethodDescriptorProto {
        name: Some("Land".into()),
        input_type: Some(".h.v1.Req".into()),
        output_type: Some(".h.v1.Req".into()),
        ..Default::default()
    }
    .encode_to_vec();
    method.extend(ld(4, &ld(51701, &options.encode_to_vec())));
    let mut service =
        prost_types::ServiceDescriptorProto { name: Some("H".into()), ..Default::default() }.encode_to_vec();
    service.extend(ld(2, &method));
    let mut file = prost_types::FileDescriptorProto {
        package: Some("h.v1".into()),
        message_type: vec![prost_types::DescriptorProto {
            name: Some("Req".into()),
            field: vec![prost_types::FieldDescriptorProto {
                name: Some("train".into()),
                number: Some(1),
                r#type: Some(prost_types::field_descriptor_proto::Type::String as i32),
                label: Some(prost_types::field_descriptor_proto::Label::Optional as i32),
                ..Default::default()
            }],
            ..Default::default()
        }],
        ..Default::default()
    }
    .encode_to_vec();
    file.extend(ld(6, &service));

    let methods = anemnez_extension::descriptor::check_descriptor(&ld(1, &file)).expect("admitted");
    use anemnez_extension::options::*;
    assert_eq!(methods.len(), 1);
    assert_eq!(methods[0].path, "/h.v1.H/Land");
    let mut want = MethodOptions::new(
        "Land a train.",
        ResultShape::Bound(50),
        Effect::Write,
        Presence::Minimal,
        Permission::Ask,
        Callers::Product,
    );
    want.route = Route::Leader("herald".into());
    want.person_act = true;
    want.session = Some(Session::Opens);
    want.blocks = true;
    want.runs_to_completion = true;
    want.examples = vec![Example::new(r#"{"train": "train/h-x"}"#, "Land one")];
    assert_eq!(methods[0].options, want);
}
