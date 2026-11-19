//! The crate's edge (substrate `docs/plans/ASYNCIFICATION.md` §4): the one place sync work meets the
//! async runtime. It holds [`FileNonces`], the durable nonce set every product needs for step 8 of
//! substrate `docs/specs/PRINCIPAL_TOKEN.md` §4 (K, D, 2026-11-07).
//!
//! A disk write is not a quick call, so it never runs on an async worker. ONE writer thread owns the
//! file, fed by a channel, and each caller awaits a oneshot: one thread regardless of load, in arrival
//! order (the graph edge's shape, ASYNCIFICATION A1b). The thread group-commits: it takes every record
//! already queued, appends them, makes them durable with one [`full_sync`], and only then answers each.
//! A burst of calls costs one fsync, and no call is answered before its nonce is on disk.

use std::collections::HashMap;
use std::fs::{File, OpenOptions};
use std::io::{self, BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::sync::mpsc;

use anemnez_clock::Clock;
use anemnez_extension::principal::Refusal;
use tokio::sync::oneshot;

use crate::principal::Nonces;

/// The file a [`FileNonces`] keeps under its directory.
pub const FILE: &str = "nonces.log";

/// The file's schema line, `anemnez-nonces <N>`, first when present (HOUSE rule 8, G m/9 direct). A file without one
/// reads as schema 1. This build reads schema 1 and doesn't write the line yet: it is written once every reader in
/// service (and its rollback) reads it, the upgrade-compat two-step.
pub const HEADER: &str = "anemnez-nonces";
/// The schema this build reads.
pub const SCHEMA: u32 = 1;
/// Open's refusal of a file written by a newer schema: never read as this one's.
pub const NEWER: &str = "nonces-file-newer";
/// Open's refusal of a file with a line that isn't an entry anywhere but last: damage, never skipped in silence.
pub const UNREADABLE: &str = "nonces-file-unreadable";

/// The durable nonce set: an append-only file under a product's own data directory, loaded at open,
/// pruned of what has expired, and compacted once dead entries dominate.
pub struct FileNonces {
    jobs: mpsc::Sender<Job>,
}

pub(crate) struct Job {
    issuer: String,
    nonce: [u8; 16],
    until_ms: i64,
    answer: oneshot::Sender<Result<(), Refusal>>,
}

impl FileNonces {
    /// Open (or create) the set in `dir`, keeping every entry not yet past its `until_ms` on `clock`,
    /// and start its writer thread. The directory is the product's own, one store per instance.
    ///
    /// # Errors
    /// The directory or file cannot be created, read or written.
    pub fn open(dir: &Path, clock: Clock) -> io::Result<FileNonces> {
        let mut store = Store::open(dir, clock)?;
        let (jobs, queue) = mpsc::channel::<Job>();
        std::thread::Builder::new().name("nonces-writer".into()).spawn(move || {
            // Ends when the last sender goes, that is, when the FileNonces is dropped.
            while let Ok(first) = queue.recv() {
                let mut batch = vec![first];
                batch.extend(queue.try_iter());
                store.commit(batch);
            }
        })?;
        Ok(FileNonces { jobs })
    }
}

#[tonic::async_trait]
impl Nonces for FileNonces {
    async fn record(&self, issuer: &str, nonce: [u8; 16], until_ms: i64) -> Result<(), Refusal> {
        let (answer, answered) = oneshot::channel();
        let job = Job { issuer: issuer.to_owned(), nonce, until_ms, answer };
        // A writer that has gone cannot record: refuse, never admit an unrecorded nonce.
        self.jobs.send(job).map_err(|_| Refusal::Replay)?;
        answered.await.unwrap_or(Err(Refusal::Replay))
    }
}

/// The writer thread's state: the file and what it holds.
pub(crate) struct Store {
    path: PathBuf,
    file: File,
    clock: Clock,
    held: HashMap<(String, [u8; 16]), i64>,
    /// Lines in the file, live or dead: compaction runs when they outnumber the live entries enough.
    lines: usize,
    /// How many [`full_sync`] calls it has made, for the group-commit proof.
    pub(crate) syncs: u64,
    /// A write failed, so the file may end in a fragment: the next commit rewrites it whole before it appends.
    pub(crate) dirty: bool,
}

impl Store {
    pub(crate) fn open(dir: &Path, clock: Clock) -> io::Result<Store> {
        std::fs::create_dir_all(dir)?;
        let path = dir.join(FILE);
        let now = clock.now_wall().unix_millis();
        let mut held = HashMap::new();
        if let Ok(file) = File::open(&path) {
            let lines: Vec<Option<String>> = BufReader::new(file).lines().map(Result::ok).collect();
            let last = lines.len().saturating_sub(1);
            for (n, line) in lines.iter().enumerate() {
                if n == 0
                    && let Some(said) = line.as_deref().and_then(|l| l.strip_prefix(HEADER)?.strip_prefix(' '))
                {
                    schema(&path, said)?;
                    continue;
                }
                match line.as_deref().and_then(parse) {
                    Some((key, until)) => {
                        if until >= now {
                            held.insert(key, until);
                        }
                    }
                    // A torn last line is a record whose sync never finished, so it was never answered:
                    // dropping it loses no acknowledged nonce. Open compacts it away before anything appends.
                    None if n == last => {}
                    // Anywhere else it is damage: skipping it would forget nonces in silence.
                    None => {
                        return Err(io::Error::new(
                            io::ErrorKind::InvalidData,
                            format!("{UNREADABLE}: {} line {} is not an entry; left as it is", path.display(), n + 1),
                        ));
                    }
                }
            }
        }
        let mut store = Store {
            file: OpenOptions::new().create(true).append(true).open(&path)?,
            path,
            clock,
            lines: 0,
            held,
            syncs: 0,
            dirty: false,
        };
        store.compact()?;
        Ok(store)
    }

    /// One group commit: refuse what is held, append the rest, one sync, then answer every job.
    pub(crate) fn commit(&mut self, batch: Vec<Job>) {
        let now = self.clock.now_wall().unix_millis();
        let mut out = String::new();
        let mut answers = Vec::with_capacity(batch.len());
        for job in batch {
            let key = (job.issuer, job.nonce);
            let replay = self.held.get(&key).is_some_and(|until| *until >= now);
            if replay {
                answers.push((job.answer, Err(Refusal::Replay)));
                continue;
            }
            out.push_str(&line(&key, job.until_ms));
            self.held.insert(key, job.until_ms);
            answers.push((job.answer, Ok(())));
        }
        if !out.is_empty() {
            // A fragment left by a failed write is never appended after: the file is first rewritten whole from
            // what is held (which already carries this batch, so a rewrite makes it durable too).
            let written = if self.dirty {
                self.compact()
            } else {
                let appended = self.file.write_all(out.as_bytes()).and_then(|()| full_sync(&self.file));
                self.lines += out.lines().count();
                appended
            };
            self.syncs += 1;
            self.dirty = written.is_err();
            if written.is_err() {
                // Not durable, so not accepted: refuse every record in this batch (fail closed).
                for answer in &mut answers {
                    if answer.1.is_ok() {
                        answer.1 = Err(Refusal::Replay);
                    }
                }
            }
        }
        for (answer, result) in answers {
            let _ = answer.send(result);
        }
        if self.lines > 2 * self.held.len() + 1024 {
            let _ = self.compact();
        }
    }

    /// Drop what has expired and rewrite the file with only the live entries, durably: a temp file,
    /// synced, then an atomic rename and a sync of the directory.
    fn compact(&mut self) -> io::Result<()> {
        let now = self.clock.now_wall().unix_millis();
        self.held.retain(|_, until| *until >= now);
        let tmp = self.path.with_extension("log.tmp");
        let mut out = File::create(&tmp)?;
        let body: String = self.held.iter().map(|(key, until)| line(key, *until)).collect();
        out.write_all(body.as_bytes())?;
        full_sync(&out)?;
        std::fs::rename(&tmp, &self.path)?;
        if let Some(dir) = self.path.parent() {
            full_sync(&File::open(dir)?)?;
        }
        self.file = OpenOptions::new().append(true).open(&self.path)?;
        self.lines = self.held.len();
        Ok(())
    }
}

/// Make a file's writes durable on the drive. On macOS a plain `fsync` leaves data in the drive's
/// own write cache, so an answered nonce could be lost on power loss and its token replayed after a
/// reboot. Rust std's `sync_data` (and `sync_all`) on Apple platforms already calls
/// `fcntl(F_FULLFSYNC)`, which asks the drive to flush (J measured it on MB16, 2026-11-07: about 5 ms,
/// against 19 µs for a plain fsync). So this IS `sync_data`, by name, so nobody swaps in a raw
/// `libc`/`nix` fsync, which would not flush the drive. With group commit it is one flush per batch.
pub fn full_sync(file: &File) -> io::Result<()> {
    file.sync_data()
}

/// Schema `said` in `path`'s first line: this build's opens; a newer one is refused by name, and anything else is
/// damage.
fn schema(path: &Path, said: &str) -> io::Result<()> {
    match said.parse::<u32>() {
        Ok(SCHEMA) => Ok(()),
        Ok(newer) if newer > SCHEMA => Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("{NEWER}: {} is schema {newer}; this build reads {SCHEMA}; left as it is", path.display()),
        )),
        _ => Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("{UNREADABLE}: {} says schema {said:?}; left as it is", path.display()),
        )),
    }
}

