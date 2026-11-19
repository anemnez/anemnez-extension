//! FileNonces: durable before answered, group-committed, pruned, and surviving a crash.

use std::path::Path;
use std::time::Duration;

use anemnez_clock::ManualClock;
use anemnez_extension::principal::Refusal;

use crate::edge::{FILE, FileNonces, HEADER, NEWER, Store, UNREADABLE, job};
use crate::principal::Nonces;

const ISSUER: &str = "01substrate";

fn later(manual: &ManualClock, ms: i64) -> i64 {
    manual.clock().now_wall().unix_millis() + ms
}

/// A nonce recorded is refused as a replay, and still is after the store is closed and reopened:
/// a restart (every upgrade under Fabric is one) loses no nonce.
#[tokio::test]
async fn a_recorded_nonce_is_a_replay_even_after_a_reopen() {
    let manual = ManualClock::new();
    let dir = tempfile::tempdir().expect("a temp dir");
    let until = later(&manual, 330_000);
    let nonces = FileNonces::open(dir.path(), manual.clock()).expect("opens");
    nonces.record(ISSUER, [1; 16], until).await.expect("recorded");
    assert_eq!(nonces.record(ISSUER, [1; 16], until).await, Err(Refusal::Replay));
    assert!(nonces.record("01other", [1; 16], until).await.is_ok(), "the pair is the key");
    drop(nonces);

    let reopened = FileNonces::open(dir.path(), manual.clock()).expect("reopens");
    assert_eq!(reopened.record(ISSUER, [1; 16], until).await, Err(Refusal::Replay));
}

/// What has passed its hold is pruned when the store opens, and the file keeps only the live.
#[tokio::test]
async fn expired_nonces_are_pruned_at_open() {
    let manual = ManualClock::new();
    let dir = tempfile::tempdir().expect("a temp dir");
    let nonces = FileNonces::open(dir.path(), manual.clock()).expect("opens");
    nonces.record(ISSUER, [1; 16], later(&manual, 1_000)).await.expect("recorded");
    nonces.record(ISSUER, [2; 16], later(&manual, 600_000)).await.expect("recorded");
    drop(nonces);

    manual.advance(Duration::from_secs(2)).await;
    let reopened = FileNonces::open(dir.path(), manual.clock()).expect("reopens");
    let text = std::fs::read_to_string(dir.path().join(FILE)).expect("reads");
    assert_eq!(text.lines().count(), 1, "only the live entry is kept:\n{text}");
    assert!(reopened.record(ISSUER, [1; 16], later(&manual, 1_000)).await.is_ok(), "expired, so free");
    assert_eq!(reopened.record(ISSUER, [2; 16], later(&manual, 1_000)).await, Err(Refusal::Replay));
}

/// One batch is one sync: a burst of records costs one fsync, and a nonce twice in one batch is a
/// replay the second time.
#[tokio::test]
async fn a_batch_is_one_sync() {
    let manual = ManualClock::new();
    let dir = tempfile::tempdir().expect("a temp dir");
    let mut store = Store::open(dir.path(), manual.clock()).expect("opens");
    let until = later(&manual, 330_000);
    let (a, ra) = job(ISSUER, 1, until);
    let (b, rb) = job(ISSUER, 2, until);
    let (c, rc) = job(ISSUER, 1, until);
    store.commit(vec![a, b, c]);
    assert_eq!(store.syncs, 1);
    assert_eq!(ra.await.expect("answered"), Ok(()));
    assert_eq!(rb.await.expect("answered"), Ok(()));
    assert_eq!(rc.await.expect("answered"), Err(Refusal::Replay));

    // A batch of nothing but replays writes nothing and syncs nothing.
    let (d, rd) = job(ISSUER, 2, until);
    store.commit(vec![d]);
    assert_eq!(store.syncs, 1);
    assert_eq!(rd.await.expect("answered"), Err(Refusal::Replay));
}

/// A torn last line (a write whose sync never finished, so never answered) is dropped at open; the
/// lines before it are kept.
#[tokio::test]
async fn a_torn_last_line_is_dropped_and_the_rest_kept() {
    let manual = ManualClock::new();
    let dir = tempfile::tempdir().expect("a temp dir");
    let nonces = FileNonces::open(dir.path(), manual.clock()).expect("opens");
    nonces.record(ISSUER, [1; 16], later(&manual, 330_000)).await.expect("recorded");
    drop(nonces);
    let path = dir.path().join(FILE);
    let mut text = std::fs::read_to_string(&path).expect("reads");
    text.push_str("3031737562 0202");
    std::fs::write(&path, text).expect("writes");

    let reopened = FileNonces::open(dir.path(), manual.clock()).expect("reopens");
    assert_eq!(reopened.record(ISSUER, [1; 16], later(&manual, 1)).await, Err(Refusal::Replay));
    assert!(reopened.record(ISSUER, [2; 16], later(&manual, 1)).await.is_ok());
}

const CHILD: &str = "ANEMNEZ_NONCES_CHILD_DIR";

