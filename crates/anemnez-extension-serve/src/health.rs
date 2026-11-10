//! Health: state, with parts, and the events cursor it is true as of
//! (CONTRACT.md `#health-is-state-with-parts`).
//!
//! A [`Board`] holds the extension's health and publishes every change to its [`Journal`] as a
//! `health` event. `Get` answers the snapshot with the cursor it is true as of, read under the same
//! lock that orders the changes, so a watcher that reads `Get` and then subscribes after `as_of`
//! sees every change since and none from before.

use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use anemnez_extension::health::State;
use prost::Message;

use crate::journal::Journal;
use crate::pb;

/// The event kind a health change is published as.
pub const KIND: &str = "health";

/// An extension's health board. Cloning it shares the one board.
#[derive(Clone)]
pub struct Board {
    journal: Journal,
    reply: Arc<Mutex<pb::HealthReply>>,
}

impl Board {
    /// A board starting in `STARTING`, publishing to `journal`.
    pub fn new(journal: Journal) -> Board {
        let reply = pb::HealthReply {
            state: pb::State::Starting as i32,
            ..Default::default()
        };
        Board {
            journal,
            reply: Arc::new(Mutex::new(reply)),
        }
    }

    /// Set the extension's own state. Publishes only on a change, answering whether there was one,
    /// so a product may set its health as often as it likes without filling the journal.
    pub fn set(&self, state: State, detail: &str) -> bool {
        let state = proto_state(state) as i32;
        let mut reply = self.reply();
        if reply.state == state && reply.detail == detail {
            return false;
        }
        reply.state = state;
        detail.clone_into(&mut reply.detail);
        self.publish(&mut reply);
        true
    }

    /// Set one part's state, by path (`memory/index`). Its `since` moves only when its state does,
    /// and it publishes only on a change, answering whether there was one.
    pub fn set_part(&self, name: &str, state: State, detail: &str) -> bool {
        let now = self.journal.clock().now_wall().unix_millis();
        let mut reply = self.reply();
        if !upsert(
            &mut reply.parts,
            name,
            proto_state(state) as i32,
            detail,
            now,
        ) {
            return false;
        }
        self.publish(&mut reply);
        true
    }

    /// Remove a part (a stopped model's `runtimes/<m>`), publishing if it was there.
    pub fn remove_part(&self, name: &str) -> bool {
        let mut reply = self.reply();
        let before = reply.parts.len();
        reply.parts.retain(|p| p.name != name);
        if reply.parts.len() == before {
            return false;
        }
        self.publish(&mut reply);
        true
    }

    /// Make the parts exactly `parts`, for a product that recomputes them all on a period: parts not
    /// named are removed, and ONE event is published only if anything changed. Answers whether
    /// anything did.
    pub fn sync_parts<'a, I>(&self, parts: I) -> bool
    where
        I: IntoIterator<Item = (&'a str, State, &'a str)>,
    {
        let now = self.journal.clock().now_wall().unix_millis();
        let mut reply = self.reply();
        let mut changed = false;
        let mut named = Vec::new();
        for (name, state, detail) in parts {
            changed |= upsert(
                &mut reply.parts,
                name,
                proto_state(state) as i32,
                detail,
                now,
            );
            named.push(name);
        }
        let before = reply.parts.len();
        reply.parts.retain(|p| named.contains(&p.name.as_str()));
        changed |= reply.parts.len() != before;
        if changed {
            self.publish(&mut reply);
        }
        changed
    }

    /// Say what a restart would cost now (`None`: name the product only). Publishes only on a change,
    /// answering whether there was one, so a product may set it on every recompute.
    pub fn set_restart_cost(&self, cost: Option<&anemnez_extension::health::RestartCost>) -> bool {
        let want = cost.map(|c| pb::RestartCost {
            line: c.line.clone(),
            secs: c.secs,
        });
        let mut reply = self.reply();
        if reply.restart_cost == want {
            return false;
        }
        reply.restart_cost = want;
        self.publish(&mut reply);
        true
    }

    /// The snapshot, with the cursor it is true as of.
    pub fn get(&self) -> pb::HealthReply {
        let reply = self.reply();
        let mut answer = reply.clone();
        answer.as_of = self.journal.head();
        answer
    }

    /// Publish the board as it now stands. Called with the board's lock held, so changes reach the
    /// journal in the order they were made.
    fn publish(&self, reply: &mut pb::HealthReply) {
        let mut event = reply.clone();
        event.as_of.clear();
        reply.as_of = self.journal.publish(KIND, event.encode_to_vec());
    }

