//! The open contract an Anemnez extension is written against (`docs/specs/CONTRACT.md`).
//!
//! An extension offers three things: **calls**, described by its own proto with the options in
//! `crates/anemnez-extension/proto/anemnez/extension/v1/options.proto`; **events**, one resumable stream; and **health**,
//! state with parts. Nothing here knows a platform: no Fabric, no swarm, no launchd. Hosted on the
//! Anemnez platform an extension gains more faces and reach; on its own it is a complete program.
//!
//! These are plain Rust types for the contract's vocabulary. The protos are the source of truth,
//! and a test holds every enum here to its proto, name for name.

pub mod health;
pub mod options;

/// The proto files, for a build that compiles them (`prost`, `tonic`) or a tool that reads them.
pub mod protos {
    pub const OPTIONS: &str = include_str!("../proto/anemnez/extension/v1/options.proto");
    pub const EVENTS: &str = include_str!("../proto/anemnez/extension/v1/events.proto");
    pub const HEALTH: &str = include_str!("../proto/anemnez/extension/v1/health.proto");
}

/// Named refusals a caller acts on (CONTRACT.md `#a-product-pushes-events-as-a-stream-it-serves`).
pub mod refusal {
    /// An instance-scoped cursor presented to another instance: subscribe afresh, and surface the gap.
    pub const CURSOR_NOT_MINE: &str = "cursor-not-mine";
    /// The stream's principal token ran out: re-mint, and resume from the last cursor.
    pub const EXPIRED: &str = "expired";
}

/// Where a call's principal token rides (CONTRACT.md `#every-call-carries-its-callers-principal`).
pub mod principal {
    /// The gRPC metadata key.
    pub const GRPC_METADATA_KEY: &str = "anemnez-principal";
    /// The MCP `_meta` path: `_meta.anemnez.principal`.
    pub const MCP_META_PATH: [&str; 2] = ["anemnez", "principal"];
}

#[cfg(test)]
mod parity_tests;
