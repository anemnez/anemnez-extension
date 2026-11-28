//! One Events face over several named logs (L m/32091, D m/32100): a product whose facts live in more than one
//! hydramq-events log (Horai's owed, forecast and ship streams; Ergon's books; Stoa's channels) serves them all through
//! the one face, never a router of its own (rule 2).
//!
//! [`ManyLogs`] is a hydramq-events [`Face`] that routes each read by the log its request's `Position` names, so
//! `wire::serve` (Call) and `wire::watch` (Watch) take it unchanged, as they take one log. A request never opens a log
//! it doesn't name. A log the face doesn't serve is `ForeignLog`, naming every log it does. `Latest` is answered from
//! the log its request names (hydramq's `latest_in`, a client's `latest_all_in(log, prefix)`; K m/33557 item 4, L
//! m/34051), and one naming no log is refused by name: it is never routed by a prefix's spelling (D m/32100).

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use anemnez_clock::Clock;
use hydramq_events::{Event, EventsError, Face, Position, Read, Refused};

use crate::log_events::Log;
use crate::reread::ReRead;

/// The refusal of a many-log face's `Latest` that names no log: a client names it (`latest_all_in(log, prefix)`).
pub const LATEST_NEEDS_ITS_LOG: &str = "latest-needs-its-log";

/// A family of logs served by one prefix (ergon:M/history-log-family; O's ask, M's shape): logs a product opens while it
/// serves, such as a book per owner (`book/<owner>`), each read as a face of its own. It is asked per request, by the
/// log the request names, and is pure: it hands back the face it holds for that name, or none, and opens nothing.
pub trait Family: Send + Sync + 'static {
    /// The prefix every log of the family's name starts with (`book/`), never empty.
    fn prefix(&self) -> &str;

    /// The face of the log named `log` (which starts with [`Family::prefix`]), when the product holds one now.
    fn face(&self, log: &str) -> Option<Arc<dyn Face>>;

    /// Wakes every read waiting on one of the family's logs, as [`Face::wake`] does for one. Required, so a family says
    /// how its logs wake (M's should m/48057): a forgotten wake would leave a Watch on one of them unwoken until its own
    /// bound, a stall with no error.
    fn wake(&self);

    /// The names of the logs the family holds now, read at one moment (D's should m/48063): what [`Logs::replicas`]
    /// asks, so a family's log read from a replica (a book on a follower) is said as one. A family's face says its own
    /// end per request, as any face does.
    fn held(&self) -> Vec<String>;
}

/// What a [`crate::LogEvents`] reads: one log, or several by name and by family.
#[derive(Clone)]
pub(crate) enum Logs {
    One(Arc<dyn Log>),
    Many { named: Vec<(String, Arc<dyn Log>)>, families: Vec<Arc<dyn Family>> },
}

/// The family that serves `log`, by the longest prefix it starts with: the one resolution every read takes, after an
/// exact name ([`ManyLogs`]' route, [`Logs::serves`]).
fn family_of<'a>(families: &'a [Arc<dyn Family>], log: &str) -> Option<&'a Arc<dyn Family>> {
    families.iter().filter(|f| log.starts_with(f.prefix())).max_by_key(|f| f.prefix().len())
}

/// The names a many-log face says it serves: each log's, then each family's as `<prefix>*`.
fn names_of<'a>(named: impl Iterator<Item = &'a str>, families: &[Arc<dyn Family>]) -> String {
    named.map(str::to_owned).chain(families.iter().map(|f| format!("{}*", f.prefix()))).collect::<Vec<_>>().join(", ")
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

    /// Why a read found a log it couldn't read at all, if one did.
    pub fn unreadable(&self) -> Option<String> {
        self.rereads.iter().find_map(|r| r.unreadable())
    }

    /// Whether every log opened per read has a source of changes: without one, a wait wakes only at its bound.
    pub fn sourced(&self) -> bool {
        self.rereads.iter().all(|r| r.sourced())
    }
}

impl Logs {
    /// The face for one call or watch: a held log or a [`crate::FaceLog`] as it is, one opened per read through a
    /// [`ReRead`], several behind a [`ManyLogs`].
    pub fn faced(&self, open_within: Duration, clock: &Clock) -> Faced {
        let mut rereads = Vec::new();
        let mut face_of = |log: &Arc<dyn Log>| -> Arc<dyn Face> {
            match log.face() {
                Some(face) => face,
                None => {
                    let reread = Arc::new(ReRead::new(Arc::clone(log), open_within, clock));
                    rereads.push(Arc::clone(&reread));
                    reread
                }
            }
        };
        let face = match self {
            Logs::One(log) => face_of(log),
            Logs::Many { named, families } => {
                let faces = named.iter().map(|(name, log)| (name.clone(), face_of(log))).collect();
                Arc::new(ManyLogs::with_families(faces, families.clone()))
            }
        };
        Faced { face, rereads }
    }

