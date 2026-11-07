# CLAUDE.md

anemnez-extension: the open contract an Anemnez extension is written against (MIT OR Apache-2.0,
a public repo). Read `docs/specs/CONTRACT.md` first.

- **No platform in it.** Nothing here may depend on Fabric, the swarm or launchd; the hosting side is
  anemnez-service's. A test refuses `fabric` in the protos' code.
- **Two crates, two layers** (substrate `docs/plans/ASYNCIFICATION.md`). `anemnez-extension` is the
  kernel: pure types, serde and ed25519 only, no runtime, no clock, no private crate.
  `anemnez-extension-serve` is the async serving face (Events, Health, Describe over tonic); it takes
  an injected `anemnez-clock` `Clock`, private and on Artichoke, which is allowed because both ship
  to Artichoke only (DECISION crates-io-publishing, User 2026-11-07). Never a `serve` feature on the
  kernel: Cargo unifies features. No `block_on`, no `Clock::system()` inside either crate.
- **The protos are the source of truth** (`crates/anemnez-extension/proto/anemnez/extension/v1/`); the Rust types are held to
  them name for name by `parity_tests.rs`.
- **Proofs:** `prova`, `prova run ut`, `prova run quality`, `prova run all`.
- **Releasing is Herald's**, to Artichoke only for now. Land by train; never publish by hand.
- VCS is jj; never git.
