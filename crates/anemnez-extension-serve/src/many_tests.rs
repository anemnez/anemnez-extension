//! The many-log face's rows (L m/32091, D m/32100).

use std::sync::Arc;
use std::time::Duration;

use anemnez_clock::ManualClock;
use hydramq_events::wire::{latest_reply, read_reply, read_request};
use hydramq_events::{Events, Face, Position, Read};

use super::ManyLogs;
use crate::log_events::{Authorize, Log, LogEvents};
use crate::pb;
use crate::pb::events_server::Events as _;

/// A held log named `log`, with `n` events published.
fn log(dir: &std::path::Path, log: &str, n: usize) -> Arc<Events> {
    let clock = ManualClock::new();
    let events = Events::open(&dir.join(log), "horaid", log, hydramq_events::Settings::default(), &clock.clock())
        .expect("the log opens");
    std::mem::forget(clock);
    for i in 0..n {
        let body = hydramq_events::body("horai.fact", &[]).expect("encodes");
        events.publish_next(&format!("{log}/{i}"), "fact", &body, 1).expect("published");
    }
    Arc::new(events)
}

/// A many-log face over owed (two events) and forecast (three), Horai's two streams.
fn owed_and_forecast(dir: &std::path::Path) -> ManyLogs {
    let faces: std::collections::BTreeMap<String, Arc<dyn Face>> = [
        ("owed".to_owned(), log(dir, "owed", 2) as Arc<dyn Face>),
        ("forecast".to_owned(), log(dir, "forecast", 3) as Arc<dyn Face>),
    ]
    .into();
    ManyLogs::new(faces)
}

fn at(log: &str) -> Position {
    Position { log: log.into(), offset: 0 }
}

fn count(reply: &[u8]) -> Result<usize, String> {
    match read_reply(reply) {
        Ok(Read::Events { events, .. }) => Ok(events.len()),
        Ok(other) => Err(format!("{other:?}")),
        Err(e) => Err(e.to_string()),
    }
}

/// A read goes to the log its position names and no other, so one log's events never answer for another's; a log the
/// face doesn't serve is refused, naming every log it does, never read from another.
#[test]
fn a_read_goes_to_the_log_its_position_names() {
    let dir = tempfile::tempdir().unwrap();
    let many = owed_and_forecast(dir.path());
    let serve = |log: &str| hydramq_events::wire::serve(&many, &read_request(&at(log), 10, Duration::ZERO).unwrap());
    assert_eq!(count(&serve("owed")), Ok(2), "owed's own two");
    assert_eq!(count(&serve("forecast")), Ok(3), "forecast's own three");
    let unknown = count(&serve("ship")).expect_err("no such log here");
    assert!(unknown.contains("ship") && unknown.contains("forecast, owed"), "named, with the logs served: {unknown}");
}

/// D m/32100, K m/33557 item 4, L m/34051: Latest on a many-log face is answered from the log its request names, that
/// log's own subjects only (owed's latest never forecast's), so a follower starts with latest_all_in("owed", "owed/").
/// One naming no log is refused by name, never routed by a prefix's spelling, and one naming a log the face doesn't
/// serve is ForeignLog, naming the logs it does.
#[test]
fn latest_on_a_many_log_face_reads_the_log_it_names_and_never_routes_by_spelling() {
    use hydramq_events::wire::{latest_reply_in, latest_request_in};
    let dir = tempfile::tempdir().unwrap();
    let many = owed_and_forecast(dir.path());
    let latest = |log: Option<&str>, prefix: &str| {
        hydramq_events::wire::serve(&many, &latest_request_in(log, prefix).expect("encodes"))
    };
    let (owed, as_of) = latest_reply_in("owed", &latest(Some("owed"), "owed/")).expect("owed's latest");
    assert_eq!(owed.iter().map(|e| e.subject.as_str()).collect::<Vec<_>>(), ["owed/0", "owed/1"]);
    assert_eq!(as_of.log, "owed", "as of owed's own position");
    let (forecast, _) = latest_reply_in("forecast", &latest(Some("forecast"), "")).expect("forecast's latest");
    assert_eq!(forecast.len(), 3, "forecast's own three, every subject");
    let (none, _) = latest_reply_in("forecast", &latest(Some("forecast"), "owed/")).expect("answered");
    assert!(none.is_empty(), "owed/ in forecast is forecast's, empty: never another log's");
    let unnamed = latest_reply(&latest(None, "owed/")).expect_err("refused").to_string();
    assert!(unnamed.contains("latest-needs-its-log") && unnamed.contains("latest_all_in"), "{unnamed}");
    let foreign = latest_reply(&latest(Some("ship"), "ship/")).expect_err("refused").to_string();
    assert!(foreign.contains("ship") && foreign.contains("forecast, owed"), "{foreign}");
}

