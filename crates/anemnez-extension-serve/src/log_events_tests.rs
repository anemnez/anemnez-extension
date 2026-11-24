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
use crate::log_events::{Admit, Authorize, LogEvents, Method, Revoke};
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
    assert_eq!(
        ended.metadata().get(anemnez_extension::refusal::END_METADATA_KEY).and_then(|v| v.to_str().ok()),
        Some(anemnez_extension::refusal::WATCH_RENEW),
        "the end is typed as a renewal by its trailer, never by its message"
    );

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

/// A log whose reads wait on a clock the row holds: a read asked to wait for events on an empty log blocks its
/// thread until the row advances that clock, which it does at the end so no thread outlives the row.
fn held_fixture() -> (Fixture, ManualClock) {
    let dir = tempfile::tempdir().expect("a temp dir");
    let log_clock = ManualClock::new();
    let log = Arc::new(
        Events::open(&dir.path().join("log"), "miniond", LOG, hydramq_events::Settings::default(), &log_clock.clock())
            .expect("the log opens"),
    );
    (Fixture { _dir: dir, log, manual: ManualClock::new() }, log_clock)
}

/// A read that waits for events, on the empty log: it holds its thread until the log's clock moves.
fn held_read() -> tonic::Request<pb::CallRequest> {
    tonic::Request::new(pb::CallRequest { request: read_request(&from_start(), 1, Duration::from_secs(30)).unwrap() })
}

/// a-call-is-bounded-by-one-deadline (E m/20675, rule 4): the product's authorization and the log's read share one
/// deadline from admit. An authorization that answers late leaves the read only what remains, so the call ends by
/// the deadline, before the caller's own bound, and names the read as the stage that ran out.
#[tokio::test(flavor = "multi_thread")]
async fn a_late_authorization_leaves_the_read_only_what_remains() {
    use crate::log_events::FACE_WITHIN;
    let (f, log_clock) = held_fixture();
    let late = Duration::from_secs(40);
    let clock = f.manual.clock();
    let face = Arc::new(f.face(Authorize::new(move |_| {
        let clock = clock.clone();
        async move {
            clock.sleep(late).await;
            Ok(())
        }
    })));
    let calling = Arc::clone(&face);
    let mut call = tokio::spawn(async move { calling.call(held_read()).await });
    let authorizing = f.manual.until_waiting(1, PARK).await;
    f.manual.advance(late).await;
    // The read now waits on its bound: what the deadline has left, never CALL_WITHIN whole.
    let reading = f.manual.until_waiting(1, PARK).await;
    let left = FACE_WITHIN - late;
    f.manual.advance(left - Duration::from_millis(1)).await;
    let early = call.is_finished();
    f.manual.advance(Duration::from_millis(1)).await;
    let ended = anemnez_clock::Clock::system().timeout(Duration::from_secs(5), &mut call).await;
    // The held read's thread is let go before any assert, so a red row fails and never hangs the runtime.
    let _ = log_clock.until_waiting(1, PARK).await;
    log_clock.advance(Duration::from_secs(30)).await;
    assert!(authorizing && reading, "the authorization, then the read, waited on the face's clock");
    assert!(!early, "the read still had a millisecond");
    let refused = ended.expect("the call ends at the deadline").expect("joins").expect_err("refused");
    assert_eq!(refused.code(), tonic::Code::Unavailable);
    assert!(refused.message().starts_with("events-call-unanswered: "), "the read is named: {}", refused.message());
    assert!(late + left < crate::log_events::CALLER_WITHIN, "the face ends inside the caller's minute");
}

