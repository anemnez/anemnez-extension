# anemnez-extension

The open contract an Anemnez extension is written against.

An extension offers three things:

- **Calls**: its own gRPC proto, with the options in `crates/anemnez-extension/proto/anemnez/extension/v1/options.proto` on
  each method (what it does, who may call it, whether it writes, how its results come back).
- **Events**: one resumable stream, `Events(after, kinds?)`.
- **Health**: `Health/Get`, state with parts, and the events cursor it is true as of.

Nothing here depends on a platform. An extension built on this contract is a complete program on
its own; hosted on the Anemnez platform it also gains a Lua face for agents, an MCP face for clients
like Claude Code, and routing across a swarm, built from the same descriptor.

The contract and its claims: [`docs/specs/CONTRACT.md`](docs/specs/CONTRACT.md).

## Using it

Take the contract crate from this repository as a git dependency, pinned to a tag or a rev:

```toml
[dependencies]
anemnez-extension = { git = "https://github.com/anemnez/anemnez-extension", rev = "<12-hex rev>" }
```

It needs nothing beyond crates.io: the descriptor types, the method options, `check_descriptor`,
`exposure` and the protos (and the MCP mapping, as it lands). An extension serves `anemnez.extension.v1.Describe/Get`
from these types, answering its own `FileDescriptorSet`. Nothing here is on crates.io yet; it will
be, one crate at a time.

The other crates in this workspace (`-serve`, `-client`, `-fixture`) are built and released inside
Anemnez for now: they depend on a clock library from Anemnez's own registry, so they do not
resolve from a git checkout yet.

## Working in this repository

Building this workspace needs Anemnez's private registry (`.cargo/config.toml`, for the serving
crates), and the proof suite's standards leg (`.prova/prova.toml`, and the CI token in
`.github/workflows`) reads a private standards repository, so neither a clone's build nor its CI
runs outside Anemnez yet. Outside it, take the contract crate as a git dependency, as above: that
resolves from crates.io alone.

## Licence

Licensed under either of [Apache License, Version 2.0](LICENSE-APACHE) or [MIT](LICENSE-MIT), at
your option.
