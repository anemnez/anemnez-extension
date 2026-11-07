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
| `route` | `leader(group)` \| `leader(group, key)` \| `local-first` \| `any` | where a call goes: the instance leading `group` (its election, fabric SWARM.md), the instance holding the call's `key` in `group` (one leader per key, resolved through Fabric's lock on every call), this host's instance first, or any instance. Default `local-first` |
| `person_act` | bool | the act is a person's: the caller's grant must be Ask-gated, and the product refuses it from an agent principal without one |
| `presence` | `full` \| `minimal` \| `index` \| `hidden` | how prominently a caller's tool surface lists it (Substrate's four tiers, TOOL_GUIDE) |
| `permission` | `allow` \| `ask` \| `deny`, **required** | the product's suggested permission, which an agent with no grant of its own for this method reads (Substrate's sparse rule; a role reads it closed). No default: every method states one. A `person_act` method may not say `allow`; the descriptor is refused at connect by name if it does |
| `callers` | `agent` \| `product` \| `owner`, **required** | who may call it, one to one with Ladon's `Caller` (and never confused with a principal token's `audience`, which names the instance that may accept it): only an `agent` method gets an agent face (Substrate's Lua, Agora's MCP); a `product` method is for another product's process (Ladon's value-bearing calls); an `owner` method is a person's CLI. No default, so nothing gets an agent face by omission (G, fabric CONVERGENCE.md §4). The method still checks its caller itself; the face is never the only gate |
| `session` | `opens` \| `within` \| `closes` | for state that outlives one call (Archetect's prompt sessions, Prova's held topologies): `opens` answers a session id, `within` and `closes` take it. A session belongs to the principal that opened it, and the product ends it when that principal's grant goes (E) |

<!-- claim: a-keyed-route-names-a-string-field -->
**A keyed route names one string field of the call** (B, D, H, 2026-11-07). `leader(group, key)`
(`Route.keyed`, beside the unchanged group-only `leader`) routes each call to the instance holding
that call's key in `group`, so a product like Herald has one leader per repo rather than one for
everything (`leader("herald-repo", key = "repo")`). At connect, `key` must name a top-level,
non-repeated string field of the method's input message, or the descriptor is refused by name.
proto3 has no `required` and nested paths are not supported yet. `options::check_route_key` is that
one check, pure, for every bridge alike; it answers the field's number too, so a bridge reading wire
bytes finds the key without decoding the whole call (B). At call time an empty key is refused with
`no-key` before the call leaves. The bridge resolves the key's holder through Fabric's lock on every
call and never caches it past one read, so a handoff routes the next call to the new holder (D).

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
subscriber fallen behind the live stream (resubscribe from the last cursor seen). The descriptor also states `events_kept`, how many events the product keeps: how far
behind a subscriber may fall before `data-loss`, and how old a cursor may be before `out-of-range`, so a
consumer sizes its reading to it (C). A stream's principal is checked when it opens, and the product ends it at
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

<!-- claim: one-signed-framing -->
**One signed framing, many schemas** (B, I, D, 2026-11-06). Every signed statement in the swarm (a
principal token, an Iris answer) is signed over the same message, built by `signed::message`: the
domain string, the algorithm tag, then the schema's parts, each a u32 big-endian length then its
bytes, with strings UTF-8 and integers 8-byte big-endian. The domain string names the schema and its
version (`anemnez-principal/1`, `anemnez-iris-answer/1`), so a signature made for one can never be
replayed as another. The algorithm tag is framed from day one (`ed25519` today, `p256` when a
Secure Enclave key arrives), so a new algorithm needs no new framing. `signed::verify` checks the
signature and `signed::fresh` the window from `SKEW_MS` and `MAX_TTL_MS`, given the verifier's own
clock reading. Each schema keeps its own refusal steps (audience, method, nonce) beside these.

## 2. The serving face: `anemnez-extension-serve`

The kernel crate is pure: types, the protos and the signed framing, with no runtime and no clock
(substrate `docs/plans/ASYNCIFICATION.md`, layer 1). Serving the contract is async (layer 3), and
lives in a crate of its own, `anemnez-extension-serve`, never a feature of the kernel, because Cargo
unifies features and one consumer's `serve` would put tokio into every other's kernel build (B). It
offers a product the contract's three services: a `Journal` behind `Events`, a health `Board` behind
`Health` (it publishes only on a change, removes a part with `remove_part`, and `sync_parts` makes
the parts exactly a recomputed set in one event, so a product that recomputes its health on a period
fills the journal with changes, not the period: K), and `Describe` over the product's own `FileDescriptorSet`. A product takes them; it does
not reimplement them, so every product keeps these rules alike:

<!-- claim: async-stream-never-awaits-a-subscriber -->
**Publishing never waits on a subscriber.** `Journal::publish` is a plain call: it appends and wakes.
A subscriber is a position in the journal, never a queue the publisher fills, so a subscriber that
never reads costs the publisher nothing. One that falls more than `events_kept` behind has its stream
ended with `data-loss`, never silently skipped.

<!-- claim: a-stream-ends-at-its-tokens-expiry-on-the-injected-clock -->
**A stream ends at its token's expiry, on the injected clock.** The serving face takes an
`anemnez-clock` `Clock` at construction and never reads the system's; a stream's expiry is a `Mono`
deadline on that clock, read once when it opens (C), so a `ManualClock` test drives it end to end.

<!-- claim: async-cancelled-only-by-its-owner -->
**A dropped subscriber cancels nothing.** A stream only reads the journal; the product's own work
never runs inside a handler's future, so a client that disconnects ends its stream and nothing else.

<!-- claim: the-serving-face-runs-the-whole-principal-ladder -->
**The serving face runs the whole principal ladder** (B, 2026-11-07), so no product skips a step.
A `Verifier` runs substrate `PRINCIPAL_TOKEN.md` §4 in order: the kernel's stateless steps (1 to 4
and 7), and the two that need state through what the product hands it, never one without the
other: `Keys` (steps 5 and 6: the issuer's `fabric.principals` record, its attestation checked
against a pinned host node) and `Nonces` (step 8,
the durable set of accepted nonces, held `MAX_TTL + SKEW` past issue). `Events` opens a stream only
through it, and ends it at the token's expiry as a `Mono` on the injected clock. A gRPC call's body
digest is SHA-256 of the request message's bytes exactly as received, taken below tonic's codec by
`DigestLayer` on the token-checked methods, never a re-encoding (D): prost drops unknown fields on
decode, so a re-encoding would refuse every newer caller the day a message gained a field. A
compressed frame, or a method served without the layer, is refused `malformed`. Token refusals keep their names as the status message:
`not-for-you`, `unknown-key`, `expired` and `not-yet-valid`, which a caller may retry with a fresh
token, are UNAUTHENTICATED; `call-mismatch`, `bad-signature`, `unattested` and `replay`, which are
never retried, are PERMISSION_DENIED; `malformed` is INVALID_ARGUMENT.

<!-- claim: the-verifier-applies-retired-versus-revoked -->
**The verifier, not the product, picks the key** (B, D, 2026-11-07; PRINCIPAL_TOKEN §2). `Keys`
answers the issuer's whole record and `KeyRecord::key_for` applies the rule the same for every
product. A REVOKED key refuses every token, a backdated one included, since `issued` is signed by
the key a thief holds. A RETIRED key verifies only a token issued before its `retired_at_secs`. Either
refusal is `unknown-key`. The record's times are Unix seconds like `issued` (B), named with the unit
(`made_at_secs`, `retired_at_secs`, `revoked_at_secs`), and a time outside 2020 to 2100 is a unit slip,
refused `unattested` rather than compared off by a thousand (D).

<!-- claim: a-token-binds-the-bytes-as-received -->
**A token binds the request bytes as received**, so a request carrying a field the product's build
does not know still verifies, and a request whose bytes changed in flight is refused `call-mismatch`.

<!-- claim: a-nonce-is-durable-before-the-method-runs -->
**A nonce is durable before the method runs.** `Verifier::verify` answers only after
`Nonces::record` has, so a method never runs on a token whose nonce a restart could forget. A product
supplies a durable store that group-commits (one fsync per batch of a few milliseconds);
`MemoryNonces` is for tests only.

**The rollout rule** (PRINCIPAL_TOKEN §3, B and D, for every product). A call carrying no token is
admitted only while `Keys::admit_absent` says so, which holds while some Substrate instance in the
swarm has no `fabric.principals` record yet. The default is `false`. Each such call is counted by
method (`Verifier::absent_calls`), for Health and a product's doctor to name. A token that is
present is always checked in full.

**The refusals reach a caller by name.** A refusal's name is the gRPC status message, under the
nearest code: `cursor-not-mine` FAILED_PRECONDITION, `out-of-range` OUT_OF_RANGE, `data-loss`
DATA_LOSS, `expired` UNAUTHENTICATED (retryable: re-mint and resume from the last cursor).

Cursors are instance-scoped today (`<inst8>.<boot>:<seq>`); a swarm-scoped journal, whose cursors
resume at any instance, is the next slice.