/// A face holds at most its cap of reads: the next call is refused `events-call-busy` at once, and a read past
/// its bound still counts until its thread ends (D m/20673).
#[tokio::test(flavor = "multi_thread")]
async fn calls_past_the_cap_are_refused_at_once() {
    let (f, log_clock) = held_fixture();
    let face = Arc::new(f.face(Authorize::allow_all()).with_calls_max(1));
    let holding = Arc::clone(&face);
    let held = tokio::spawn(async move { holding.call(held_read()).await });
    let counted = anemnez_clock::Clock::system()
        .timeout(PARK, async {
            while face.calls_open() < 1 {
                tokio::task::yield_now().await;
            }
        })
        .await;
    let second = anemnez_clock::Clock::system().timeout(Duration::from_secs(5), face.call(held_read())).await;
    // The held read times out on the face, and still counts until its thread ends.
    let armed = f.manual.until_waiting(1, PARK).await;
    f.manual.advance(crate::log_events::CALL_WITHIN).await;
    let held_ended = anemnez_clock::Clock::system().timeout(Duration::from_secs(5), held).await.is_ok();
    let still_counted = face.calls_open();
    // Every held read's thread is let go before any assert, so a red row fails and never hangs the runtime.
    let _ = log_clock.until_waiting(1, PARK).await;
    log_clock.advance(Duration::from_secs(30)).await;
    assert!(counted.is_ok() && armed, "the first read holds its slot and waits on its bound");
    let busy = second.expect("refused at once, never waiting").expect_err("refused");
    assert_eq!(busy.code(), tonic::Code::ResourceExhausted);
    assert!(busy.message().starts_with("events-call-busy"), "{}", busy.message());
    assert!(held_ended, "the held call ends at its bound");
    assert_eq!(still_counted, 1, "a read past its bound counts until its thread ends");
    let freed = anemnez_clock::Clock::system()
        .timeout(PARK, async {
            while face.calls_open() > 0 {
                tokio::task::yield_now().await;
            }
        })
        .await;
    assert!(freed.is_ok(), "the slot is freed when the read's thread ends");
    face.call(tonic::Request::new(pb::CallRequest {
        request: read_request(&from_start(), 1, Duration::ZERO).unwrap(),
    }))
    .await
    .expect("served once the slot is free");
}

/// a-watch-ends-when-its-authorization-does: a watch admitted under a [`Revoke`] the product keeps sends what it
/// read, and the revoke ends it `unauthorized` at once, long before its token's life, stopping its thread.
#[tokio::test(flavor = "multi_thread")]
async fn a_revoke_mid_stream_ends_the_watch_unauthorized_after_what_was_sent() {
    let f = fixture();
    let hold = Revoke::new();
    let given = hold.clone();
    let face = f.face(Authorize::holding(move |_| {
        let given = given.clone();
        async move { Ok(Admit::admitted().revoked_by(given)) }
    }));
    let mut stream = face.watch(watch(10)).await.expect("opens").into_inner();
    f.publish(1);
    let first = stream.next().await.expect("a frame").expect("not refused");
    assert_eq!(events_in(&first), 1, "what was read before the revoke is sent");

    hold.revoke();
    let ended = anemnez_clock::Clock::system()
        .timeout(PARK, async {
            loop {
                match stream.next().await.expect("the stream says its end") {
                    Ok(_) => {}
                    Err(status) => break status,
                }
            }
        })
        .await
        .expect("the revoke ended the stream, not the token's life");
    assert_eq!(ended.code(), tonic::Code::PermissionDenied);
    assert!(ended.message().starts_with("unauthorized"), "{}", ended.message());
    assert!(stream.next().await.is_none(), "nothing after the end");
    until_no_watches(&face).await;
}

/// a-watch-ends-when-its-authorization-does: a watch admitted until a grant's end, short of its token's life,
/// ends `unauthorized` when that end passes on the injected clock, not `expired` at the token's.
#[tokio::test(flavor = "multi_thread")]
async fn a_watch_admitted_until_an_end_ends_unauthorized_at_it() {
    let f = fixture();
    let clock = f.manual.clock();
    let face = f.face(Authorize::holding(move |_| {
        let until = clock.now_mono() + Duration::from_secs(30);
        async move { Ok(Admit::admitted().until(until)) }
    }));
    let mut stream = face.watch(watch(10)).await.expect("opens").into_inner();
    let reading = tokio::spawn(async move {
        let mut last = None;
        while let Some(item) = stream.next().await {
            last = Some(item);
        }
        last
    });
    assert!(f.manual.until_waiting(2, PARK).await, "the stream waits on the grant's end and its token's expiry");
    f.manual.advance(Duration::from_secs(29)).await;
    f.manual.settle().await;
    assert!(!reading.is_finished(), "still held short of the end");
    f.manual.advance(Duration::from_secs(1)).await;
    let ended = anemnez_clock::Clock::system()
        .timeout(PARK, reading)
        .await
        .expect("the grant's end ended the stream")
        .expect("joins")
        .expect("an end")
        .expect_err("ended by name");
    assert_eq!(ended.code(), tonic::Code::PermissionDenied);
    assert!(ended.message().starts_with("unauthorized"), "{}", ended.message());
    until_no_watches(&face).await;
}

