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
        std::thread::Builder::new()
            .name("nonces-writer".into())
            .spawn(move || {
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
        let job = Job {
            issuer: issuer.to_owned(),
            nonce,
            until_ms,
            answer,
        };
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
}

impl Store {
    pub(crate) fn open(dir: &Path, clock: Clock) -> io::Result<Store> {
        std::fs::create_dir_all(dir)?;
        let path = dir.join(FILE);
        let now = clock.now_wall().unix_millis();
        let mut held = HashMap::new();
        if let Ok(file) = File::open(&path) {
            for line in BufReader::new(file).lines() {
                // A torn last line is a record whose sync never finished, so it was never answered:
                // dropping it loses no acknowledged nonce.
                if let Some((key, until)) = line.ok().as_deref().and_then(parse)
                    && until >= now
                {
                    held.insert(key, until);
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
            let written = self
                .file
                .write_all(out.as_bytes())
                .and_then(|()| full_sync(&self.file));
            self.syncs += 1;
            self.lines += out.lines().count();
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
        let body: String = self
            .held
            .iter()
            .map(|(key, until)| line(key, *until))
            .collect();
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

/// Make a file's writes durable on the DRIVE, not just handed to it (D, 2026-11-07). On macOS
/// `fsync` (what `sync_data` does) leaves the data in the drive's own write cache, so an answered
/// nonce could be lost on power loss and its token replayed after a reboot. `F_FULLFSYNC` asks the
/// drive to flush, as J's raft WAL does; a filesystem that does not support it answers ENOTSUP or
/// EINVAL, and then `sync_data` is the best there is. With group commit this is one full flush per
/// batch. A process-abort test cannot see the difference (the page cache survives an abort); this
/// comment is the reason the call is what it is.
pub fn full_sync(file: &File) -> io::Result<()> {
    #[cfg(any(target_os = "macos", target_os = "ios"))]
    {
        use std::os::fd::AsRawFd;
        // SAFETY: fcntl with F_FULLFSYNC takes only the descriptor, which `file` owns and keeps open
        // for the duration of the call; it reads and writes no memory of ours.
        let done = unsafe { libc::fcntl(file.as_raw_fd(), libc::F_FULLFSYNC) };
        if done != -1 {
            return Ok(());
        }
        let error = io::Error::last_os_error();
        match error.raw_os_error() {
            Some(libc::ENOTSUP | libc::EINVAL) => {}
            _ => return Err(error),
        }
    }
    file.sync_data()
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
    (0..text.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(text.get(i..i + 2)?, 16).ok())
        .collect()
}

#[cfg(test)]
pub(crate) fn job(
    issuer: &str,
    nonce: u8,
    until_ms: i64,
) -> (Job, oneshot::Receiver<Result<(), Refusal>>) {
    let (answer, answered) = oneshot::channel();
    (
        Job {
            issuer: issuer.into(),
            nonce: [nonce; 16],
            until_ms,
            answer,
        },
        answered,
    )
}
