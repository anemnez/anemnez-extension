//! Who reads a product's log as a house reader, said at a bounded rate (E m/41723's nit on D m/40623 (4)): one line
//! per (identifier, method) when it first reads, then each minute's count, never a line per read. A reader such as
//! themisd reopens its Watch every token life and reads often, so a line per read would drown the log it is there
//! to keep visible. Pure: the face hands in `now` on its own clock.
//!
//! The first read of a pair is said at once. Reads inside the next [`SAID_EVERY`] are counted. The first read after it
//! says how many came in that window, and is the first read counted in the next (E m/41723: each read is counted once).
//! A pair that stops reading leaves its last window's count unsaid until its next read.
//! The pairs are bounded by what reaches here: a house-signed identifier (one per house product) times the log's
//! four faces.

use std::collections::HashMap;
use std::time::Duration;

use anemnez_clock::Mono;

/// How often a pair's reads are said, at most.
pub const SAID_EVERY: Duration = Duration::from_secs(60);

/// What one read asks to be said.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Said {
    /// The pair's first read: say it.
    First,
    /// The first read past a window: `reads` came in that window, over `over`. This read opens the next window.
    Counted { reads: u64, over: Duration },
    /// Inside a window: counted, said later.
    Quiet,
}

/// Each pair's open window: when it began, and the reads it has counted.
#[derive(Debug, Default)]
pub struct Tally(HashMap<(String, String), (Mono, u64)>);

impl Tally {
    /// One authorized read by `identifier` of `method`, at `now`.
    pub fn read(&mut self, identifier: &str, method: &str, now: Mono) -> Said {
        match self.0.get_mut(&(identifier.to_owned(), method.to_owned())) {
            None => {
                self.0.insert((identifier.to_owned(), method.to_owned()), (now, 0));
                Said::First
            }
            Some((since, reads)) if now.saturating_since(*since) >= SAID_EVERY => {
                let said = Said::Counted { reads: *reads, over: now.saturating_since(*since) };
                (*since, *reads) = (now, 1);
                said
            }
            Some((_, reads)) => {
                *reads += 1;
                Said::Quiet
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// E m/41723: the first read is said, those inside the minute are only counted, and the first past it says the
    /// window's count and is counted in the next, so each read is counted exactly once. Each pair keeps its own window.
    /// Red with a line per read, and with the crossing read counted in neither window.
    #[test]
    fn a_house_readers_reads_are_said_once_then_counted_each_minute() {
        let manual = anemnez_clock::ManualClock::new();
        let at = |secs: u64| manual.clock().now_mono() + Duration::from_secs(secs);
        let mut tally = Tally::default();
        assert_eq!(tally.read("ai.anemnez.themisd", "Watch", at(0)), Said::First);
        for s in [1, 10, 59] {
            assert_eq!(tally.read("ai.anemnez.themisd", "Watch", at(s)), Said::Quiet, "{s}s: counted");
        }
        assert_eq!(tally.read("ai.anemnez.themisd", "Call", at(30)), Said::First, "another method is its own pair");
        assert_eq!(tally.read("ai.anemnez.ergond", "Watch", at(30)), Said::First, "another reader too");
        let said = tally.read("ai.anemnez.themisd", "Watch", at(60));
        assert_eq!(said, Said::Counted { reads: 3, over: Duration::from_secs(60) }, "the minute's count");
        assert_eq!(tally.read("ai.anemnez.themisd", "Watch", at(61)), Said::Quiet, "the next window began");
        let late = tally.read("ai.anemnez.themisd", "Watch", at(3600));
        let counted = Said::Counted { reads: 2, over: Duration::from_secs(3540) };
        assert_eq!(late, counted, "the reads at 60 s and 61 s, each counted once, said by a late read");
    }
}
