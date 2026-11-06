# The extension contract

> The open contract an Anemnez extension is written against (MIT OR Apache-2.0). Every product is an
> extension (the User, 2026-11-05); this is what one offers its callers: **calls** (its own proto,
> with these options on its methods), **events** (one resumable stream), and **health** (state, with
> parts). It carries no platform: hosted on the Anemnez platform, an extension is reached, routed and
> granted by the platform's own pieces (anemnez-service `PRODUCT.md` for the hosting side; fabric
> `CONVERGENCE.md` for how they meet); on its own it is a complete program.
>
> Moved verbatim from anemnez-service `PRODUCT.md` §6 (D, 2026-11-05, for substrate UNIFIED_PROTOCOL
> B9; agreed with B, C, E, H, I), with its proto package renamed once: `anemnez.fabric.product.v1`
> → `anemnez.extension.v1`. The protos are in `crates/anemnez-extension/proto/anemnez/extension/v1/`; the crate's plain Rust
> types are held to them name for name.

Claims are marked `<!-- claim: … -->`. A claim is owed until a named test speaks for it
(`proofs/ut/nextest.prova.lua`).

## 1. The descriptor: what an extension offers callers


(D, 2026-11-05, for substrate UNIFIED_PROTOCOL B9: every Fabric-hosted product is a Lua namespace an
agent may be granted, mounted at runtime from what the product declares. Agreed with B, C, E, H, I.)

<!-- claim: a-product-describes-itself-from-its-idl -->
**A product describes what it offers from its own IDL, the gRPC proto it already serves.** The
descriptor is that proto's `FileDescriptorSet`, plus the options below written on its methods, and is
served by the product itself: `anemnez.extension.v1.Describe/Get` on the instance socket, beside
`Lifecycle`. There is no second IDL to keep in step, and the descriptor cannot drift from the build.

<!-- claim: a-contract-is-named-and-versioned -->
**Each contract a product offers is named and versioned in its manifest**:
`"contracts": { "herald": "2.3.0" }` (semver; a caller requires a range, `herald ^2`). An instance
outside a caller's range is refused at connect with both versions named, never called. Discovery's
`Instances` record carries the descriptor's SHA-256, so a caller re-fetches only when it changes.

**The options layer** (`anemnez.extension.v1`, `crates/anemnez-extension/proto/anemnez/extension/v1/options.proto`), per method:

| Option | Values | Means |
|---|---|---|
| `description` | text | what the method does, written for the agent that calls it; its quality is a proof in the product's gate |
| `result` | `cursor` \| `bound(n)` \| `one` | a collection is a resumable cursor or capped at n; a descriptor with an unbounded collection is refused at connect |
| `effect` | `read` \| `write` | whether it changes anything |
| `route` | `leader(group)` \| `local-first` \| `any` | where a call goes: the instance leading `group` (its election, fabric SWARM.md), this host's instance first, or any instance. Default `local-first` |
| `person_act` | bool | the act is a person's: the caller's grant must be Ask-gated, and the product refuses it from an agent principal without one |
| `presence` | `full` \| `minimal` \| `index` \| `hidden` | how prominently a caller's tool surface lists it (Substrate's four tiers, TOOL_GUIDE) |
| `permission` | `allow` \| `ask` \| `deny`, **required** | the product's suggested permission, which an agent with no grant of its own for this method reads (Substrate's sparse rule; a role reads it closed). No default: every method states one. A `person_act` method may not say `allow`; the descriptor is refused at connect by name if it does |
| `callers` | `agent` \| `product` \| `owner`, **required** | who may call it, one to one with Ladon's `Caller` (and never confused with a principal token's `audience`, which names the instance that may accept it): only an `agent` method gets an agent face (Substrate's Lua, Agora's MCP); a `product` method is for another product's process (Ladon's value-bearing calls); an `owner` method is a person's CLI. No default, so nothing gets an agent face by omission (G, fabric CONVERGENCE.md §4). The method still checks its caller itself; the face is never the only gate |
| `session` | `opens` \| `within` \| `closes` | for state that outlives one call (Archetect's prompt sessions, Prova's held topologies): `opens` answers a session id, `within` and `closes` take it. A session belongs to the principal that opened it, and the product ends it when that principal's grant goes (E) |

