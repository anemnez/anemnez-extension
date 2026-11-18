//! The per-issuer key-record cache (PRINCIPAL_TOKEN §4; D m/4, E m/5 direct): a pure, sync machine the
//! [`Verifier`](crate::principal::Verifier) hosts, so a token is one swarm read per issuer per [`KEY_AGE`], never one
//! per call. It saves the read, never a check: every token is still checked against the record (`key_for`).
//!
//! Per issuer, what is known and what is in flight:
//!
//! | Known | Means | A lookup |
//! |---|---|---|
//! | none, or stale | nothing usable | starts a flight ([`Step::Fetch`]), or joins the one in flight ([`Step::Join`]) |
//! | `Held { record, read_at }`, fresh | read within [`KEY_AGE`] | uses it ([`Step::Use`]); a key id it lacks starts a flight at most once per [`REFETCH_EVERY`] |
//! | `Refused { why, read_at }`, fresh | `unknown-key` or `unattested` within [`REFUSED_AGE`] | is refused by the same name ([`Step::Refuse`]) |
//!
//! A flight has an id and a deadline ([`FETCH_WITHIN`]). Past it every waiter is Unclear and the flight is gone; an
//! answer naming a flight that is gone or replaced is dropped ([`Settled::Dropped`]), so a late answer never
//! overwrites a newer record or revives a cleared one (E). An Unclear answer is never cached, and a stale record is
//! never used: past [`KEY_AGE`] with its refetch Unclear, the call is Unclear (D: a revoke that can't be read is not a
//! key that's still good). Every age is on [`Host`], so a laptop asleep through a revoke doesn't wake trusting the
//! old record (rule 3).
//!
//! Bounds (bounds.state): held records at most [`HELD_AT_MOST`] and refusals at most [`REFUSED_AT_MOST`], each its own
//! least-recently-used pool, so a flood of made-up issuers evicts only refusals, never a real issuer's record (D);
//! flights at most [`FLIGHTS_AT_MOST`], past which a lookup is Unclear at once.

use std::collections::{BTreeMap, HashMap};
use std::time::Duration;

use anemnez_clock::Host;
use anemnez_extension::principal::{KEYS_WITHIN_MS, KeyRecord, Lookup, Refusal};

/// How long a held record is used before it is read again: the worst-case latency of a revoke (PRINCIPAL_TOKEN §4).
pub const KEY_AGE: Duration = Duration::from_secs(60);
/// How long an issuer's `unknown-key` or `unattested` is answered from the cache.
pub const REFUSED_AGE: Duration = Duration::from_secs(5);
/// How often a token naming a key id the held record lacks (a rotation) may read the issuer again.
pub const REFETCH_EVERY: Duration = Duration::from_secs(1);
/// A flight's deadline: the Verifier's own bound on a key lookup.
pub const FETCH_WITHIN: Duration = Duration::from_millis(KEYS_WITHIN_MS);
/// Held records kept, least recently used evicted first.
pub const HELD_AT_MOST: usize = 1024;
/// Refusals kept, in a pool of their own.
pub const REFUSED_AT_MOST: usize = 256;
/// Flights at once; past it a lookup is Unclear at once, never queued.
pub const FLIGHTS_AT_MOST: usize = 64;

/// A flight's id: [`KeyCache::fetched`] names it, so an answer for a flight that is gone is dropped.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Flight(u64);

/// What a lookup is to do.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Step {
    /// Check the token against this record.
    Use(KeyRecord),
    /// Refuse it by this name.
    Refuse(Refusal),
    /// Read the issuer's record, as this flight, and hand the answer to [`KeyCache::fetched`].
    Fetch(Flight),
    /// Wait for this flight's answer, bounded by its deadline.
    Join(Flight),
}

/// What a flight's answer did.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Settled {
    /// The flight's waiters take this answer.
    Answered(Result<KeyRecord, Refusal>),
    /// The flight was gone (past its deadline, cleared or replaced): the answer is dropped.
    Dropped,
}

#[derive(Clone, Debug)]
enum Known {
    Held { record: KeyRecord, read_at: Host },
    Refused { why: Refusal, read_at: Host },
}

#[derive(Clone, Copy, Debug)]
struct InFlight {
    flight: Flight,
    by: Host,
}

#[derive(Debug, Default)]
struct Entry {
    known: Option<Known>,
    flight: Option<InFlight>,
    /// When this issuer was last read, for the key-id bypass.
    last_read: Option<Host>,
    /// Its place in its pool's order.
    used: u64,
}

/// The cache: see the module's docs.
#[derive(Debug, Default)]
pub struct KeyCache {
    entries: HashMap<String, Entry>,
    /// Each pool's order, least recently used first: `used` to issuer.
    held: BTreeMap<u64, String>,
    refused: BTreeMap<u64, String>,
    flights: usize,
    next_flight: u64,
    tick: u64,
}

impl KeyCache {
    #[must_use]
    pub fn new() -> KeyCache {
        KeyCache::default()
    }