/// The product's authorization is handed the request's metadata as the call carried it, so it can attribute the
/// call (Agora's stamp naming the lane) beside the principal it decides on.
#[tokio::test(flavor = "multi_thread")]
async fn authorize_is_handed_the_requests_metadata() {
    let f = fixture();
    let lanes = Arc::new(std::sync::Mutex::new(Vec::new()));
    let seen = Arc::clone(&lanes);
    let face = f.face(Authorize::new(move |a| {
        seen.lock()
            .unwrap()
            .push(a.metadata.get("anemnez-caller-lane").and_then(|v| v.to_str().ok()).map(String::from));
        async { Err("attributed only".to_owned()) }
    }));
    let mut request =
        tonic::Request::new(pb::CallRequest { request: read_request(&from_start(), 10, Duration::ZERO).unwrap() });
    request.metadata_mut().insert("anemnez-caller-lane", "K".parse().unwrap());
    let _ = face.call(request).await;
    assert_eq!(*lanes.lock().unwrap(), vec![Some("K".to_owned())]);
}

/// A log opened for each read, as horaid opens Horai's (L m/30712): it counts its opens, records the bound each was
/// handed, and is busy (its writer holds it) when told.
struct PerRead {
    log: Arc<Events>,
    opens: std::sync::atomic::AtomicUsize,
    busy: AtomicBool,
    within: std::sync::Mutex<Vec<Duration>>,
}

impl crate::log_events::Log for PerRead {
    fn with(&self, within: Duration, read: &mut dyn FnMut(&Events)) -> Result<(), crate::log_events::Busy> {
        self.within.lock().unwrap().push(within);
        if self.busy.load(Ordering::SeqCst) {
            return Err(crate::log_events::Busy("its stream's flock is held by `horai ship`".into()));
        }
        self.opens.fetch_add(1, Ordering::SeqCst);
        read(&self.log);
        Ok(())
    }
}

/// L m/30712: a log opened for each read serves Call through one open per call, handed what the call has left, and a
/// log its writer holds is events-call-busy by name, nothing read; its Watch is refused as a Journal's is, so the
/// client reads watch-unserved, and no watch thread starts.
#[tokio::test(flavor = "multi_thread")]
async fn a_log_opened_per_read_serves_call_once_per_call_and_says_busy() {
    let f = fixture();
    f.publish(1);
    let per_read = Arc::new(PerRead {
        log: Arc::clone(&f.log),
        opens: std::sync::atomic::AtomicUsize::new(0),
        busy: AtomicBool::new(false),
        within: std::sync::Mutex::new(Vec::new()),
    });
    let verifier = crate::principal_tests::verifier(&f.manual, true);
    let face = LogEvents::over(per_read.clone(), verifier, Authorize::allow_all(), f.manual.clock());
    let read =
        || tonic::Request::new(pb::CallRequest { request: read_request(&from_start(), 10, Duration::ZERO).unwrap() });

    for n in 1..=2 {
        let reply = face.call(read()).await.expect("served").into_inner();
        assert!(matches!(read_reply(&reply.reply), Ok(Read::Events { ref events, .. }) if events.len() == 1));
        assert_eq!(per_read.opens.load(Ordering::SeqCst), n, "one open per call");
    }
    let handed = per_read.within.lock().unwrap().clone();
    assert!(
        handed.iter().all(|w| *w > Duration::ZERO && *w <= crate::log_events::CALL_WITHIN),
        "each open is bounded by what the call has left: {handed:?}"
    );

    per_read.busy.store(true, Ordering::SeqCst);
    let busy = face.call(read()).await.expect_err("its writer holds it");
    assert_eq!(busy.code(), tonic::Code::ResourceExhausted);
    assert!(busy.message().starts_with("events-call-busy: the log's writer holds it"), "{}", busy.message());
    assert_eq!(per_read.opens.load(Ordering::SeqCst), 2, "nothing read while busy");

    let unserved = face.watch(watch(10)).await.err().expect("refused");
    assert_eq!(unserved.code(), tonic::Code::Unimplemented);
    assert!(unserved.message().starts_with("events-call-unserved: "), "{}", unserved.message());
    assert_eq!(face.watches_open(), 0, "a refused watch starts no thread");
}
