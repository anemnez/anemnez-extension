//! The log face's rows: a product's own hydramq-events log through `Events/Call` and `Events/Watch`, every wait
//! on an injected `ManualClock` (the face's) and the log on a clock of its own that is never advanced, so a
//! watch never beats and no row waits product time.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use anemnez_clock::ManualClock;
use futures_util::StreamExt as _;
use hydramq_events::wire::{read_reply, read_request, watch_request};
use hydramq_events::{Events, Position, Read};
use tokio::sync::Notify;

use crate::journal::StreamEnd;
use crate::log_events::{Authorize, LogEvents, Method};
use crate::pb;
use crate::pb::events_server::Events as _;

/// The real-time bound on a thread or task getting where a row waits for it.
const PARK: Duration = Duration::from_secs(30);
const LOG: &str = "audit";

struct Fixture {
    _dir: tempfile::TempDir,
    log: Arc<Events>,
    manual: ManualClock,
}

fn fixture() -> Fixture {
    let dir = tempfile::tempdir().expect("a temp dir");
    let log_clock = ManualClock::new();
    let log = Arc::new(
        Events::open(&dir.path().join("log"), "miniond", LOG, hydramq_events::Settings::default(), &log_clock.clock())
            .expect("the log opens"),
    );
    // The log's clock lives as long as the log reads it; it is never advanced.
    std::mem::forget(log_clock);
    Fixture { _dir: dir, log, manual: ManualClock::new() }
}

impl Fixture {
    fn face(&self, authorize: Authorize) -> LogEvents {
        let verifier = crate::principal_tests::verifier(&self.manual, true);
        LogEvents::new(Arc::clone(&self.log), verifier, authorize, self.manual.clock())
    }

    fn publish(&self, n: usize) {
        let body = hydramq_events::body("minion.decided", &[]).expect("encodes");
        let published = self.log.publish_next(&format!("call/{n}"), "decided", &body, 1);
        assert!(published.is_ok(), "the writer's append returned: {published:?}");
    }
}

fn from_start() -> Position {
    Position { log: LOG.into(), offset: 0 }
}

fn watch(max: usize) -> tonic::Request<pb::WatchRequest> {
    tonic::Request::new(pb::WatchRequest {
        request: watch_request(&from_start(), max, Duration::from_secs(10)).expect("a request"),
    })
}

fn events_in(frame: &pb::WatchFrame) -> usize {
    match read_reply(&frame.frame).expect("an events frame") {
        Read::Events { events, .. } => events.len(),
        other => panic!("not an events frame: {other:?}"),
    }
}

async fn until_no_watches(face: &LogEvents) {
    let ended = face.watch_ended();
    let waited = anemnez_clock::Clock::system()
        .timeout(PARK, async {
            loop {
                let notified = ended.notified();
                if face.watches_open() == 0 {
                    return;
                }
                notified.await;
            }
        })
        .await;
    assert!(waited.is_ok(), "every watch thread ended");
}

/// a-products-events-are-read-through-one-call: an authorized call reads the product's own log; one the
/// product refuses is refused `unauthorized` by name, and nothing of the log is served.
#[tokio::test(flavor = "multi_thread")]
async fn a_call_reads_the_log_only_once_authorized() {
    let f = fixture();
    f.publish(1);
    let read =
        || tonic::Request::new(pb::CallRequest { request: read_request(&from_start(), 10, Duration::ZERO).unwrap() });

    let open = f.face(Authorize::allow_all());
    let reply = open.call(read()).await.expect("served").into_inner();
    match read_reply(&reply.reply).expect("a read reply") {
        Read::Events { events, .. } => assert_eq!(events.len(), 1),
        other => panic!("one event: {other:?}"),
    }

    let asked = Arc::new(std::sync::Mutex::new(Vec::new()));
    let seen = Arc::clone(&asked);
    let closed = f.face(Authorize::new(move |a| {
        seen.lock().unwrap().push(a.method);
        async { Err("no minion.audit grant".to_owned()) }
    }));
    let refused = closed.call(read()).await.expect_err("refused");
    assert_eq!(refused.code(), tonic::Code::PermissionDenied);
    assert_eq!(refused.message(), "unauthorized: no minion.audit grant");
    let refused = closed.watch(watch(10)).await.err().expect("refused");
    assert_eq!(refused.message(), "unauthorized: no minion.audit grant");
    assert_eq!(closed.watches_open(), 0, "a refused watch starts no thread");
    assert_eq!(*asked.lock().unwrap(), vec![Method::Call, Method::Watch], "the product decided each method");

    // The Journal's stream isn't a log face's.
    let unserved = open.events(tonic::Request::new(pb::EventsRequest::default())).await.err().expect("refused");
    assert!(unserved.message().starts_with("events-call-unserved"), "{}", unserved.message());
}

/// A product's authorization that never answers is refused `unauthorized` at its bound, on the injected
/// clock, and never hangs the call.
#[tokio::test(flavor = "multi_thread")]
async fn an_unanswered_authorization_is_refused_at_its_bound() {
    let f = fixture();
    let face =
        Arc::new(f.face(Authorize::new(|_| std::future::pending())).with_authorize_within(Duration::from_secs(5)));
    let calling = Arc::clone(&face);
    let call = tokio::spawn(async move {
        calling
            .call(tonic::Request::new(pb::CallRequest {
                request: read_request(&from_start(), 1, Duration::ZERO).unwrap(),
            }))
            .await
    });
    assert!(f.manual.until_waiting(1, PARK).await, "the call waits on its bound");
    f.manual.advance(Duration::from_secs(5)).await;
    let refused = call.await.expect("joins").expect_err("refused");
    assert_eq!(refused.code(), tonic::Code::Unavailable);
    assert!(refused.message().starts_with("unauthorized: "), "{}", refused.message());
}

