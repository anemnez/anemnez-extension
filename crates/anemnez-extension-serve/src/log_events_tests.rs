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
    drop(log_clock);
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

    // The Journal's stream, on a log face, is one Call: refused by the same decision.
    let refused = closed.events(tonic::Request::new(pb::EventsRequest::default())).await.err().expect("refused");
    assert_eq!(refused.message(), "unauthorized: no minion.audit grant");
    assert_eq!(asked.lock().unwrap().last(), Some(&Method::Call), "decided as a Call");
}

/// `Events/Events` on a log face (O m/38880): what the flattened MCP `events` tool calls for every product. One
/// bounded read from `after`, every event of an asked kind with its `<log>:<offset>` cursor and canonical bytes, then
/// the stream ends; the last cursor as `after` reads on. A cursor of another log is cursor-not-mine, a misshapen one
/// invalid, by name.
#[tokio::test(flavor = "multi_thread")]
async fn the_journals_stream_on_a_log_face_is_one_bounded_read_by_cursor() {
    let f = fixture();
    for n in 0..3 {
        f.publish(n);
    }
    let face = f.face(Authorize::allow_all());
    let read = |after: &str, kinds: &[&str]| {
        let request =
            pb::EventsRequest { after: after.to_owned(), kinds: kinds.iter().map(|k| (*k).to_owned()).collect() };
        let face = &face;
        async move {
            let stream = face.events(tonic::Request::new(request)).await?.into_inner();
            let got: Vec<Result<pb::Event, tonic::Status>> = stream.collect().await;
            got.into_iter().collect::<Result<Vec<_>, _>>()
        }
    };
    let all = read("", &[]).await.expect("read");
    let first = f.log.read(&from_start(), 10).expect("reads");
    let Read::Events { events, .. } = first else { panic!("events") };
    let want: Vec<(String, String, Vec<u8>)> = events
        .iter()
        .map(|(at, e)| (format!("{LOG}:{}", at.offset), e.kind.clone(), hydramq_events::to_bytes(e).expect("encodes")))
        .collect();
    let got: Vec<(String, String, Vec<u8>)> =
        all.iter().map(|e| (e.cursor.clone(), e.kind.clone(), e.payload.clone())).collect();
    assert_eq!(got, want, "every event, its cursor, kind and canonical bytes, by value");
    assert_eq!(got.len(), 3);

    let after_first = read(&all[0].cursor, &[]).await.expect("read on");
    assert_eq!(
        after_first.iter().map(|e| e.cursor.clone()).collect::<Vec<_>>(),
        [all[1].cursor.clone(), all[2].cursor.clone()]
    );
    assert!(
        read(&all[2].cursor, &[]).await.expect("at the head").is_empty(),
        "nothing past the head, and the stream ends"
    );
    assert_eq!(read(&format!("{LOG}:"), &[]).await.expect("from its start").len(), 3, "<log>: is its start");
    assert!(read("", &["other"]).await.expect("read").is_empty(), "only the kinds asked");
    assert_eq!(read("", &["decided"]).await.expect("read").len(), 3);

    let other = read("herald@studio:4", &[]).await.expect_err("another log's cursor");
    assert!(other.message().starts_with("cursor-not-mine: "), "{}", other.message());
    for bad in ["4", "audit:four", "audit:-1"] {
        let refused = read(bad, &[]).await.expect_err("misshapen");
        assert_eq!(refused.code(), tonic::Code::InvalidArgument, "{bad}: {}", refused.message());
    }
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
    // Every deadline here is an instant from admit, and the row moves the clock to instants, so a timer that registers
    // after a move is already due and fires at once: nothing waits on a count of waiters or a real-time poll (E
    // m/33551, M m/33563). The authorizer's word is the one event the row waits on. HANG only names a hang.
    const HANG: Duration = Duration::from_secs(60);
    let system = anemnez_clock::Clock::system();
    let admitted = f.manual.now();
    let clock = f.manual.clock();
    let (slept_tx, slept) = tokio::sync::oneshot::channel::<()>();
    let slept_tx = Arc::new(std::sync::Mutex::new(Some(slept_tx)));
    // Asked means admitted: the face fixed its one deadline before it asked, so time may move from here.
    let (asked_tx, asked) = tokio::sync::oneshot::channel::<()>();
    let asked_tx = Arc::new(std::sync::Mutex::new(Some(asked_tx)));
    let face = Arc::new(f.face(Authorize::new(move |_| {
        let (clock, slept_tx) = (clock.clone(), Arc::clone(&slept_tx));
        if let Some(tx) = asked_tx.lock().expect("unpoisoned").take() {
            let _ = tx.send(());
        }
        async move {
            clock.sleep_until(admitted + late).await;
            if let Some(tx) = slept_tx.lock().expect("unpoisoned").take() {
                let _ = tx.send(());
            }
            Ok(())
        }
    })));
    let calling = Arc::clone(&face);
    let mut call = tokio::spawn(async move { calling.call(held_read()).await });
    let admitting = system.timeout(HANG, asked).await.is_ok();
    f.manual.advance_to(admitted + late).await;
    let authorized = system.timeout(HANG, slept).await.is_ok();
    // The read now waits on its bound, the call's one deadline: what it has left, never CALL_WITHIN whole.
    let left = FACE_WITHIN - late;
    f.manual.advance_to(admitted + (FACE_WITHIN - Duration::from_millis(1))).await;
    let early = call.is_finished();
    f.manual.advance_to(admitted + FACE_WITHIN).await;
    let ended = system.timeout(HANG, &mut call).await;
    // The held read's thread is let go before any assert, so a red row fails and never hangs the runtime.
    let _ = log_clock.until_waiting(1, PARK).await;
    log_clock.advance(Duration::from_secs(30)).await;
    assert!(admitting && authorized, "asked once admitted, and answered at admit + late");
    assert!(!early, "the read still had a millisecond");
    let refused = ended
        .expect("the row's hang guard: the manual clock never reached the read's deadline")
        .expect("joins")
        .expect_err("refused");
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
    fn name(&self) -> &str {
        LOG
    }

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

/// A log that won't open at all (D m/32403): it says so through `open_with`.
struct Damaged;

impl crate::log_events::Log for Damaged {
    fn name(&self) -> &str {
        LOG
    }

    fn with(&self, _within: Duration, _read: &mut dyn FnMut(&Events)) -> Result<(), crate::log_events::Busy> {
        unreachable!("the face reads through open_with")
    }

    fn open_with(&self, _within: Duration, _read: &mut dyn FnMut(&Events)) -> Result<(), crate::log_events::Unopened> {
        Err(crate::log_events::Unopened::Unreadable("its index doesn't read: a record past its end".into()))
    }
}

/// D m/32403: a log that can't be read is answered events-log-unreadable, FAILED_PRECONDITION, by name and with its
/// reason: never events-call-busy, so a follower doesn't retry a broken log forever as if it were load.
#[tokio::test(flavor = "multi_thread")]
async fn a_log_that_cannot_be_read_is_unreadable_by_name_never_busy() {
    let f = fixture();
    let verifier = crate::principal_tests::verifier(&f.manual, true);
    let face = LogEvents::over(Arc::new(Damaged), verifier, Authorize::allow_all(), f.manual.clock());
    let read =
        tonic::Request::new(pb::CallRequest { request: read_request(&from_start(), 10, Duration::ZERO).unwrap() });
    let refused = face.call(read).await.expect_err("it can't be read");
    assert_eq!(refused.code(), tonic::Code::FailedPrecondition, "{}", refused.message());
    assert!(
        refused.message().starts_with("events-log-unreadable: its index doesn't read"),
        "by name, with its reason: {}",
        refused.message()
    );
}

/// L m/30712: a log opened for each read serves Call through one open per call, handed what the call has left, and a
/// log its writer holds is events-call-busy by name, nothing read; its Watch is served through the same re-read
/// (m/31024), its first frame the log's event.
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

    per_read.busy.store(false, Ordering::SeqCst);
    let mut frames = face.watch(watch(10)).await.expect("a per-read log's watch is served").into_inner();
    let first = anemnez_clock::Clock::system().timeout(PARK, frames.next()).await.expect("a frame in time");
    assert_eq!(events_in(&first.expect("a frame").expect("not an error")), 1, "the log's event, through the re-read");
}