    /// What a token from `issuer` naming `key_id` is to do, at `now`.
    pub fn lookup(&mut self, issuer: &str, key_id: &str, now: Host) -> Step {
        self.expire(now);
        let entry = self.entries.entry(issuer.to_owned()).or_default();
        let fresh = match &entry.known {
            Some(Known::Held { record, read_at }) if now < *read_at + KEY_AGE => Some(Ok(record.clone())),
            Some(Known::Refused { why, read_at }) if now < *read_at + REFUSED_AGE => Some(Err(why.clone())),
            _ => None,
        };
        let step = match (fresh, entry.flight) {
            (Some(Ok(record)), flight) => {
                let has = record.keys.iter().any(|k| k.key_id == key_id);
                let may_refetch = entry.last_read.is_none_or(|last| now >= last + REFETCH_EVERY);
                match flight {
                    _ if has => Step::Use(record),
                    Some(f) => Step::Join(f.flight),
                    None if may_refetch => return self.start(issuer, now),
                    // A key id the record lacks, read again too lately: the check refuses it by the record.
                    None => Step::Use(record),
                }
            }
            (Some(Err(why)), _) => Step::Refuse(why),
            (None, Some(f)) => Step::Join(f.flight),
            (None, None) => return self.start(issuer, now),
        };
        self.touch(issuer);
        step
    }

    /// The answer to `flight`'s read of `issuer`, at `now`.
    pub fn fetched(&mut self, issuer: &str, flight: Flight, answer: Result<KeyRecord, Refusal>, now: Host) -> Settled {
        self.expire(now);
        let Some(entry) = self.entries.get_mut(issuer) else { return Settled::Dropped };
        if entry.flight.map(|f| f.flight) != Some(flight) {
            return Settled::Dropped;
        }
        entry.flight = None;
        self.flights = self.flights.saturating_sub(1);
        match &answer {
            Ok(record) => self.know(issuer, Known::Held { record: record.clone(), read_at: now }),
            Err(why @ (Refusal::UnknownKey | Refusal::Unattested)) => {
                self.know(issuer, Known::Refused { why: why.clone(), read_at: now });
            }
            // Unclear is never cached: what was known stays as it was, and a stale record stays unused.
            Err(_) => self.forget_if_bare(issuer),
        }
        Settled::Answered(answer)
    }

    /// Clear every flight past its deadline at `now`: each one's waiters are Unclear. Answers for them are dropped.
    pub fn expire(&mut self, now: Host) -> Vec<(String, Flight)> {
        let gone: Vec<(String, Flight)> = self
            .entries
            .iter()
            .filter_map(|(issuer, e)| e.flight.filter(|f| now >= f.by).map(|f| (issuer.clone(), f.flight)))
            .collect();
        for (issuer, _) in &gone {
            if let Some(entry) = self.entries.get_mut(issuer) {
                entry.flight = None;
                self.flights = self.flights.saturating_sub(1);
            }
            self.forget_if_bare(issuer);
        }
        gone
    }

    /// The next flight deadline, for the host's timer.
    #[must_use]
    pub fn next_deadline(&self) -> Option<Host> {
        self.entries.values().filter_map(|e| e.flight.map(|f| f.by)).min()
    }

    /// How many records are held, refusals kept, and flights in the air.
    #[must_use]
    pub fn counts(&self) -> (usize, usize, usize) {
        (self.held.len(), self.refused.len(), self.flights)
    }

    fn start(&mut self, issuer: &str, now: Host) -> Step {
        if self.flights >= FLIGHTS_AT_MOST {
            self.forget_if_bare(issuer);
            return Step::Refuse(Refusal::LookupUnanswered(Lookup::Keys));
        }
        self.next_flight += 1;
        let flight = Flight(self.next_flight);
        let entry = self.entries.entry(issuer.to_owned()).or_default();
        entry.flight = Some(InFlight { flight, by: now + FETCH_WITHIN });
        entry.last_read = Some(now);
        self.flights += 1;
        Step::Fetch(flight)
    }

    /// Put what `issuer` is known as in its pool, evicting that pool's least recently used past its cap.
    fn know(&mut self, issuer: &str, known: Known) {
        self.unpool(issuer);
        let held = matches!(known, Known::Held { .. });
        if let Some(entry) = self.entries.get_mut(issuer) {
            entry.known = Some(known);
        }
        self.tick += 1;
        let tick = self.tick;
        if let Some(entry) = self.entries.get_mut(issuer) {
            entry.used = tick;
        }
        let (pool, cap) = if held { (&mut self.held, HELD_AT_MOST) } else { (&mut self.refused, REFUSED_AT_MOST) };
        pool.insert(tick, issuer.to_owned());
        while pool.len() > cap {
            let Some((_, evicted)) = pool.pop_first() else { break };
            if let Some(entry) = self.entries.get_mut(&evicted) {
                entry.known = None;
            }
            if self.entries.get(&evicted).is_some_and(|e| e.flight.is_none()) {
                self.entries.remove(&evicted);
            }
        }
    }

    fn touch(&mut self, issuer: &str) {
        let Some(entry) = self.entries.get(issuer) else { return };
        let Some(known) = &entry.known else { return };
        let held = matches!(known, Known::Held { .. });
        let old = entry.used;
        self.tick += 1;
        let tick = self.tick;
        let pool = if held { &mut self.held } else { &mut self.refused };
        if pool.remove(&old).is_some() {
            pool.insert(tick, issuer.to_owned());
            if let Some(entry) = self.entries.get_mut(issuer) {
                entry.used = tick;
            }
        }
    }

    fn unpool(&mut self, issuer: &str) {
        if let Some(entry) = self.entries.get(issuer) {
            self.held.remove(&entry.used);
            self.refused.remove(&entry.used);
        }
    }

    /// An entry with nothing known and nothing in flight goes, so lookups alone never grow the map.
    fn forget_if_bare(&mut self, issuer: &str) {
        if self.entries.get(issuer).is_some_and(|e| e.known.is_none() && e.flight.is_none()) {
            self.entries.remove(issuer);
        }
    }
}

#[cfg(test)]
#[path = "key_cache_tests.rs"]
mod tests;
