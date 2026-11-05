# CLAUDE.md

anemnez-extension: the open contract an Anemnez extension is written against (MIT OR Apache-2.0,
a public repo). Read `docs/specs/CONTRACT.md` first.

- **No platform in it.** Nothing here may depend on Fabric, the swarm, launchd or any private crate;
  the hosting side is anemnez-service's. A test refuses `fabric` in the protos' code.
- **The protos are the source of truth** (`proto/anemnez/extension/v1/`); the Rust types are held to
  them name for name by `parity_tests.rs`.
- **Proofs:** `prova`, `prova run ut`, `prova run quality`, `prova run all`.
- **Releasing is Herald's**, to Artichoke only for now. Land by train; never publish by hand.
- VCS is jj; never git.