/// D m/40656: a page holding none of the asked kinds is read past, inside the one deadline, so an empty answer is
/// only the head. More than a page of another kind, then one `decided`: reading kinds=[decided] reaches it, from the
/// start and from a cursor inside the other kind's run.
#[tokio::test(flavor = "multi_thread")]
async fn a_kind_filtered_read_reads_past_pages_of_other_kinds() {
    let f = fixture();
    let body = hydramq_events::body("minion.decided", &[]).expect("encodes");
    let other = hydramq_events::wire::WATCH_MAX + 5;
    for n in 0..other {
        f.log.publish_next(&format!("noise/{n}"), "noise", &body, 1).expect("published");
    }
    f.publish(0);
    let face = f.face(Authorize::allow_all());
    let read = |after: String| {
        let request = pb::EventsRequest { after, kinds: vec!["decided".to_owned()] };
        let face = &face;
        async move {
            let stream = face.events(tonic::Request::new(request)).await.expect("served").into_inner();
            stream.map(|e| e.expect("an event")).collect::<Vec<_>>().await
        }
    };
    let found = read(String::new()).await;
    assert_eq!(found.iter().map(|e| e.kind.as_str()).collect::<Vec<_>>(), ["decided"], "past a page of noise");
    assert_eq!(found[0].cursor, format!("{LOG}:{other}"));
    assert_eq!(read(format!("{LOG}:3")).await.len(), 1, "from inside the noise");
    assert!(read(found[0].cursor.clone()).await.is_empty(), "and past it, the head");
}

