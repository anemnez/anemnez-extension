//! The open contract an Anemnez extension is written against (`docs/specs/CONTRACT.md`).
//!
//! An extension offers three things: **calls**, described by its own proto with the options in
//! `crates/anemnez-extension/proto/anemnez/extension/v1/options.proto`; **events**, one resumable stream; and **health**,
//! state with parts. Nothing here knows a platform: no Fabric, no swarm, no launchd. Hosted on the
//! Anemnez platform an extension gains more faces and reach; on its own it is a complete program.
//!
//! These are plain Rust types for the contract's vocabulary. The protos are the source of truth,
//! and a test holds every enum here to its proto, name for name.

pub mod describe;
pub mod descriptor;
pub mod exposure;
pub mod health;
pub mod label;
pub mod mcp;
pub mod media;
pub mod options;
pub mod peer;
pub mod signed;

/// The proto files, for a build that compiles them (`prost`, `tonic`) or a tool that reads them.
pub mod protos {
    pub const OPTIONS: &str = include_str!("../proto/anemnez/extension/v1/options.proto");
    pub const EVENTS: &str = include_str!("../proto/anemnez/extension/v1/events.proto");
    pub const HEALTH: &str = include_str!("../proto/anemnez/extension/v1/health.proto");

    /// Where each proto sits under an include root, as `import` names it.
    pub const FILES: [(&str, &str); 3] = [
        ("anemnez/extension/v1/options.proto", OPTIONS),
        ("anemnez/extension/v1/events.proto", EVENTS),
        ("anemnez/extension/v1/health.proto", HEALTH),
    ];

    /// Write every proto under `root` (`root/anemnez/extension/v1/…`), for a build script that
    /// compiles an extension's own proto: add `root` as an include path, and the version of this
    /// crate you depend on IS the contract you compile against, with no copy in your repository.
    ///
    /// ```no_run
    /// // build.rs
    /// let root = std::path::PathBuf::from(std::env::var("OUT_DIR").unwrap()).join("proto");
    /// anemnez_extension::protos::write_all(&root).unwrap();
    /// // then: tonic_build / prost_build with `root` among the include paths
    /// ```
    pub fn write_all(root: &std::path::Path) -> std::io::Result<()> {
        for (path, text) in FILES {
            let file = root.join(path);
            if let Some(dir) = file.parent() {
                std::fs::create_dir_all(dir)?;
            }
            // Written only when different, so a build script does not re-trigger a rebuild.
            if std::fs::read_to_string(&file).ok().as_deref() != Some(text) {
                std::fs::write(&file, text)?;
            }
        }
        Ok(())
    }
}

/// Named refusals a caller acts on (CONTRACT.md `#a-product-pushes-events-as-a-stream-it-serves`).
pub mod refusal {
    /// What a consumer is told, typed, when a product's `Events/Watch` is unserved (an older producer, or
    /// a Journal): it reads with `Events/Call` instead, never retrying an Unclear (D).
    pub const WATCH_UNSERVED: &str = "watch-unserved";
    /// `Events/Call` or `Events/Watch` on a product whose events are not on its hydramq-events log yet:
    /// read them with `Events/Events` until it moves (CONTRACT.md
    /// `#a-products-events-are-read-through-one-call`).
    pub const EVENTS_CALL_UNSERVED: &str = "events-call-unserved";
    /// An instance-scoped cursor presented to another instance: subscribe afresh, and surface the gap.
    pub const CURSOR_NOT_MINE: &str = "cursor-not-mine";
    /// The stream's principal token ran out: re-mint, and resume from the last cursor.
    pub const EXPIRED: &str = "expired";
    /// The cursor is older than what the extension still keeps: start fresh at the head, and
    /// surface the gap to the subscriber's owner, never paper over it.
    pub const OUT_OF_RANGE: &str = "out-of-range";
    /// The subscriber fell behind the live stream and events were dropped: the stream ends with this,
    /// never silently; resubscribe from the last cursor seen and surface the gap.
    pub const DATA_LOSS: &str = "data-loss";
    /// The instance is draining: the stream sent every event it held, then ended, retryable. Subscribe afresh at
    /// another instance from your own checkpoint (a cursor is its instance's own, so this one's is `cursor-not-mine`
    /// there) and surface the gap: no lossless resumption is promised (CONTRACT.md §8.3).
    pub const DRAINING: &str = "draining";
    /// `Events/Watch` past the most concurrent watches a product's log face holds (each is a thread of its own):
    /// refused at once, never queued; retry later or read with `Events/Call` (CONTRACT.md, the log face).
    pub const EVENTS_WATCH_BUSY: &str = "events-watch-busy";
    /// `Events/Call` whose read on the product's log didn't answer within the face's bound: Unclear, retry.
    pub const EVENTS_CALL_UNANSWERED: &str = "events-call-unanswered";
    /// `Events/Call` past the most concurrent reads a product's log face holds (each holds a blocking thread
    /// while it reads): refused at once, never queued; retry later (CONTRACT.md, the log face).
    pub const EVENTS_CALL_BUSY: &str = "events-call-busy";
    /// The product's own authorization of an `Events/Call` or `Events/Watch` refused it, or didn't answer within
    /// its bound: nothing of the log is served (CONTRACT.md, the log face). The reason follows the name.
    pub const UNAUTHORIZED: &str = "unauthorized";
    /// A keyed-route call carried an empty key: refused by the bridge before the call leaves, since
    /// no holder can be resolved for it.
    pub const NO_KEY: &str = "no-key";
    /// A call whose method declares a route this build doesn't know (a newer descriptor's variant): refused by name,
    /// `route-unknown: <method>`, never sent local-first or any (D m/22441).
    pub const ROUTE_UNKNOWN: &str = "route-unknown";
}

/// The principal token (CONTRACT.md `#every-call-carries-its-callers-principal`).
pub mod principal;

#[cfg(test)]
mod parity_tests;
