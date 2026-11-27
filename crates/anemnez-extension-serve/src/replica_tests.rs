//! The replica face's rows (ergon:M/log-face-any-replica; D m/42395, A m/42402): a log face over a replicated partition
//! says where it read (`anemnez-read-at: <node>@<end>`) on Call, Watch and `Events/Events`; a cursor past the replica's
//! committed end is `replica-behind` as Unavailable by name, naming the node, inside the call's bound; and a descriptor
//! that doesn't declare SWARM over a replica is refused by name. Every wait is on a ManualClock.

use std::borrow::Cow;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anemnez_clock::ManualClock;
use futures_util::StreamExt as _;
use hydramq_events::wire::{read_reply, read_request, watch_request};
use hydramq_events::{Event, Face, PartitionEvents, Position, Read, Source};
use hydramq_message::{Message, Symbol, Value, encode_message_canonical};

use super::Reading;
use crate::log_events::{Authorize, FaceLog, Log, LogEvents};
use crate::pb;
use crate::pb::events_server::Events as _;

/// The real-time bound on a row's wait for a thread to get where it waits.
const PARK: Duration = Duration::from_secs(30);
const LOG: &str = "ergon.books";
const NODE: &str = "mb16";

/// A partition in memory: its messages by offset, from 0.
#[derive(Default)]
struct Memory(Mutex<Vec<Vec<u8>>>);

impl Source for Memory {
    fn read(&self, from: u64, max: usize) -> Result<Vec<(u64, Vec<u8>)>, String> {
        let m = self.0.lock().expect("messages");
        let from = usize::try_from(from).expect("small");
        Ok(m.iter().enumerate().skip(from).take(max).map(|(i, b)| (i as u64, b.clone())).collect())
    }

    fn start(&self) -> Result<u64, String> {
        Ok(0)
    }
}

fn fact(seq: u64) -> Vec<u8> {
    let body = encode_message_canonical(&Message {
        kind: Some(Cow::Borrowed("ergon.fact")),
        body: Some(Value::Struct(vec![(Symbol::from("seq"), Value::from(i64::try_from(seq).expect("small")))])),
        ..Message::default()
    })
    .expect("encodes");
    let event = Event {
        producer: "ergon".into(),
        subject: format!("item/{seq}"),
        kind: "ergon.fact".into(),
        seq,
        wall_ms: 1_000,
        body,
        ..Event::default()
    };
    hydramq_events::to_bytes(&event).expect("encodes")
}

/// A replica of `LOG` holding `n` facts, committed to `n`, its waits on `part`.
fn replica(n: u64, part: &ManualClock) -> Arc<PartitionEvents> {
    let memory = Arc::new(Memory::default());
    memory.0.lock().expect("messages").extend((0..n).map(fact));
    Arc::new(PartitionEvents::new(LOG, memory as Arc<dyn Source>, n, &part.clock()))
}

fn face_over(replica: &Arc<PartitionEvents>, manual: &ManualClock) -> LogEvents {
    let verifier = crate::principal_tests::verifier(manual, true);
    let log: Arc<dyn Log> = Arc::new(FaceLog::new(Arc::clone(replica) as Arc<dyn Face>));
    LogEvents::over(log, verifier, Authorize::allow_all(), manual.clock()).read_at(NODE)
}

fn at(offset: u64) -> Position {
    Position { log: LOG.into(), offset }
}

fn call(from: u64, within: Duration) -> tonic::Request<pb::CallRequest> {
    tonic::Request::new(pb::CallRequest { request: read_request(&at(from), 10, within).expect("a request") })
}

fn read_at_of(metadata: &tonic::metadata::MetadataMap) -> Option<String> {
    metadata.get(crate::READ_AT_METADATA).and_then(|v| v.to_str().ok()).map(str::to_owned)
}