<!-- claim: a-product-pushes-events-as-a-stream-it-serves -->
**A product's events are one more method it serves**, never a push through Fabric: a server stream
`Events(after: cursor, kinds?)` declared in the descriptor, one stream with one cursor space, resumable
from the cursor a caller last saw, filtered to the kinds it asks for (B). Each kind is declared in the
proto like a method, and no payload ever carries a secret's value (Ladon's redaction applies). The
descriptor declares `cursor_scope`: `swarm` (the events are replicated, so a cursor resumes at any
instance) or `instance` (a cursor carries its issuing instance and is refused elsewhere by name,
`cursor-not-mine`; the subscriber starts fresh at the new instance and surfaces the gap to its owner,
never papers over it) (I, B). An instance-scoped cursor also names the instance's boot (K's Typhon:
`<inst8>.<boot>:<seq>`), so a cursor from before a restart, whose sequence the restart reset, is
`cursor-not-mine` too, never silently resumed. Two more refusals end a stream, each surfaced, never
silent: `out-of-range`, a cursor older than what is kept (start fresh at the head), and `data-loss`, a
subscriber fallen behind the live stream (resubscribe from the last cursor seen). A stream's principal is checked when it opens, and the product ends it at
that token's expiry with a retryable `expired`; the subscriber re-mints and resumes from its cursor. A
caller subscribes to the products it is granted; across hosts the stream rides Fabric's stream brokering
(fabric SWARM.md `#fabric-brokers-streams-not-protocols`).

<!-- claim: health-is-state-with-parts -->
**Health is state, with parts, and events after it.** `Health/Get` answers `{state, parts, as_of}`:
`state` one of `STARTING`, `SERVING`, `DEGRADED`, `FAILING`, `DRAINING`, `DRAINED`, `UNKNOWN`; `parts`
named by path (`memory/index`), nesting to the depth the descriptor declares, each `{name, state, detail,
since}`; `as_of` a cursor. A watcher reads it, then subscribes `Events(after: as_of, kinds = [health])`, so
it never races the snapshot and never replays history. To Fabric, `SERVING` and `DEGRADED` are both
serving: an upgrade verifies on either and records the degradation, because the product is the judge of
whether it can do its work, and a product that should be rolled back says `FAILING`. (Substrate's
`HealthStatus` and `Diagnostic` are replaced by these, not mapped: B, CONVERGENCE.md.)

<!-- claim: every-call-carries-its-callers-principal -->
**Every call carries its caller's principal, and the product checks it.** A call's metadata carries
the Substrate-signed principal token (substrate UNIFIED_PROTOCOL B9 (6): per call, at most 5 minutes,
bound to the agent, its Substrate instance and the audience). The product verifies it against the
instance key Fabric publishes and attests (`fabric.principals/<instance id>`), and enforces only its
own data. On gRPC it rides the metadata key `anemnez-principal`; over MCP, `_meta.anemnez.principal` (substrate
`docs/specs/PRINCIPAL_TOKEN.md` is the token's format). No log keeps the token; logs keep the claimed
address, the Substrate id, the expiry and the token's digest.

First adopter: Herald (E), whose descriptor's acts and queue reads route `leader(herald)`.

<!-- claim: one-skew-for-the-swarm -->
**One skew for the swarm, and one token life** (B, J, 2026-11-06). The crate exports
`principal::SKEW_MS = 30_000` and `principal::MAX_TTL_MS = 300_000`, the two constants substrate's
`docs/specs/PRINCIPAL_TOKEN.md` derives every window from: a token is not yet valid past
`issued + SKEW`, expired past `expires + SKEW` or when `expires - issued > MAX_TTL`, and a nonce is
held for `MAX_TTL + SKEW`. Anything else in the swarm that compares one host's clock with another's
(lanes' claim expiry, the consent engine's `skew_ms`) reads the same `SKEW_MS` rather than a literal
of its own, so one value is reasoned about everywhere. Changing either is a contract change.