/// A log on its own manual clock, written durably (as hydramq's own compaction rows write), with four events of one
/// subject compacted and one published past the floor; and a face over it.
fn compacted() -> (tempfile::TempDir, Arc<Events>, ManualClock, hydramq_events::Position) {
    let dir = tempfile::tempdir().expect("a temp dir");
    let log_clock = ManualClock::new();
    let settings = hydramq_events::Settings { durability: hydramq_events::Durability::Sync, ..Default::default() };
    let log = Arc::new(
        Events::open(&dir.path().join("log"), "miniond", LOG, settings, &log_clock.clock()).expect("the log opens"),
    );
    drop(log_clock);
    let body = hydramq_events::body("minion.decided", &[]).expect("encodes");
    let ev = |seq: u64| hydramq_events::Event {
        producer: "miniond".into(),
        subject: "call/x".into(),
        kind: "decided".into(),
        seq,
        wall_ms: 1,
        body: body.clone(),
        ..Default::default()
    };
    for seq in 0..4 {
        log.publish(&ev(seq)).expect("published");
    }
    log.compact().expect("compacts");
    log.publish(&ev(4)).expect("after the floor");
    let Read::Compacted { floor } = log.read(&from_start(), 10).expect("reads") else { panic!("not compacted") };
    (dir, log, ManualClock::new(), floor)
}

#[allow(clippy::result_large_err)] // A tonic Status is the face's own error, returned as is.
async fn events_after(face: &LogEvents, after: &str) -> Result<Vec<pb::Event>, tonic::Status> {
    let request = pb::EventsRequest { after: after.to_owned(), kinds: Vec::new() };
    let stream = face.events(tonic::Request::new(request)).await?.into_inner();
    stream.collect::<Vec<_>>().await.into_iter().collect()
}

/// D m/40656 (should): a read of a compacted log's start begins past its floor and reads what was published after it,
/// never out-of-range; a cursor below the floor is out-of-range, naming the floor to read on from (E m/40661).
#[tokio::test(flavor = "multi_thread")]
async fn a_compacted_logs_start_begins_past_its_floor_and_a_cursor_below_it_is_out_of_range() {
    let (_dir, log, manual, floor) = compacted();
    let face = LogEvents::new(
        Arc::clone(&log),
        crate::principal_tests::verifier(&manual, true),
        Authorize::allow_all(),
        manual.clock(),
    );
    let past = hydramq_events::Position { log: LOG.into(), offset: floor.offset + 1 };
    let Read::Events { events, .. } = log.read(&past, 10).expect("reads") else { panic!("past the floor reads") };
    let want: Vec<String> = events.iter().map(|(at, _)| format!("{LOG}:{}", at.offset)).collect();
    assert!(!want.is_empty(), "the event published past the floor is there to read");
    for start in ["", "audit:"] {
        let got = events_after(&face, start).await.expect("the start of a compacted log is past its floor");
        assert_eq!(got.iter().map(|e| e.cursor.clone()).collect::<Vec<_>>(), want, "{start:?}");
    }
    assert!(floor.offset >= 1, "a floor above the first offset, so a cursor can be below it: {floor:?}");
    let below = events_after(&face, &format!("{LOG}:{}", floor.offset - 1)).await.expect_err("below the floor");
    assert_eq!(below.code(), tonic::Code::OutOfRange);
    assert!(below.message().starts_with("out-of-range: "), "{}", below.message());
    assert!(below.message().ends_with(&format!("{LOG}:{}", floor.offset)), "names the floor: {}", below.message());
}

