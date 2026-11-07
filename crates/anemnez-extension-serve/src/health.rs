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

    /// Set the extension's own state.
    pub fn set(&self, state: State, detail: &str) {
        let mut reply = self.reply();
        reply.state = proto_state(state) as i32;
        detail.clone_into(&mut reply.detail);
        self.publish(&mut reply);
    }

    /// Set one part's state, by path (`memory/index`). Its `since` moves only when its state does.
    pub fn set_part(&self, name: &str, state: State, detail: &str) {
        let now = self.journal.clock().now_wall().unix_millis();
        let state = proto_state(state) as i32;
        let mut reply = self.reply();
        match reply.parts.iter_mut().find(|p| p.name == name) {
            Some(part) => {
                if part.state != state {
                    part.state = state;
                    part.since = now;
                }
                detail.clone_into(&mut part.detail);
            }
            None => reply.parts.push(pb::Part {
                name: name.to_owned(),
                state,
                detail: detail.to_owned(),
                since: now,
            }),
        }
        self.publish(&mut reply);
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