/// One entry: `<issuer hex> <nonce hex> <until_ms>`. The issuer is hex so no character in it can
/// break a line.
fn line((issuer, nonce): &(String, [u8; 16]), until_ms: i64) -> String {
    format!("{} {} {until_ms}\n", hex(issuer.as_bytes()), hex(nonce))
}

fn parse(line: &str) -> Option<((String, [u8; 16]), i64)> {
    let mut fields = line.split(' ');
    let issuer = String::from_utf8(unhex(fields.next()?)?).ok()?;
    let nonce: [u8; 16] = unhex(fields.next()?)?.try_into().ok()?;
    let until = fields.next()?.parse().ok()?;
    if fields.next().is_some() {
        return None;
    }
    Some(((issuer, nonce), until))
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn unhex(text: &str) -> Option<Vec<u8>> {
    if !text.len().is_multiple_of(2) {
        return None;
    }
    (0..text.len()).step_by(2).map(|i| u8::from_str_radix(text.get(i..i + 2)?, 16).ok()).collect()
}

#[cfg(test)]
pub(crate) fn job(issuer: &str, nonce: u8, until_ms: i64) -> (Job, oneshot::Receiver<Result<(), Refusal>>) {
    let (answer, answered) = oneshot::channel();
    (Job { issuer: issuer.into(), nonce: [nonce; 16], until_ms, answer }, answered)
}