/// E m/40661: on a face of several logs, an empty `after` names no log, so it is read-needs-its-log, naming them; a
/// log's own start, `<log>:`, reads that log.
#[tokio::test(flavor = "multi_thread")]
async fn an_empty_cursor_on_a_many_log_face_is_refused_naming_its_logs() {
    let dir = tempfile::tempdir().expect("a temp dir");
    let log_clock = ManualClock::new();
    let open = |name: &str| -> Arc<dyn crate::log_events::Log> {
        let log = Events::open(
            &dir.path().join(name),
            "miniond",
            name,
            hydramq_events::Settings::default(),
            &log_clock.clock(),
        )
        .expect("the log opens");
        let body = hydramq_events::body("minion.decided", &[]).expect("encodes");
        log.publish_next("call/x", "decided", &body, 1).expect("published");
        Arc::new(Arc::new(log))
    };
    let manual = ManualClock::new();
    let logs = vec![("audit".to_owned(), open("audit")), ("moves".to_owned(), open("moves"))];
    let face = LogEvents::over_many(
        logs,
        crate::principal_tests::verifier(&manual, true),
        Authorize::allow_all(),
        manual.clock(),
    )
    .expect("two named logs");
    let refused = events_after(&face, "").await.expect_err("no log named");
    assert_eq!(refused.code(), tonic::Code::InvalidArgument);
    assert!(refused.message().starts_with("read-needs-its-log: "), "{}", refused.message());
    assert!(refused.message().contains("audit, moves"), "names the logs: {}", refused.message());
    let moves = events_after(&face, "moves:").await.expect("a log's own start");
    assert_eq!(moves.iter().map(|e| e.cursor.clone()).collect::<Vec<_>>(), ["moves:0"]);
    drop(log_clock);
}

/// A log whose first read is served and every later one never answers until the row lets it go (its gate opens on
/// drop, so a red row fails and never hangs): a scan the deadline cuts mid-page.
struct StallsAfterOne {
    log: Arc<Events>,
    reads: std::sync::atomic::AtomicUsize,
    entered: std::sync::Mutex<Option<tokio::sync::oneshot::Sender<()>>>,
    gate: Arc<(std::sync::Mutex<bool>, std::sync::Condvar)>,
    /// Its later reads are busy at once, instead of never answering.
    busy: bool,
}

/// Opens the gate when the row ends, whatever it asserted.
struct Opens(Arc<(std::sync::Mutex<bool>, std::sync::Condvar)>);

impl Drop for Opens {
    fn drop(&mut self) {
        *self.0.0.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = true;
        self.0.1.notify_all();
    }
}

impl crate::log_events::Log for StallsAfterOne {
    fn name(&self) -> &str {
        LOG
    }

    fn with(&self, _within: Duration, read: &mut dyn FnMut(&Events)) -> Result<(), crate::log_events::Busy> {
        if self.reads.fetch_add(1, Ordering::SeqCst) == 0 {
            read(&self.log);
            return Ok(());
        }
        if self.busy {
            return Err(crate::log_events::Busy("its writer holds it".into()));
        }
        if let Some(tx) = self.entered.lock().unwrap().take() {
            let _ = tx.send(());
        }
        let (open, rung) = &*self.gate;
        let mut opened = open.lock().unwrap();
        while !*opened {
            opened = rung.wait(opened).unwrap();
        }
        Err(crate::log_events::Busy("let go at the row's end".into()))
    }
}

