//! ReRead's rows (D m/31023, L m/31024): a log opened for each read, waited on outside its lock, woken by its
//! changes, never losing a write that lands between a read and its wait.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anemnez_clock::ManualClock;
use hydramq_events::{Events, Face, Position, Read};

use super::{Changes, ReRead};
use crate::log_events::{Busy, Log};

/// The real-time bound a row waits on a wake, never the product's clock.
const PARK: Duration = Duration::from_secs(5);

/// A per-read log over a held one, with its own changes. `after_first_read`, when set, writes once right after the
/// first read, inside the open and before any wait: the exact window a lost wake would miss.
/// A write a row lands right after the first read, inside the open.
type Write = Box<dyn FnOnce(&Events) + Send>;

struct Opened {
    log: Arc<Events>,
    changes: Option<Arc<Changes>>,
    opens: AtomicUsize,
    after_first_read: Mutex<Option<Write>>,
}

impl Log for Opened {
    fn name(&self) -> &str {
        "audit"
    }

    fn with(&self, _within: Duration, read: &mut dyn FnMut(&Events)) -> Result<(), Busy> {
        self.opens.fetch_add(1, Ordering::SeqCst);
        read(&self.log);
        if let Some(write) = self.after_first_read.lock().unwrap().take() {
            write(&self.log);
        }
        Ok(())
    }

    fn changes(&self) -> Option<Arc<Changes>> {
        self.changes.clone()
    }
}

fn log(dir: &std::path::Path) -> Arc<Events> {
    let clock = ManualClock::new();
    let log = Events::open(&dir.join("log"), "horaid", "audit", hydramq_events::Settings::default(), &clock.clock())
        .expect("the log opens");
    std::mem::forget(clock);
    Arc::new(log)
}

fn publish(log: &Events, n: usize) {
    let body = hydramq_events::body("horai.shipped", &[]).expect("encodes");
    log.publish_next(&format!("ship/{n}"), "shipped", &body, 1).expect("published");
}

fn from_start() -> Position {
    Position { log: "audit".into(), offset: 0 }
}

fn events(read: &Read) -> usize {
    match read {
        Read::Events { events, .. } => events.len(),
        other => panic!("not events: {other:?}"),
    }
}

/// D m/31023: a write landing between a read and its wait, bumping the changes, is delivered at once, never at the
/// bound: the manual clock is never advanced, so only the moved generation could have woken the wait.
#[test]
fn a_write_between_the_read_and_the_wait_is_never_lost() {
    let dir = tempfile::tempdir().unwrap();
    let manual = ManualClock::new();
    let changes = Changes::new(&manual.clock());
    let bump = Arc::clone(&changes);
    let opened = Arc::new(Opened {
        log: log(dir.path()),
        changes: Some(changes),
        opens: AtomicUsize::new(0),
        after_first_read: Mutex::new(Some(Box::new(move |log| {
            publish(log, 1);
            bump.bump();
        }))),
    });
    let reread = ReRead::new(opened.clone(), Duration::from_secs(1), &manual.clock());
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let _ = tx.send(reread.read_until(&from_start(), 10, Duration::from_secs(60), None).map(|r| events(&r)));
    });
    let got = rx.recv_timeout(PARK).expect("woken by the change, never the 60 s bound");
    assert_eq!(got.expect("read"), 1, "the write that landed in the window");
    assert_eq!(opened.opens.load(Ordering::SeqCst), 2, "read, then read again once woken");
}

/// L m/31024: a long-poll's wait sits outside the open, woken by the log's owner bumping its changes; with no source
/// it waits on its bound alone, said by sourced().
#[tokio::test(flavor = "multi_thread")]
async fn a_wait_is_outside_the_open_and_woken_by_changes_or_its_bound() {
    let dir = tempfile::tempdir().unwrap();
    let manual = ManualClock::new();
    let changes = Changes::new(&manual.clock());
    let held = log(dir.path());
    let opened = Arc::new(Opened {
        log: Arc::clone(&held),
        changes: Some(Arc::clone(&changes)),
        opens: AtomicUsize::new(0),
        after_first_read: Mutex::new(None),
    });
    let reread = ReRead::new(opened.clone(), Duration::from_secs(1), &manual.clock());
    assert!(reread.sourced());
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let _ = tx.send(reread.read_until(&from_start(), 10, Duration::from_secs(60), None).map(|r| events(&r)));
    });
    // The first read found nothing and the wait is outside the open: one open so far, none held now.
    while opened.opens.load(Ordering::SeqCst) == 0 {
        std::thread::yield_now();
    }
    publish(&held, 1);
    changes.bump();
    assert_eq!(rx.recv_timeout(PARK).expect("woken").expect("read"), 1);

    let quiet =
        Arc::new(Opened { log: held, changes: None, opens: AtomicUsize::new(0), after_first_read: Mutex::new(None) });
    let past = Position { log: "audit".into(), offset: 1 };
    let unsourced = ReRead::new(quiet, Duration::from_secs(1), &manual.clock());
    assert!(!unsourced.sourced(), "no source: the bound is the only wake");
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let _ = tx.send(unsourced.read_until(&past, 10, Duration::from_secs(30), None).map(|r| events(&r)));
    });
    assert!(rx.recv_timeout(Duration::from_millis(200)).is_err(), "waits while its bound hasn't passed");
    manual.advance(Duration::from_secs(30)).await;
    assert_eq!(rx.recv_timeout(PARK).expect("its bound").expect("read"), 0, "nothing past it, at the bound");
}