/// a-nonce-is-durable-before-the-method-runs, under a crash (D): a child process records a nonce, is
/// answered, says so, and aborts at once (no destructors, nothing flushed but what was synced). The
/// parent reopens the directory and the nonce is still a replay.
#[test]
fn an_acknowledged_nonce_survives_an_abort_and_a_reopen() {
    let manual = ManualClock::new();
    let until = later(&manual, 330_000);
    if let Ok(dir) = std::env::var(CHILD) {
        // The child: a test harness's own entry point, so it may build a runtime.
        let runtime = tokio::runtime::Builder::new_current_thread().build().expect("a runtime");
        runtime.block_on(async {
            let nonces = FileNonces::open(Path::new(&dir), manual.clock()).expect("opens");
            nonces.record(ISSUER, [9; 16], until).await.expect("recorded");
        });
        println!("acked");
        std::process::abort();
    }

    let dir = tempfile::tempdir().expect("a temp dir");
    let me = std::env::current_exe().expect("the test binary");
    let child = std::process::Command::new(me)
        .args(["--exact", "edge_tests::an_acknowledged_nonce_survives_an_abort_and_a_reopen", "--nocapture"])
        .env(CHILD, dir.path())
        .output()
        .expect("the child runs");
    let said = String::from_utf8_lossy(&child.stdout);
    assert!(said.contains("acked"), "the child was answered first:\n{said}");
    assert!(!child.status.success(), "the child aborted");

    let runtime = tokio::runtime::Builder::new_current_thread().build().expect("a runtime");
    let replay = runtime.block_on(async {
        let nonces = FileNonces::open(dir.path(), manual.clock()).expect("reopens");
        nonces.record(ISSUER, [9; 16], until).await
    });
    assert_eq!(replay, Err(Refusal::Replay), "the acknowledged nonce survived the crash");
}

/// full_sync flushes a file and a directory on this platform (F_FULLFSYNC on macOS), falling back
/// only where the filesystem has no full flush.
#[test]
fn full_sync_flushes_a_file_and_a_directory() {
    use std::io::Write;
    let dir = tempfile::tempdir().expect("a temp dir");
    let mut file = std::fs::File::create(dir.path().join("x")).expect("creates");
    file.write_all(b"durable").expect("writes");
    crate::edge::full_sync(&file).expect("the file flushes");
    crate::edge::full_sync(&std::fs::File::open(dir.path()).expect("opens the dir")).expect("the dir flushes");
}

/// G m/9 direct, rule 8: a file written by a newer schema is refused by name at open, never read as this one's (its
/// lines might mean something else). Schema 1, said or not, opens: no header reads as schema 1.
#[tokio::test]
async fn a_newer_schema_is_refused_by_name_and_schema_1_opens_said_or_not() {
    let manual = ManualClock::new();
    let dir = tempfile::tempdir().expect("a temp dir");
    let path = dir.path().join(FILE);
    let entry = format!("{} {} {}\n", "3031737562", "01".repeat(16), later(&manual, 330_000));

    std::fs::write(&path, format!("{HEADER} 2\n{entry}")).expect("writes");
    let newer = FileNonces::open(dir.path(), manual.clock()).err().expect("a newer schema is refused");
    assert!(newer.to_string().starts_with(NEWER), "{newer}");
    assert_eq!(std::fs::read_to_string(&path).unwrap(), format!("{HEADER} 2\n{entry}"), "and left as it was");

    for text in [format!("{HEADER} 1\n{entry}"), entry.clone()] {
        std::fs::write(&path, &text).expect("writes");
        let opened = FileNonces::open(dir.path(), manual.clock()).expect("schema 1 opens");
        assert_eq!(opened.record("01sub", [1; 16], later(&manual, 1)).await, Err(Refusal::Replay), "{text}");
    }
}

/// Rule 8 and rule 5: a line that isn't an entry anywhere but last is damage, not a torn write (a torn write is only
/// ever the last line: open compacts it away before anything appends). Skipping it would forget nonces in silence, so
/// open refuses by name and leaves the file for a person to read.
#[tokio::test]
async fn a_bad_line_before_the_last_is_refused_by_name() {
    let manual = ManualClock::new();
    let dir = tempfile::tempdir().expect("a temp dir");
    let path = dir.path().join(FILE);
    let entry = format!("{} {} {}\n", "3031737562", "01".repeat(16), later(&manual, 330_000));
    std::fs::write(&path, format!("not an entry\n{entry}")).expect("writes");
    let damaged = FileNonces::open(dir.path(), manual.clock()).err().expect("refused");
    assert!(damaged.to_string().starts_with(UNREADABLE), "{damaged}");
}

/// A write that fails part-way leaves a fragment. Appending the next batch after it would glue the two into one bad
/// line and lose an acknowledged nonce at the next open. So a failed write marks the file dirty, and the next commit
/// rewrites it whole from what is held before it appends, refusing its batch if it can't.
#[tokio::test]
async fn after_a_failed_write_the_next_commit_rewrites_the_file_whole_first() {
    let manual = ManualClock::new();
    let dir = tempfile::tempdir().expect("a temp dir");
    let mut store = Store::open(dir.path(), manual.clock()).expect("opens");
    let until = later(&manual, 330_000);
    let (first, first_answer) = job(ISSUER, 1, until);
    store.commit(vec![first]);
    assert!(first_answer.await.unwrap().is_ok());
    // A fragment, as a write cut short leaves it, and the store told its write failed.
    let path = dir.path().join(FILE);
    let mut text = std::fs::read_to_string(&path).unwrap();
    text.push_str("3031737562 0202");
    std::fs::write(&path, text).unwrap();
    store.dirty = true;

    let (second, second_answer) = job(ISSUER, 3, until);
    store.commit(vec![second]);
    assert!(second_answer.await.unwrap().is_ok(), "answered once the file is whole again");
    drop(store);
    let reopened = FileNonces::open(dir.path(), manual.clock()).expect("reopens: no bad line inside");
    assert_eq!(reopened.record(ISSUER, [1; 16], until).await, Err(Refusal::Replay));
    assert_eq!(reopened.record(ISSUER, [3; 16], until).await, Err(Refusal::Replay), "the acknowledged nonce kept");
}