/// D m/40669: a kind-filtered scan whose next page's read the deadline cuts short still names where to read on (the
/// last scanned cursor), never read_once's bare unanswered, so a caller never re-scans the same run for ever.
#[tokio::test(flavor = "multi_thread")]
async fn a_scan_the_deadline_cuts_mid_read_names_where_to_read_on() {
    use crate::log_events::FACE_WITHIN;
    const HANG: Duration = Duration::from_secs(60);
    let f = fixture();
    let body = hydramq_events::body("minion.decided", &[]).expect("encodes");
    for n in 0..hydramq_events::wire::WATCH_MAX + 5 {
        f.log.publish_next(&format!("noise/{n}"), "noise", &body, 1).expect("published");
    }
    let gate = Arc::new((std::sync::Mutex::new(false), std::sync::Condvar::new()));
    let _opens = Opens(Arc::clone(&gate));
    let (entered_tx, entered) = tokio::sync::oneshot::channel();
    let stalls = StallsAfterOne {
        log: Arc::clone(&f.log),
        reads: std::sync::atomic::AtomicUsize::new(0),
        entered: std::sync::Mutex::new(Some(entered_tx)),
        gate: Arc::clone(&gate),
        busy: false,
    };
    let verifier = crate::principal_tests::verifier(&f.manual, true);
    let face = Arc::new(LogEvents::over(Arc::new(stalls), verifier, Authorize::allow_all(), f.manual.clock()));
    let admitted = f.manual.now();
    let asking = Arc::clone(&face);
    let mut call = tokio::spawn(async move {
        let request = pb::EventsRequest { after: String::new(), kinds: vec!["decided".to_owned()] };
        asking.events(tonic::Request::new(request)).await.map(|_| ())
    });
    let system = anemnez_clock::Clock::system();
    assert!(system.timeout(HANG, entered).await.is_ok(), "the second page's read began");
    f.manual.advance_to(admitted + FACE_WITHIN).await;
    let ended = system.timeout(HANG, &mut call).await.expect("ended by the deadline").expect("joined");
    let told = ended.expect_err("cut short");
    let last = hydramq_events::wire::WATCH_MAX - 1;
    assert_eq!(told.code(), tonic::Code::Unavailable);
    assert!(
        told.message().starts_with("events-call-unanswered: ")
            && told.message().ends_with(&format!("read on from after = \"{LOG}:{last}\"")),
        "names the last scanned cursor: {}",
        told.message()
    );
}

/// D m/40700: a page's read refused busy mid-scan keeps busy's own retryable name and says where to read on.
#[tokio::test(flavor = "multi_thread")]
async fn a_scan_refused_busy_mid_read_names_where_to_read_on() {
    let f = fixture();
    let body = hydramq_events::body("minion.decided", &[]).expect("encodes");
    for n in 0..hydramq_events::wire::WATCH_MAX + 5 {
        f.log.publish_next(&format!("noise/{n}"), "noise", &body, 1).expect("published");
    }
    let busy = StallsAfterOne {
        log: Arc::clone(&f.log),
        reads: std::sync::atomic::AtomicUsize::new(0),
        entered: std::sync::Mutex::new(None),
        gate: Arc::new((std::sync::Mutex::new(true), std::sync::Condvar::new())),
        busy: true,
    };
    let verifier = crate::principal_tests::verifier(&f.manual, true);
    let face = LogEvents::over(Arc::new(busy), verifier, Authorize::allow_all(), f.manual.clock());
    let request = pb::EventsRequest { after: String::new(), kinds: vec!["decided".to_owned()] };
    let told = face.events(tonic::Request::new(request)).await.map(|_| ()).expect_err("busy");
    let last = hydramq_events::wire::WATCH_MAX - 1;
    assert_eq!(told.code(), tonic::Code::ResourceExhausted);
    assert!(
        told.message().starts_with("events-call-busy: ")
            && told.message().ends_with(&format!("read on from after = \"{LOG}:{last}\"")),
        "busy, and where to read on: {}",
        told.message()
    );
}

// ── A house reader (D m/39080, m/40623) ────────────────────────────────────────────────────────────────────────

/// A connection judged `role`, its peer signed by `team` as `identifier`.
fn judged_as(role: anemnez_extension::peer::Role, team: &str, identifier: &str) -> crate::accepted::Judged {
    let code = Ok(anemnez_extension::peer::Code { identifier: identifier.into(), team: team.into() });
    let peer = anemnez_extension::peer::Peer::from_token([501, 501, 20, 501, 20, 41, 100_001, 7], code);
    crate::accepted::Accepted::new((), peer, role).judged().clone()
}

