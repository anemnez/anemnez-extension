//! The Watch driver: one `Events/Watch` stream over a product's own log, as a kernel (house rule 1). It is
//! pure and sync, with no I/O and no clock. The host ([`crate::log_events`]) feeds it what happened (a frame
//! the watch thread read, the thread ending, the instance's drain, the token's expiry, the product's authorization
//! ending (its revoke, or the end it said at admit), the consumer gone) and
//! does what it answers: send a frame, stop the wire watch, end the stream by name.
//!
//! Invariant: the stream never ends with its wire watch still running. Every way into [`State::Ended`]
//! answers [`Output::Stop`] first (once), so no watch thread outlives its stream by more than the frame it is
//! sending.

use anemnez_extension::refusal;

/// Why a stream ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum End {
    /// The producer's watch ended on its own (its last frame said why: compacted, refused, failed).
    Producer,
    /// The instance drained: what was read before the ask was sent, then the stream ended (CONTRACT.md §8.3).
    Draining,
    /// The stream's principal token ran out: the consumer re-mints, is authorized again, and resumes.
    Expired,
    /// The consumer went away.
    ConsumerGone,
    /// The product's authorization stopped holding before the token's life: its revoke, or the end it said when it
    /// admitted the stream. What was sent stays sent; nothing more is.
    Unauthorized,
}

impl End {
    /// The refusal the consumer is told, or `None` when the stream ends cleanly or no one is left to tell.
    #[must_use]
    pub fn refusal(self) -> Option<&'static str> {
        match self {
            End::Draining => Some(refusal::DRAINING),
            End::Expired => Some(refusal::EXPIRED),
            End::Unauthorized => Some(refusal::UNAUTHORIZED),
            End::Producer | End::ConsumerGone => None,
        }
    }
}

/// Where a stream is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum State {
    /// Frames are read and sent.
    Streaming,
    /// The drain asked: the wire watch is stopped, and what it already read is still sent.
    Draining,
    /// Over.
    Ended(End),
}

/// What happened, as the host saw it.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum Input {
    /// A frame the watch thread read from the log.
    Frame(Vec<u8>),
    /// The watch thread ended, and every frame it read has been fed.
    ProducerEnded,
    /// The instance's drain asked the stream to end.
    Drain,
    /// The principal token's expiry passed.
    Expired,
    /// The consumer dropped the stream.
    ConsumerGone,
    /// The product revoked the authorization it gave this stream.
    Revoked,
    /// The end the product's authorization said at admit passed.
    Lapsed,
}

/// What the host does.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum Output {
    /// Send this frame to the consumer.
    Send(Vec<u8>),
    /// Stop the wire watch (`hydramq_events::Events::stop` with its stopper).
    Stop,
    /// End the stream: with its [`End::refusal`] when it has one.
    End(End),
}

/// One Watch stream's driver.
#[derive(Debug)]
pub struct Driver {
    state: State,
    stopped: bool,
}

impl Default for Driver {
    fn default() -> Self {
        Driver::new()
    }
}

impl Driver {
    /// A stream whose wire watch is running.
    #[must_use]
    pub fn new() -> Driver {
        Driver { state: State::Streaming, stopped: false }
    }

    /// Where it is.
    #[must_use]
    pub fn state(&self) -> State {
        self.state
    }

    /// Whether it has asked for the wire watch to stop.
    #[must_use]
    pub fn stopped(&self) -> bool {
        self.stopped
    }

    /// What to do about `input`.
    pub fn step(&mut self, input: Input) -> Vec<Output> {
        let mut out = Vec::new();
        match (self.state, input) {
            (State::Ended(_), _) | (State::Draining, Input::Drain) => {}
            (State::Streaming | State::Draining, Input::Frame(frame)) => out.push(Output::Send(frame)),
            (State::Streaming, Input::ProducerEnded) => self.end(End::Producer, &mut out),
            (State::Draining, Input::ProducerEnded) => self.end(End::Draining, &mut out),
            (State::Streaming, Input::Drain) => {
                self.stop(&mut out);
                self.state = State::Draining;
            }
            (State::Streaming | State::Draining, Input::Expired) => self.end(End::Expired, &mut out),
            (State::Streaming | State::Draining, Input::ConsumerGone) => self.end(End::ConsumerGone, &mut out),
            (State::Streaming | State::Draining, Input::Revoked | Input::Lapsed) => {
                self.end(End::Unauthorized, &mut out);
            }
        }
        out
    }

    fn stop(&mut self, out: &mut Vec<Output>) {
        if !self.stopped {
            self.stopped = true;
            out.push(Output::Stop);
        }
    }

    fn end(&mut self, end: End, out: &mut Vec<Output>) {
        self.stop(out);
        self.state = State::Ended(end);
        out.push(Output::End(end));
    }
}
