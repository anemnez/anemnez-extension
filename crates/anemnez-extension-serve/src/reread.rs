//! A log opened for each read, served as a hydramq-events [`Face`] (L m/31024, D m/31023): so Events/Call's long-poll
//! and Events/Watch go through the one wire path every log takes, never a second Watch, and never with the log's lock
//! held while a caller waits.
//!
//! [`ReRead`] reads once inside the log's [`Log::with`]. Finding nothing past `from`, it waits outside the lock for
//! the log's [`Changes`] to move (its owner bumps them, horaid from its kqueue on the stream's file), or the bound
//! alone when the log has no source of changes, then reads again: until events come, the wait is stopped, or its
//! bound passes.
//!
//! **No lost wake** (D m/31023): the generation is read *before* each read, and the wait is only while it still
//! equals that one, so a write landing between a read and its wait wakes it at once.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anemnez_clock::{Clock, Condvar};
use hydramq_events::{Event, Events, EventsError, Face, Position, Read};

use crate::log_events::{Busy, Log};

/// A log's changes as its owner sees them: a generation it bumps on every write it learns of, waited on through the
/// clock's own condvar, so a row drives it on a ManualClock.
pub struct Changes {
    generation: Mutex<u64>,
    moved: Condvar,
    clock: Clock,
}

impl Changes {
    /// No changes yet, waited on `clock`.
    #[must_use]
    pub fn new(clock: &Clock) -> Arc<Changes> {
        Arc::new(Changes { generation: Mutex::new(0), moved: Condvar::new(clock), clock: clock.clone() })
    }

    /// The log changed: every wait past an earlier generation wakes.
    pub fn bump(&self) {
        if let Ok(mut generation) = self.generation.lock() {
            *generation = generation.wrapping_add(1);
        }
        self.moved.notify_all();
    }

    /// The generation now.
    #[must_use]
    pub fn generation(&self) -> u64 {
        self.generation.lock().map_or(0, |g| *g)
    }

    /// Wake every wait so each looks at its stop flag, without saying the log changed.
    pub fn notify(&self) {
        self.moved.notify_all();
    }

    /// Bump these changes whenever the file at `path` may hold something new: written or extended, replaced (a
    /// rename over it, which it follows to the new file), deleted, or found again at a `beat` while it's absent (D
    /// m/32441). It stops within a beat of the [`crate::follow::Following`] being dropped, so the [`crate::Log`] that
    /// hands these out keeps it.
    ///
    /// # Errors
    /// [`crate::follow::FOLLOW_UNSUPPORTED`] where there's no file watch for this host (a Watch then wakes at its beat),
    /// or the watch couldn't start.
    pub fn follow(
        self: &Arc<Self>,
        path: &std::path::Path,
        beat: Duration,
    ) -> Result<crate::follow::Following, String> {
        crate::follow::follow(Arc::clone(self), path, beat)
    }

    /// Wait at most `within` for the generation to move past `seen`, or `stop` to be set. Whether it moved.
    pub(crate) fn wait_past(&self, seen: u64, within: Duration, stop: Option<&AtomicBool>) -> bool {
        let stopped = || stop.is_some_and(|s| s.load(Ordering::SeqCst));
        let deadline = self.clock.now_mono() + within;
        let Ok(mut generation) = self.generation.lock() else { return false };
        while *generation == seen && !stopped() {
            let (guard, waited) = self.moved.wait_until(generation, deadline);
            generation = guard;
            if waited.timed_out() {
                break;
            }
        }
        *generation != seen
    }
}

/// A per-read [`Log`] as a [`Face`]: each read one [`Log::with`], each wait outside it. Made for one call or one
/// watch; [`ReRead::busy`] says whether a read found the log's writer holding it.
pub struct ReRead {
    log: Arc<dyn Log>,
    name: String,
    open_within: Duration,
    changes: Arc<Changes>,
    sourced: bool,
    busy: Mutex<Option<String>>,
}

impl ReRead {
    /// `log`, each read opening it within `open_within`, waiting on its [`Log::changes`], or on `clock` alone when it
    /// has none (the bound is then the only wake, said by [`ReRead::sourced`]).
    #[must_use]
    pub fn new(log: Arc<dyn Log>, open_within: Duration, clock: &Clock) -> ReRead {
        let (changes, sourced) = match log.changes() {
            Some(changes) => (changes, true),
            None => (Changes::new(clock), false),
        };
        let name = log.name().to_owned();
        ReRead { log, name, open_within, changes, sourced, busy: Mutex::new(None) }
    }

    /// Why a read found the log's writer holding it, if one did.
    #[must_use]
    pub fn busy(&self) -> Option<String> {
        self.busy.lock().ok().and_then(|b| b.clone())
    }

    /// Whether the log has a source of changes: without one, a wait wakes only at its bound.
    #[must_use]
    pub fn sourced(&self) -> bool {
        self.sourced
    }

    fn opened<T>(&self, read: impl FnOnce(&Events) -> Result<T, EventsError>) -> Result<T, EventsError> {
        let mut read = Some(read);
        let mut out = None;
        let opened = self.log.with(self.open_within, &mut |events| {
            if let Some(read) = read.take() {
                out = Some(read(events));
            }
        });
        match opened {
            Err(Busy(why)) => {
                let said =
                    format!("{}: the log's writer holds it ({why})", anemnez_extension::refusal::EVENTS_CALL_BUSY);
                if let Ok(mut busy) = self.busy.lock() {
                    *busy = Some(why);
                }
                Err(EventsError::Unclear(said))
            }
            Ok(()) => {
                out.unwrap_or_else(|| Err(EventsError::Unclear("the log was opened and its read never ran".into())))
            }
        }
    }
}

impl Face for ReRead {
    fn name(&self) -> &str {
        &self.name
    }

    fn read(&self, from: &Position, max: usize) -> Result<Read, EventsError> {
        self.opened(|events| Face::read(events, from, max))
    }

    fn read_until(
        &self,
        from: &Position,
        max: usize,
        within: Duration,
        stop: Option<&AtomicBool>,
    ) -> Result<Read, EventsError> {
        let stopped = || stop.is_some_and(|s| s.load(Ordering::SeqCst));
        let deadline = self.changes.clock.now_mono() + within;
        loop {
            // Before the read, never after it: a write between the read and the wait is then a moved generation.
            let seen = self.changes.generation();
            let read = self.read(from, max)?;
            let empty = matches!(&read, Read::Events { events, .. } if events.is_empty());
            let left = deadline.saturating_since(self.changes.clock.now_mono());
            if !empty || stopped() || left.is_zero() {
                return Ok(read);
            }
            if !self.changes.wait_past(seen, left, stop) && !stopped() {
                // The bound passed with no change: one last read, then answer it.
                return self.read(from, max);
            }
        }
    }

    fn latest_all(&self, prefix: &str) -> Result<(Vec<Event>, Position), EventsError> {
        self.opened(|events| Face::latest_all(events, prefix))
    }

    fn wake(&self) {
        self.changes.notify();
    }
}

#[cfg(test)]
#[path = "reread_tests.rs"]
mod tests;
