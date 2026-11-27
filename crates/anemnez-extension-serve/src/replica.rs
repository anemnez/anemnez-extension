//! A log face read from any replica (ergon:M/log-face-any-replica; D m/42395, A m/42402). A replicated partition's face
//! (hydramq-events `PartitionEvents`) serves its committed prefix: never wrong, only behind. The face says which
//! replica answered, and how far it had committed, in [`anemnez_extension::READ_AT_METADATA`] (`<node>@<end>`), and a
//! read whose cursor came from a replica further on is refused `replica-behind` by name, as Unavailable, so a router
//! (the SDK's Reach) tries the next replica within the same bound.
//!
//! - **Nested bounds** (D's condition 1). A read's wait for its cursor is what the call's deadline has left less a hop
//!   ([`HOP_MARGIN`]), never the request's own constant, so `replica-behind` arrives inside the call, not as
//!   `events-call-unanswered`.
//! - **Seen, never parsed.** [`Reading`] wraps the face `wire::serve` and `wire::watch` read, and keeps which log a
//!   read named and whether the face refused it behind. The wire bytes stay opaque to this crate.
//! - **Declared SWARM.** A face that says an end ([`Face::end`]) is a replica's, so its cursors resume at any instance:
//!   a descriptor declaring `cursor_scope: INSTANCE` over one is refused [`REPLICA_NEEDS_SWARM_SCOPE`] by name.
//! - **Compaction is the replica's own** (D's condition 3): retention is by offset, so `Compacted` names this replica's
//!   start, which another replica's may differ from. **Latest** has no cursor: it is as of this replica's end, which
//!   the read-at header says.

use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anemnez_clock::{Clock, Mono};
use hydramq_events::wire::HOP_MARGIN;
use hydramq_events::{Event, EventsError, Face, Position, Read, Refused};

pub use anemnez_extension::refusal::REPLICA_NEEDS_SWARM_SCOPE;
pub use hydramq_events::REPLICA_BEHIND;

/// What a [`Reading`] saw of its reads.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Seen {
    /// The log the last read or latest named.
    pub log: Option<String>,
    /// The face's `replica-behind` refusal, in its words, when it refused one.
    pub behind: Option<String>,
}

/// The face one call or watch reads, bounded and seen: each read's wait ends a hop before `until`, and the log it named
/// and a `replica-behind` refusal are kept for the host to say.
pub(crate) struct Reading {
    until: Option<(Mono, Clock)>,
    seen: Mutex<Seen>,
    face: Arc<dyn Face>,
}

impl Reading {
    /// `face`, each read's wait ending a hop before `until` on `clock`; none for a watch, whose beat bounds each wait.
    pub fn new(face: Arc<dyn Face>, until: Option<(Mono, Clock)>) -> Reading {
        Reading { until, seen: Mutex::default(), face }
    }

    /// What was seen so far.
    pub fn seen(&self) -> Seen {
        self.seen.lock().unwrap_or_else(std::sync::PoisonError::into_inner).clone()
    }

    fn named(&self, log: &str) {
        self.seen.lock().unwrap_or_else(std::sync::PoisonError::into_inner).log = Some(log.to_owned());
    }

    fn judged<T>(&self, read: Result<T, EventsError>) -> Result<T, EventsError> {
        if let Err(EventsError::Refused(Refused::ByProducer { why })) = &read
            && hydramq_events::replica_behind(why).is_some()
        {
            self.seen.lock().unwrap_or_else(std::sync::PoisonError::into_inner).behind = Some(why.clone());
        }
        read
    }

    /// `within`, cut to end a hop before `until`.
    fn bounded(&self, within: Duration) -> Duration {
        match &self.until {
            Some((until, clock)) => within.min(until.saturating_since(clock.now_mono()).saturating_sub(HOP_MARGIN)),
            None => within,
        }
    }
}

impl Face for Reading {
    fn name(&self) -> &str {
        self.face.name()
    }

    fn read(&self, from: &Position, max: usize) -> Result<Read, EventsError> {
        self.named(&from.log);
        self.judged(self.face.read(from, max))
    }

    fn read_until(
        &self,
        from: &Position,
        max: usize,
        within: Duration,
        stop: Option<&AtomicBool>,
    ) -> Result<Read, EventsError> {
        self.named(&from.log);
        self.judged(self.face.read_until(from, max, self.bounded(within), stop))
    }

    fn latest_all(&self, prefix: &str) -> Result<(Vec<Event>, Position), EventsError> {
        let latest = self.face.latest_all(prefix);
        if let Ok((_, as_of)) = &latest {
            self.named(&as_of.log);
        }
        latest
    }

    fn latest_in(&self, log: Option<&str>, prefix: &str) -> Result<(Vec<Event>, Position), EventsError> {
        let latest = self.face.latest_in(log, prefix);
        match (&latest, log) {
            (Ok((_, as_of)), _) => self.named(&as_of.log),
            (Err(_), Some(log)) => self.named(log),
            (Err(_), None) => {}
        }
        latest
    }

    fn end(&self) -> Option<u64> {
        self.face.end()
    }

    fn wake(&self) {
        self.face.wake();
    }
}

/// Where a face's reads are said from: its node, and the logs whose ends it reads.
#[derive(Clone)]
pub(crate) struct ReadAt {
    pub node: Option<String>,
    pub logs: crate::many::Logs,
}

impl ReadAt {
    /// The read-at value for a read of `log`, when this face says one: a node, and a replica's end for that log.
    pub fn value(&self, log: Option<&str>) -> Option<tonic::metadata::MetadataValue<tonic::metadata::Ascii>> {
        let node = self.node.as_deref()?;
        let end = self.logs.end_of(log?)?;
        read_at(node, end).parse().ok()
    }

    /// `metadata` with the read-at header for `log`, when there is one to say.
    pub fn say(&self, metadata: &mut tonic::metadata::MetadataMap, log: Option<&str>) {
        if let Some(at) = self.value(log) {
            metadata.insert(anemnez_extension::READ_AT_METADATA, at);
        }
    }

    /// A `replica-behind` refusal (`why`, the face's words, its name first), as Unavailable by name, naming where it
    /// was read in its words and its header.
    pub fn behind(&self, why: &str, log: Option<&str>) -> tonic::Status {
        let at = self.value(log);
        let named = at.as_ref().and_then(|a| a.to_str().ok()).map_or_else(String::new, |a| format!("; read at {a}"));
        let mut status = tonic::Status::unavailable(format!("{why}{named}; read it at another replica"));
        if let Some(at) = at {
            status.metadata_mut().insert(anemnez_extension::READ_AT_METADATA, at);
        }
        status
    }
}

/// The read-at value a replica's face says: `<node>@<end>`.
#[must_use]
pub fn read_at(node: &str, end: u64) -> String {
    format!("{node}@{end}")
}

#[cfg(test)]
#[path = "replica_tests.rs"]
mod tests;