/// over_many takes one or more logs, each by a name, never two of one name; and a Call through the face reads the log
/// its request names.
#[tokio::test]
async fn over_many_names_each_log_once_and_a_call_reads_the_named_one() {
    let dir = tempfile::tempdir().unwrap();
    let manual = ManualClock::new();
    let verifier = || crate::principal_tests::verifier(&manual, true);
    let owed: Arc<dyn Log> = Arc::new(log(dir.path(), "owed", 2));
    let forecast: Arc<dyn Log> = Arc::new(log(dir.path(), "forecast", 1));
    let twice = vec![("owed".to_owned(), Arc::clone(&owed)), ("owed".to_owned(), Arc::clone(&forecast))];
    let refused = LogEvents::over_many(twice, verifier(), Authorize::allow_all(), manual.clock()).err().expect("twice");
    assert!(refused.starts_with("log-names-twice"), "{refused}");
    let unnamed = vec![(String::new(), Arc::clone(&owed))];
    let refused =
        LogEvents::over_many(unnamed, verifier(), Authorize::allow_all(), manual.clock()).err().expect("unnamed");
    assert!(refused.starts_with("log-unnamed"), "{refused}");

    let logs = vec![("owed".to_owned(), owed), ("forecast".to_owned(), forecast)];
    let face = LogEvents::over_many(logs, verifier(), Authorize::allow_all(), manual.clock()).expect("two logs");
    let call = |log: &str| {
        tonic::Request::new(pb::CallRequest { request: read_request(&at(log), 10, Duration::ZERO).unwrap() })
    };
    let owed = face.call(call("owed")).await.expect("served").into_inner();
    let forecast = face.call(call("forecast")).await.expect("served").into_inner();
    assert_eq!((count(&owed.reply), count(&forecast.reply)), (Ok(2), Ok(1)), "each its own log's");
}

/// ERGON-HA step 5 (O): a face that is no `Events` (a replicated partition's `PartitionEvents`) is served by name
/// beside a held log through [`crate::FaceLog`], read as its face and never opened per read: a Call and a Latest naming
/// it read its own events, and the held log beside it still reads its own.
#[tokio::test]
async fn a_face_log_is_served_beside_a_held_log_and_read_as_its_face() {
    use hydramq_events::wire::{latest_reply_in, latest_request_in};
    let dir = tempfile::tempdir().unwrap();
    let manual = ManualClock::new();
    let verifier = crate::principal_tests::verifier(&manual, true);
    let items: Arc<dyn Log> = Arc::new(log(dir.path(), "items", 2));
    let books: Arc<dyn Log> = Arc::new(crate::FaceLog::new(log(dir.path(), "books", 3) as Arc<dyn Face>));
    assert!(books.held().is_none(), "a face log holds no Events, so only its face reads it");
    let logs = vec![("items".to_owned(), items), ("books".to_owned(), books)];
    let face = LogEvents::over_many(logs, verifier, Authorize::allow_all(), manual.clock()).expect("two logs");
    let call = |request: Vec<u8>| tonic::Request::new(pb::CallRequest { request });
    let read = |log: &str| call(read_request(&at(log), 10, Duration::ZERO).unwrap());
    let books = face.call(read("books")).await.expect("served").into_inner();
    let items = face.call(read("items")).await.expect("served").into_inner();
    assert_eq!((count(&books.reply), count(&items.reply)), (Ok(3), Ok(2)), "each its own log's");
    let latest = face.call(call(latest_request_in(Some("books"), "books/").unwrap())).await.expect("served");
    let (latest, as_of) = latest_reply_in("books", &latest.into_inner().reply).expect("books' latest");
    assert_eq!((latest.len(), as_of.log.as_str()), (3, "books"), "the face's own latest, as of its own position");
}

/// E m/40767, L m/40805: a Watch on a face log streams from its face as a held log's does, so ERGON-HA's watchers of
/// the books' partition read it through over_many: an event published after the watch opens is delivered in its bound.
#[tokio::test(flavor = "multi_thread")]
async fn a_watch_on_a_face_log_streams_from_its_face() {
    use futures_util::StreamExt as _;
    use hydramq_events::wire::watch_request;
    let dir = tempfile::tempdir().unwrap();
    let manual = ManualClock::new();
    let verifier = crate::principal_tests::verifier(&manual, true);
    let books = log(dir.path(), "books", 0);
    let logs = vec![
        ("items".to_owned(), Arc::new(log(dir.path(), "items", 1)) as Arc<dyn Log>),
        ("books".to_owned(), Arc::new(crate::FaceLog::new(Arc::clone(&books) as Arc<dyn Face>)) as Arc<dyn Log>),
    ];
    let face = LogEvents::over_many(logs, verifier, Authorize::allow_all(), manual.clock()).expect("two logs");
    let request = watch_request(&at("books"), 10, Duration::from_secs(10)).expect("a request");
    let mut stream = face.watch(tonic::Request::new(pb::WatchRequest { request })).await.expect("opens").into_inner();
    let body = hydramq_events::body("ergon.moved", &[]).expect("encodes");
    books.publish_next("book/O", "moved", &body, 1).expect("published");
    let bound = anemnez_clock::Clock::system().timeout(Duration::from_secs(10), stream.next());
    let frame = bound.await.expect("delivered within its bound").expect("a frame").expect("not refused");
    match read_reply(&frame.frame).expect("an events frame") {
        Read::Events { events, .. } => assert_eq!(events.len(), 1, "the face's own event"),
        other => panic!("not an events frame: {other:?}"),
    }
}