    fn reply(&self) -> MutexGuard<'_, pb::HealthReply> {
        // Every change is whole before the lock is released, so a poisoned lock still holds a
        // true board.
        self.reply.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

#[tonic::async_trait]
impl pb::health_server::Health for Board {
    async fn get(
        &self,
        _request: tonic::Request<pb::HealthRequest>,
    ) -> Result<tonic::Response<pb::HealthReply>, tonic::Status> {
        Ok(tonic::Response::new(Board::get(self)))
    }
}

/// Set or add one part, answering whether anything changed.
fn upsert(parts: &mut Vec<pb::Part>, name: &str, state: i32, detail: &str, now: i64) -> bool {
    match parts.iter_mut().find(|p| p.name == name) {
        Some(part) if part.state == state && part.detail == detail => false,
        Some(part) => {
            if part.state != state {
                part.state = state;
                part.since = now;
            }
            detail.clone_into(&mut part.detail);
            true
        }
        None => {
            parts.push(pb::Part {
                name: name.to_owned(),
                state,
                detail: detail.to_owned(),
                since: now,
            });
            true
        }
    }
}

/// The proto's state for the kernel's.
pub fn proto_state(state: State) -> pb::State {
    match state {
        State::Unknown => pb::State::Unknown,
        State::Starting => pb::State::Starting,
        State::Serving => pb::State::Serving,
        State::Degraded => pb::State::Degraded,
        State::Failing => pb::State::Failing,
        State::Draining => pb::State::Draining,
        State::Drained => pb::State::Drained,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use anemnez_clock::ManualClock;

    fn board() -> (Board, Journal) {
        let journal = Journal::new("a1b2c3d4.boot1", 64, ManualClock::new().clock());
        (Board::new(journal.clone()), journal)
    }

    /// A product that sets its health on a period publishes only what changed, so the journal holds
    /// changes, not the period (K).
    #[test]
    fn setting_the_same_health_again_publishes_nothing() {
        let (board, journal) = board();
        assert!(board.set(State::Serving, ""));
        assert!(board.set_part("runtimes/llama", State::Serving, "2 instances"));
        let head = journal.head();
        assert!(!board.set(State::Serving, ""));
        assert!(!board.set_part("runtimes/llama", State::Serving, "2 instances"));
        assert_eq!(journal.head(), head, "nothing published for no change");
        assert!(
            board.set_part("runtimes/llama", State::Serving, "3 instances"),
            "a detail is a change"
        );
        assert_ne!(journal.head(), head);
    }

    /// What a restart costs is said, published only when it changes, and taken back (D, 2026-11-10).
    #[test]
    fn a_restart_cost_is_said_once_per_change_and_can_be_taken_back() {
        let (board, journal) = board();
        let cost = anemnez_extension::health::RestartCost {
            line: "qwen-35b warm".into(),
            secs: Some(40),
        };
        assert!(board.set_restart_cost(Some(&cost)));
        let got = board.get().restart_cost.expect("said");
        assert_eq!((got.line.as_str(), got.secs), ("qwen-35b warm", Some(40)));
        let head = journal.head();
        assert!(
            !board.set_restart_cost(Some(&cost)),
            "the same cost again is no change"
        );
        assert_eq!(journal.head(), head);
        assert!(
            board.set_restart_cost(None),
            "absent again: the product's name only"
        );
        assert_eq!(board.get().restart_cost, None);
    }

    /// A stopped model's part goes, and its going is published (K).
    #[test]
    fn a_removed_part_is_gone_and_its_going_published() {
        let (board, journal) = board();
        board.set_part("runtimes/llama", State::Serving, "");
        board.set_part("runtimes/qwen", State::Serving, "");
        let head = journal.head();
        assert!(board.remove_part("runtimes/llama"));
        assert_ne!(journal.head(), head);
        let names: Vec<_> = board.get().parts.into_iter().map(|p| p.name).collect();
        assert_eq!(names, ["runtimes/qwen"]);
        let head = journal.head();
        assert!(!board.remove_part("runtimes/llama"), "already gone");
        assert_eq!(journal.head(), head);
    }

    /// Syncing the whole set removes what is not named and publishes once, only on a change.
    #[test]
    fn syncing_parts_publishes_once_and_only_on_a_change() {
        let (board, journal) = board();
        board.set_part("runtimes/llama", State::Serving, "");
        board.set_part("runtimes/qwen", State::Serving, "");
        let head = journal.head();
        assert!(board.sync_parts([
            ("runtimes/qwen", State::Degraded, "slow"),
            ("runtimes/phi", State::Starting, "")
        ]));
        let after_one = journal.head();
        assert_eq!(
            after_one
                .rsplit_once(':')
                .map(|(_, s)| s.parse::<u64>().unwrap_or(0)),
            head.rsplit_once(':')
                .map(|(_, s)| s.parse::<u64>().unwrap_or(0) + 1),
            "one event for the whole sync"
        );
        let mut names: Vec<_> = board.get().parts.into_iter().map(|p| p.name).collect();
        names.sort();
        assert_eq!(names, ["runtimes/phi", "runtimes/qwen"]);
        assert!(!board.sync_parts([
            ("runtimes/qwen", State::Degraded, "slow"),
            ("runtimes/phi", State::Starting, "")
        ]));
        assert_eq!(
            journal.head(),
            after_one,
            "the same set again publishes nothing"
        );
    }
}
