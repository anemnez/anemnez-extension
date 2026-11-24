//! The many-log face's rows (L m/32091, D m/32100).

use std::sync::Arc;
use std::time::Duration;

use anemnez_clock::ManualClock;
use hydramq_events::wire::{latest_reply, latest_request, read_reply, read_request};
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
    let faces: std::collections::BTreeMap<String, Arc<dyn Face>> = [
        ("owed".to_owned(), log(dir.path(), "owed", 2) as Arc<dyn Face>),
        ("forecast".to_owned(), log(dir.path(), "forecast", 3) as Arc<dyn Face>),
    ]
    .into();
    let many = ManyLogs::new(faces);
    let serve = |log: &str| hydramq_events::wire::serve(&many, &read_request(&at(log), 10, Duration::ZERO).unwrap());
    assert_eq!(count(&serve("owed")), Ok(2), "owed's own two");
    assert_eq!(count(&serve("forecast")), Ok(3), "forecast's own three");
    let unknown = count(&serve("ship")).expect_err("no such log here");
    assert!(unknown.contains("ship") && unknown.contains("forecast, owed"), "named, with the logs served: {unknown}");
}

/// D m/32100: Latest is never routed by a prefix's spelling. Until a request names its log, it's refused by name on a
/// many-log face, saying how to read instead.
#[test]
fn latest_on_a_many_log_face_is_refused_by_name() {
    let dir = tempfile::tempdir().unwrap();
    let faces: std::collections::BTreeMap<String, Arc<dyn Face>> =
        [("owed".to_owned(), log(dir.path(), "owed", 1) as Arc<dyn Face>)].into();
    let many = ManyLogs::new(faces);
    let reply = hydramq_events::wire::serve(&many, &latest_request("owed/").unwrap());
    let refused = latest_reply(&reply).expect_err("refused").to_string();
    assert!(refused.contains("latest-needs-its-log"), "{refused}");
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
