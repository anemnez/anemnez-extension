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

## Licence

Licensed under either of [Apache License, Version 2.0](LICENSE-APACHE) or [MIT](LICENSE-MIT), at
your option.
