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
}

/// Where a call's principal token rides (CONTRACT.md `#every-call-carries-its-callers-principal`).
pub mod principal {
    /// The gRPC metadata key.
    pub const GRPC_METADATA_KEY: &str = "anemnez-principal";
    /// The MCP `_meta` path: `_meta.anemnez.principal`.
    pub const MCP_META_PATH: [&str; 2] = ["anemnez", "principal"];

    /// The clock skew every party allows, at both ends of a window: one swarm-wide value
    /// (CONTRACT.md `#one-skew-for-the-swarm`; substrate `docs/specs/PRINCIPAL_TOKEN.md`).
    pub const SKEW_MS: u64 = 30_000;

    /// The longest a principal token may live: `expires - issued` never exceeds it.
    pub const MAX_TTL_MS: u64 = 300_000;

    #[cfg(test)]
    mod tests {
        /// The values PRINCIPAL_TOKEN.md names, and the skew a small part of the life it guards.
        #[test]
        fn the_token_windows_are_the_specs() {
            assert_eq!((super::SKEW_MS, super::MAX_TTL_MS), (30_000, 300_000));
            const { assert!(super::SKEW_MS * 4 < super::MAX_TTL_MS) };
        }
    }
}

#[cfg(test)]
mod parity_tests;