/// D m/42395 (2): a Call a replica answers says where it read, `<node>@<end>`, the end being what this replica had
/// committed; Events/Events says it too, which H's events_page carries as read_at beside its cursor.
#[tokio::test(flavor = "multi_thread")]
async fn a_replicas_read_says_where_it_read() {
    let (manual, part) = (ManualClock::new(), ManualClock::new());
    let replica = replica(3, &part);
    let face = face_over(&replica, &manual);

    let answered = face.call(call(0, Duration::ZERO)).await.expect("served");
    assert_eq!(read_at_of(answered.metadata()).as_deref(), Some("mb16@3"));
    match read_reply(&answered.into_inner().reply).expect("a read reply") {
        Read::Events { events, .. } => assert_eq!(events.len(), 3),
        other => panic!("three facts: {other:?}"),
    }

    let request = pb::EventsRequest { after: format!("{LOG}:"), kinds: Vec::new() };
    let page = face.events(tonic::Request::new(request)).await.expect("served");
    assert_eq!(read_at_of(page.metadata()).as_deref(), Some("mb16@3"), "Events/Events says it for events_page");
    assert_eq!(page.into_inner().collect::<Vec<_>>().await.len(), 3);

    // The end said is the replica's as it moves.
    replica.advanced(5);
    let answered = face.call(call(3, Duration::ZERO)).await.expect("served");
    assert_eq!(read_at_of(answered.metadata()).as_deref(), Some("mb16@5"));
}

/// D m/42395 (2), A m/42402 (1): a cursor strictly past this replica's end came from a replica further on. After its
/// wait it is `replica-behind` as Unavailable, by name first, naming the node in its words and its header, so a router
/// tries the next replica; never an empty page read as the head. A cursor at the end is the head, as before.
#[tokio::test(flavor = "multi_thread")]
async fn a_cursor_past_a_replicas_end_is_replica_behind_by_name() {
    let (manual, part) = (ManualClock::new(), ManualClock::new());
    let face = face_over(&replica(3, &part), &manual);

    let behind = face.call(call(7, Duration::ZERO)).await.expect_err("behind");
    assert_eq!(behind.code(), tonic::Code::Unavailable);
    assert!(behind.message().starts_with("replica-behind: "), "by name first: {}", behind.message());
    assert_eq!(hydramq_events::replica_behind(behind.message()), Some(3), "the end reads back: {}", behind.message());
    assert!(behind.message().contains("read at mb16@3"), "names the node: {}", behind.message());
    assert_eq!(read_at_of(behind.metadata()).as_deref(), Some("mb16@3"));

    let request = pb::EventsRequest { after: format!("{LOG}:6"), kinds: Vec::new() };
    let behind = face.events(tonic::Request::new(request)).await.err().expect("behind");
    assert_eq!(behind.code(), tonic::Code::Unavailable);
    assert!(behind.message().starts_with("replica-behind: "), "{}", behind.message());

    let head = face.call(call(3, Duration::ZERO)).await.expect("the head is served");
    match read_reply(&head.into_inner().reply).expect("a read reply") {
        Read::Events { events, next } => assert!(events.is_empty() && next.offset == 3, "the head, empty"),
        other => panic!("the head: {other:?}"),
    }
}

/// A m/42402 (4): a watch past a replica's end, after its beat, ends `replica-behind` as Unavailable by name, never a
/// frame a router can't route on; its open says where it reads.
#[tokio::test(flavor = "multi_thread")]
async fn a_watch_past_a_replicas_end_ends_replica_behind() {
    let (manual, part) = (ManualClock::new(), ManualClock::new());
    let face = face_over(&replica(3, &part), &manual);
    let request = pb::WatchRequest { request: watch_request(&at(9), 10, Duration::from_secs(1)).expect("a request") };
    let opened = face.watch(tonic::Request::new(request)).await.expect("opened");
    assert_eq!(read_at_of(opened.metadata()).as_deref(), Some("mb16@3"));
    let mut stream = opened.into_inner();
    // The replica's wait is its beat, on its own clock: fired as it parks.
    let (ended, _) = part.run_until(stream.next(), PARK).await.expect("the watch ended within its beat");
    let ended = ended.expect("an end").expect_err("an end by name");
    assert_eq!(ended.code(), tonic::Code::Unavailable);
    assert!(ended.message().starts_with("replica-behind: "), "{}", ended.message());
    assert!(stream.next().await.is_none(), "and nothing after");
}