/// a-products-events-are-pushed-through-one-watch: a watch pushes the log's events, and the instance's drain
/// ends it `draining` after what it had read, its thread ended.
#[tokio::test(flavor = "multi_thread")]
async fn a_drain_ends_a_watch_draining_and_its_thread() {
    let f = fixture();
    let face = f.face(Authorize::allow_all());
    let ask = Arc::new(Notify::new());
    let asked = Arc::new(AtomicBool::new(false));
    let mut request = watch(10);
    let (n, a) = (Arc::clone(&ask), Arc::clone(&asked));
    request.extensions_mut().insert(StreamEnd::new(move || {
        let (n, a) = (Arc::clone(&n), Arc::clone(&a));
        async move {
            while !a.load(Ordering::SeqCst) {
                n.notified().await;
            }
        }
    }));
    let mut stream = face.watch(request).await.expect("opens").into_inner();
    f.publish(1);
    let first = stream.next().await.expect("a frame").expect("not refused");
    assert_eq!(events_in(&first), 1);

    asked.store(true, Ordering::SeqCst);
    ask.notify_waiters();
    let ended = loop {
        match stream.next().await.expect("the stream says its end") {
            Ok(_) => {}
            Err(status) => break status,
        }
    };
    assert_eq!((ended.code(), ended.message()), (tonic::Code::Unavailable, "draining"));
    assert!(stream.next().await.is_none(), "nothing after the end");
    until_no_watches(&face).await;
}

/// A consumer that goes away stops its watch: the thread ends and its place under the cap is free.
#[tokio::test(flavor = "multi_thread")]
async fn a_consumer_gone_stops_its_watch() {
    let f = fixture();
    let face = f.face(Authorize::allow_all());
    let stream = face.watch(watch(10)).await.expect("opens").into_inner();
    assert_eq!(face.watches_open(), 1);
    drop(stream);
    until_no_watches(&face).await;
}

/// A watch ends `expired` at its token's life on the injected clock; re-opened after a revoke, it is refused
/// `unauthorized`, so a revoked grant stops a watch within one token's life.
#[tokio::test(flavor = "multi_thread")]
async fn a_revoke_between_renewals_ends_the_watch_at_the_next() {
    let f = fixture();
    let revoked = Arc::new(AtomicBool::new(false));
    let r = Arc::clone(&revoked);
    let face = f.face(Authorize::new(move |_| {
        let revoked = r.load(Ordering::SeqCst);
        async move { if revoked { Err("the grant was revoked".to_owned()) } else { Ok(()) } }
    }));
    let mut stream = face.watch(watch(10)).await.expect("opens").into_inner();
    let reading = tokio::spawn(async move {
        let mut last = None;
        while let Some(item) = stream.next().await {
            last = Some(item);
        }
        last
    });
    assert!(f.manual.until_waiting(1, PARK).await, "the stream waits on its token's expiry");
    revoked.store(true, Ordering::SeqCst);
    f.manual.settle().await;
    assert!(!reading.is_finished(), "a revoke alone doesn't reach an open stream before its renewal");
    f.manual.advance(Duration::from_millis(anemnez_extension::principal::MAX_TTL_MS)).await;
    let ended = reading.await.expect("joins").expect("an end").expect_err("ended by name");
    assert_eq!((ended.code(), ended.message()), (tonic::Code::Unauthenticated, "expired"));

    let renewed = face.watch(watch(10)).await.err().expect("refused at renewal");
    assert_eq!(renewed.message(), "unauthorized: the grant was revoked");
    until_no_watches(&face).await;
}

/// A face holds at most its cap of watches: the next is refused `events-watch-busy` at once, the ones held
/// are all served, and a freed place is taken again.
#[tokio::test(flavor = "multi_thread")]
async fn watches_past_the_cap_are_refused_and_none_starved() {
    let f = fixture();
    let face = f.face(Authorize::allow_all()).with_watches_max(2);
    let mut a = face.watch(watch(10)).await.expect("opens").into_inner();
    let mut b = face.watch(watch(10)).await.expect("opens").into_inner();
    let busy = face.watch(watch(10)).await.err().expect("refused");
    assert_eq!(busy.code(), tonic::Code::ResourceExhausted);
    assert!(busy.message().starts_with("events-watch-busy"), "{}", busy.message());

    f.publish(1);
    for stream in [&mut a, &mut b] {
        let frame = stream.next().await.expect("a frame").expect("not refused");
        assert_eq!(events_in(&frame), 1, "every held watch is served");
    }
    drop(a);
    let ended = face.watch_ended();
    let freed = anemnez_clock::Clock::system()
        .timeout(PARK, async {
            loop {
                let notified = ended.notified();
                if face.watches_open() < 2 {
                    return;
                }
                notified.await;
            }
        })
        .await;
    assert!(freed.is_ok(), "the dropped watch's thread ended");
    let _c = face.watch(watch(10)).await.expect("a freed place is taken again");
    drop(b);
}

/// A consumer that holds its stream unread stalls only that stream: every append the writer makes returns
/// while it holds (by value, not by timing), and the consumer then reads them all, in order.
#[tokio::test(flavor = "multi_thread")]
async fn a_held_consumer_never_blocks_the_writer() {
    let f = fixture();
    let face = f.face(Authorize::allow_all());
    // One event a frame, so the thread fills its bounded channel and waits on its consumer.
    let mut held = face.watch(watch(1)).await.expect("opens").into_inner();
    let appends = 50;
    for n in 0..appends {
        f.publish(n);
    }
    let mut read = 0;
    while read < appends {
        let frame = held.next().await.expect("a frame").expect("not refused");
        read += events_in(&frame);
    }
    assert_eq!(read, appends);
}
