//! One Events face over several named logs (L m/32091, D m/32100): a product whose facts live in more than one
//! hydramq-events log (Horai's owed, forecast and ship streams; Ergon's books; Stoa's channels) serves them all through
//! the one face, never a router of its own (rule 2).
//!
//! [`ManyLogs`] is a hydramq-events [`Face`] that routes each read by the log its request's `Position` names, so
//! `wire::serve` (Call) and `wire::watch` (Watch) take it unchanged, as they take one log. A request never opens a log
//! it doesn't name. A log the face doesn't serve is `ForeignLog`, naming every log it does. `Latest` is refused by
//! name until it can name its log on the wire (hydramq's latest_in, readers first): it is never routed by a prefix's
//! spelling (D m/32100).

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use anemnez_clock::Clock;
use hydramq_events::{Event, EventsError, Face, Position, Read, Refused};

use crate::log_events::Log;
use crate::reread::ReRead;

/// The refusal of a many-log face's `Latest`, until a request can name its log.
pub const LATEST_NEEDS_ITS_LOG: &str = "latest-needs-its-log";

/// What a [`crate::LogEvents`] reads: one log, or several by name.
#[derive(Clone)]
pub(crate) enum Logs {
    One(Arc<dyn Log>),
    Many(Vec<(String, Arc<dyn Log>)>),
}

/// The face one call or one watch reads, and the re-reads it opened, for their `Busy`.
pub(crate) struct Faced {
    pub face: Arc<dyn Face>,
    pub rereads: Vec<Arc<ReRead>>,
}

impl Faced {
    /// Why a read found a log's writer holding it, if one did.
    pub fn busy(&self) -> Option<String> {
        self.rereads.iter().find_map(|r| r.busy())
    }

    /// Whether every log opened per read has a source of changes: without one, a wait wakes only at its bound.
    pub fn sourced(&self) -> bool {
        self.rereads.iter().all(|r| r.sourced())
    }
}

impl Logs {
    /// The face for one call or watch: a held log as it is, one opened per read through a [`ReRead`], several behind
    /// a [`ManyLogs`].
    pub fn faced(&self, open_within: Duration, clock: &Clock) -> Faced {
        let mut rereads = Vec::new();
        let mut face_of = |log: &Arc<dyn Log>| -> Arc<dyn Face> {
            match log.held() {
                Some(events) => events,
                None => {
                    let reread = Arc::new(ReRead::new(Arc::clone(log), open_within, clock));
                    rereads.push(Arc::clone(&reread));
                    reread
                }
            }
        };
        let face = match self {
            Logs::One(log) => face_of(log),
            Logs::Many(logs) => {
                let faces = logs.iter().map(|(name, log)| (name.clone(), face_of(log))).collect();
                Arc::new(ManyLogs::new(faces))
            }
        };
        Faced { face, rereads }
    }

    /// The logs' names, as a log line says them.
    pub fn names(&self) -> String {
        match self {
            Logs::One(log) => log.name().to_owned(),
            Logs::Many(logs) => logs.iter().map(|(n, _)| n.as_str()).collect::<Vec<_>>().join(", "),
        }
    }
}

/// Several named logs as one [`Face`], each read routed by the log its position names.
pub struct ManyLogs {
    faces: BTreeMap<String, Arc<dyn Face>>,
    names: String,
}

impl ManyLogs {
    /// The faces, by their log's name.
    #[must_use]
    pub fn new(faces: BTreeMap<String, Arc<dyn Face>>) -> ManyLogs {
        let names = faces.keys().cloned().collect::<Vec<_>>().join(", ");
        ManyLogs { faces, names }
    }

    fn route(&self, log: &str) -> Result<&Arc<dyn Face>, EventsError> {
        self.faces.get(log).ok_or_else(|| {
            EventsError::Refused(Refused::ForeignLog { asked: log.to_owned(), ours: self.names.clone() })
        })
    }
}

impl Face for ManyLogs {
    fn name(&self) -> &str {
        &self.names
    }

    fn read(&self, from: &Position, max: usize) -> Result<Read, EventsError> {
        self.route(&from.log)?.read(from, max)
    }

    fn read_until(
        &self,
        from: &Position,
        max: usize,
        within: Duration,
        stop: Option<&std::sync::atomic::AtomicBool>,
    ) -> Result<Read, EventsError> {
        self.route(&from.log)?.read_until(from, max, within, stop)
    }

    fn latest_all(&self, prefix: &str) -> Result<(Vec<Event>, Position), EventsError> {
        Err(EventsError::Refused(Refused::ByProducer {
            why: format!(
                "{LATEST_NEEDS_ITS_LOG}: this face serves several logs ({}), and Latest for {prefix:?} names none of \
                 them; read by Call from a position in the log you want",
                self.names
            ),
        }))
    }

    fn wake(&self) {
        for face in self.faces.values() {
            face.wake();
        }
    }
}

#[cfg(test)]
#[path = "many_tests.rs"]
mod tests;