/// A product's own log, not replicated, says no end, so a face with a node says no header for it.
#[tokio::test(flavor = "multi_thread")]
async fn a_log_that_is_no_replica_says_no_read_at() {
    let (manual, log_clock) = (ManualClock::new(), ManualClock::new());
    let dir = tempfile::tempdir().expect("a temp dir");
    let events = Arc::new(
        hydramq_events::Events::open(&dir.path().join("log"), "ergon", LOG, Default::default(), &log_clock.clock())
            .expect("opens"),
    );
    let verifier = crate::principal_tests::verifier(&manual, true);
    let face = LogEvents::new(events, verifier, Authorize::allow_all(), manual.clock()).read_at(NODE);
    let answered = face.call(call(0, Duration::ZERO)).await.expect("served");
    assert_eq!(read_at_of(answered.metadata()), None);
    assert!(face.replicas().is_empty());
}

/// D m/42395 (1): a read's wait for its cursor is what the call's bound has left less a hop, never the request's own
/// constant, so replica-behind arrives inside the call.
#[test]
fn a_reads_wait_ends_a_hop_inside_its_bound() {
    let (manual, part) = (ManualClock::new(), ManualClock::new());
    let until = manual.now() + Duration::from_secs(10);
    let reading = Reading::new(replica(0, &part) as Arc<dyn Face>, Some((until, manual.clock())));
    let hop = hydramq_events::wire::HOP_MARGIN;
    assert_eq!(reading.bounded(Duration::from_secs(30)), Duration::from_secs(10) - hop);
    assert_eq!(reading.bounded(Duration::from_secs(1)), Duration::from_secs(1), "a shorter ask stands");
    let watched = Reading::new(replica(0, &part) as Arc<dyn Face>, None);
    assert_eq!(watched.bounded(Duration::from_secs(30)), Duration::from_secs(30), "a watch's beat is its own");
}

fn descriptor(scope: pb::CursorScope) -> crate::Describe {
    let service = pb::ServiceOptions {
        contract: "ergon".into(),
        version: "1.0.0".into(),
        cursor_scope: scope as i32,
        ..Default::default()
    };
    let options = crate::serve_tests::land_options(pb::Effect::Read, pb::Callers::Product);
    crate::Describe::new(crate::serve_tests::descriptor_with("ergon.v1", &options, Some(&service)))
}

fn board() -> crate::Board {
    crate::Board::new(crate::journal::Journal::new("ab12cd34", 4, ManualClock::new().clock()))
}

/// D m/42395: a replica's cursor resumes at any instance, so a face read from one under a descriptor declaring
/// INSTANCE is refused `replica-needs-swarm-scope` at build, naming the log; built anyway, every read is refused by it.
/// SWARM is served.
#[tokio::test(flavor = "multi_thread")]
async fn a_replica_face_declares_swarm() {
    let (manual, part) = (ManualClock::new(), ManualClock::new());
    let replica = replica(3, &part);

    let refused =
        crate::ApiFaces::try_new(descriptor(pb::CursorScope::Instance), board(), face_over(&replica, &manual))
            .err()
            .expect("refused at build");
    assert_eq!(refused.0, crate::REPLICA_NEEDS_SWARM_SCOPE);
    assert!(refused.1.starts_with("replica-needs-swarm-scope: the logs ergon.books"), "{}", refused.1);

    assert!(
        crate::ApiFaces::try_new(descriptor(pb::CursorScope::Swarm), board(), face_over(&replica, &manual)).is_ok(),
        "SWARM is served"
    );

    let mut events = crate::log_events::EventsFace::from(face_over(&replica, &manual));
    let why = crate::serve::replica_scope(&descriptor(pb::CursorScope::Instance), &events).expect_err("refused");
    let crate::log_events::EventsFace::Log(log) = &mut events else { panic!("a log face") };
    log.misdeclared(why);
    let refused = log.call(call(0, Duration::ZERO)).await.expect_err("served nothing");
    assert_eq!(refused.code(), tonic::Code::FailedPrecondition);
    assert!(refused.message().starts_with("replica-needs-swarm-scope: "), "{}", refused.message());
}