/// D m/40623 (1, 2): the house principal is a house reader's code identity, or a carried caller's whose preface proves
/// the house's team; never another team's, never an ordinary local caller's, and never a carried caller unproven.
#[test]
fn the_house_principal_is_a_house_team_code_identity_and_nothing_else() {
    use anemnez_extension::peer::Role;
    use anemnez_trust::ANEMNEZ_TEAM;
    let house = crate::log_events::house_of(Some(&judged_as(Role::HouseReader, ANEMNEZ_TEAM, "ai.anemnez.themisd")));
    let house = house.expect("a house reader is a house principal");
    assert_eq!((house.identifier.as_str(), house.via), ("ai.anemnez.themisd", None));
    assert_eq!(
        crate::log_events::house_of(Some(&judged_as(Role::HouseReader, "OTHERTEAM", "x"))),
        None,
        "another team"
    );
    assert_eq!(crate::log_events::house_of(Some(&judged_as(Role::Other, ANEMNEZ_TEAM, "x"))), None, "rest's own role");
    assert_eq!(crate::log_events::house_of(None), None, "unjudged");

    let mut carried = judged_as(Role::Carried, "AEGISTEAM", "ai.anemnez.aegisd");
    assert_eq!(crate::log_events::house_of(Some(&carried)), None, "a carried caller with no preface is unproven");
    let holder = anemnez_extension::carried::Holder { unique_id: 9, boot: "boot".into() };
    let code =
        anemnez_extension::carried::CodeIdentity { team: ANEMNEZ_TEAM.into(), identifier: "ai.anemnez.ergond".into() };
    carried.carried = Some(anemnez_extension::carried::Preface::new("node-b", Vec::new(), holder).with_code(code));
    let far = crate::log_events::house_of(Some(&carried)).expect("a proven house carried caller");
    assert_eq!((far.identifier.as_str(), far.via.as_deref()), ("ai.anemnez.ergond", Some("node-b")));
}

/// D m/40623 (1): a house reader with no token reads the log with the token check never asked (this verifier refuses
/// every absent call), and Authorize sees it as itself: a House principal, no token's claims. An ordinary caller with
/// no token is still refused by the verifier, as before.
#[tokio::test(flavor = "multi_thread")]
async fn a_house_reader_reads_the_log_with_no_token_as_its_code_identity() {
    use anemnez_extension::peer::Role;
    let f = fixture();
    f.publish(0);
    let seen = Arc::new(std::sync::Mutex::new(Vec::new()));
    let saw = Arc::clone(&seen);
    let verifier = crate::principal_tests::verifier(&f.manual, false);
    let authorize = Authorize::new(move |a| {
        saw.lock().unwrap().push((a.house.map(|h| h.identifier), a.admitted.is_some()));
        async { Ok(()) }
    });
    let face = LogEvents::new(Arc::clone(&f.log), verifier, authorize, f.manual.clock());
    let call = |judged: crate::accepted::Judged| {
        let mut request =
            tonic::Request::new(pb::CallRequest { request: read_request(&from_start(), 10, Duration::ZERO).unwrap() });
        request.extensions_mut().insert(judged);
        request
    };
    let house = judged_as(Role::HouseReader, anemnez_trust::ANEMNEZ_TEAM, "ai.anemnez.themisd");
    let read = face.call(call(house)).await.expect("a house reader reads");
    let reply = read_reply(&read.into_inner().reply);
    assert!(matches!(reply, Ok(Read::Events { ref events, .. }) if events.len() == 1), "the log was read: {reply:?}");
    assert_eq!(*seen.lock().unwrap(), vec![(Some("ai.anemnez.themisd".to_owned()), false)], "decided as House");

    let other = judged_as(Role::Other, anemnez_trust::ANEMNEZ_TEAM, "ai.anemnez.themisd");
    let refused = face.call(call(other)).await.expect_err("an ordinary caller with no token is refused as before");
    assert_eq!((refused.code(), refused.message()), (tonic::Code::InvalidArgument, "malformed"), "the verifier's own");
    assert_eq!(seen.lock().unwrap().len(), 1, "never decided");
}