    /// The committed end of the log named `log`, when its face says one (a replica's): on a one-log face, that log's
    /// whatever the name.
    pub fn end_of(&self, log: &str) -> Option<u64> {
        let named = match self {
            Logs::One(one) => Some(one),
            Logs::Many { named, families } => match named.iter().find(|(name, _)| name == log) {
                Some((_, one)) => Some(one),
                None => return family_of(families, log)?.face(log)?.end(),
            },
        };
        named?.face()?.end()
    }

    /// Whether this face serves the log named `log` now: its one log, a log of that name, or a family's that holds it.
    pub fn serves(&self, log: &str) -> bool {
        match self {
            Logs::One(one) => one.name() == log,
            Logs::Many { named, families } => {
                named.iter().any(|(name, _)| name == log)
                    || family_of(families, log).is_some_and(|f| f.face(log).is_some())
            }
        }
    }

    /// The logs read from a replica: those whose face says a committed end.
    pub fn replicas(&self) -> Vec<String> {
        let is_replica = |log: &Arc<dyn Log>| log.face().is_some_and(|face| face.end().is_some());
        match self {
            Logs::One(one) => is_replica(one).then(|| one.name().to_owned()).into_iter().collect(),
            Logs::Many { named, families } => {
                let fixed = named.iter().filter(|(_, log)| is_replica(log)).map(|(name, _)| name.clone());
                let held = families.iter().flat_map(|f| {
                    f.held().into_iter().filter(|name| f.face(name).is_some_and(|face| face.end().is_some()))
                });
                fixed.chain(held).collect()
            }
        }
    }

    /// The logs' names, as a log line says them.
    pub fn names(&self) -> String {
        match self {
            Logs::One(log) => log.name().to_owned(),
            Logs::Many { named, families } => names_of(named.iter().map(|(n, _)| n.as_str()), families),
        }
    }
}

/// Several named logs as one [`Face`], each read routed by the log its position names.
pub struct ManyLogs {
    faces: BTreeMap<String, Arc<dyn Face>>,
    families: Vec<Arc<dyn Family>>,
    names: String,
}

impl ManyLogs {
    /// The faces, by their log's name.
    #[must_use]
    pub fn new(faces: BTreeMap<String, Arc<dyn Face>>) -> ManyLogs {
        ManyLogs::with_families(faces, Vec::new())
    }

    /// The faces, by their log's name, and the families whose logs are resolved per request.
    #[must_use]
    pub fn with_families(faces: BTreeMap<String, Arc<dyn Face>>, families: Vec<Arc<dyn Family>>) -> ManyLogs {
        let names = names_of(faces.keys().map(String::as_str), &families);
        ManyLogs { faces, families, names }
    }

    /// The face of the log named `log`: an exact name first, then the family of the longest prefix it starts with;
    /// one neither holds is `ForeignLog`, naming every log and family served.
    fn route(&self, log: &str) -> Result<Arc<dyn Face>, EventsError> {
        let foreign = || EventsError::Refused(Refused::ForeignLog { asked: log.to_owned(), ours: self.names.clone() });
        match self.faces.get(log) {
            Some(face) => Ok(Arc::clone(face)),
            None => family_of(&self.families, log).and_then(|f| f.face(log)).ok_or_else(foreign),
        }
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
                 them; name the one you want (latest_all_in(log, prefix))",
                self.names
            ),
        }))
    }

    /// Latest from the log the request names, that log's own answer, never another's; a log this face doesn't serve is
    /// `ForeignLog`, and naming none is [`LATEST_NEEDS_ITS_LOG`].
    fn latest_in(&self, log: Option<&str>, prefix: &str) -> Result<(Vec<Event>, Position), EventsError> {
        match log {
            Some(named) => self.route(named)?.latest_in(Some(named), prefix),
            None => self.latest_all(prefix),
        }
    }

    fn wake(&self) {
        for face in self.faces.values() {
            face.wake();
        }
        for family in &self.families {
            family.wake();
        }
    }
}

#[cfg(test)]
#[path = "many_tests.rs"]
mod tests;
