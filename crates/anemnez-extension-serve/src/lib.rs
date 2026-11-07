//! The serving face of the Anemnez extension contract (`docs/specs/CONTRACT.md`): an extension's
//! **Events** stream, its **Health** and its **Describe**, served over gRPC.
//!
//! This is the async layer above the `anemnez-extension` kernel, and it keeps the async strategy's
//! rules (substrate `docs/plans/ASYNCIFICATION.md`):
//!
//! - **Time comes from the [`Clock`] it is given**, never the system's: a stream's end at its
//!   token's expiry is a deadline on that clock, so a `ManualClock` test drives it.
//! - **Publishing never waits on a subscriber.** [`Journal::publish`] appends and wakes, and is a
//!   plain call. A subscriber that falls further behind than the journal keeps is ended with
//!   `data-loss`, never waited for.
//! - **A dropped subscriber cancels nothing.** A stream only reads the journal; the extension's own
//!   work never runs inside it.
//! - **No bridges.** Nothing here blocks on a runtime; every caller is async.
//!
//! [`Clock`]: anemnez_clock::Clock

pub mod describe;
pub mod health;
pub mod journal;

pub use describe::Describe;
pub use health::Board;
pub use journal::{Journal, Refusal, Subscription};

/// The contract's gRPC types and services, compiled from the kernel crate's protos.
#[allow(clippy::all, missing_docs)]
pub mod pb {
    tonic::include_proto!("anemnez.extension.v1");
}

#[cfg(test)]
mod serve_tests;
