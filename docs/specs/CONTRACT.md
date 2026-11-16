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
served by the product itself: `anemnez.extension.v1.Describe/Get` on its API plane (§8), and on the
instance socket beside `Lifecycle` until §8's step 2. There is no second IDL to keep in step, and the descriptor cannot drift from the build.

<!-- claim: one-describe-reply-for-every-product -->
**One `Describe` reply for every product** (D, E, L, H, 2026-11-14). The reply carries the
descriptor and its digest: SHA-256 of the encoded `FileDescriptorSet`, in lowercase hex, which a
caller compares so that it re-fetches only when the descriptor changes. It is made by one pure
function in this crate, `describe::reply`. The serving face, every product that serves `Describe`
itself (Herald; Prova and Archetect, which take this crate from git) and every reader all use that
one rule. A product mounts it with a three-line adapter onto its own generated `Describe` service.
`describe::DescribeReply` is the proto's message, field for field, and a parity test holds it there.

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
| `effect` | `read` \| `write` \| `act` | what the call does (D): `read` changes nothing; `write` changes state this product owns, visible in its data afterwards; `act` causes something outside the call's own data (runs a gate, sends a notification, moves a channel, starts a process). `person_act` is orthogonal: who confirms, not what happens |
| `route` | `leader(group)` \| `leader(group, key)` \| `local-first` \| `any` | where a call goes: the instance leading `group` (its election, fabric SWARM.md), the instance holding the call's `key` in `group` (one leader per key, resolved through Fabric's lock on every call), this host's instance first, or any instance. Default `local-first` |
| `person_act` | bool | the act is a person's: the caller's grant must be Ask-gated, and the product refuses it from an agent principal without one |
| `presence` | `full` \| `minimal` \| `index` \| `hidden` | how prominently a caller's tool surface lists it (Substrate's four tiers, TOOL_GUIDE) |
| `permission` | `allow` \| `ask` \| `deny`, **required** | the product's suggested permission, which an agent with no grant of its own for this method reads (Substrate's sparse rule; a role reads it closed). No default: every method states one. A `person_act` method may not say `allow`; the descriptor is refused at connect by name if it does |
| `callers` | `agent` \| `product` \| `owner`, **required** | who may call it, one to one with Ladon's `Caller` (and never confused with a principal token's `audience`, which names the instance that may accept it): only an `agent` method gets an agent face (Substrate's Lua, Agora's MCP); a `product` method is for another product's process (Ladon's value-bearing calls); an `owner` method is a person's CLI. No default, so nothing gets an agent face by omission (G, fabric CONVERGENCE.md §4). The method still checks its caller itself; the face is never the only gate |
| `session` | `opens` \| `within` \| `closes` | for state that outlives one call (Archetect's prompt sessions, Prova's held topologies): `opens` answers a session id, `within` and `closes` take it. A session belongs to the principal that opened it, and the product ends it when that principal's grant goes (E) |
| `blocks` | bool, default false | a call that does not finish at once parks the calling mind's execute (Substrate RUNTIME S2c) rather than answering Pending |
| `runs_to_completion` | bool, default false | bounded only by its own end, like `process.run`, so the caller's deadline is not charged for it. A request carrying a wait-budget field (`timeout_ms` and its kin) owns its budget without this; the bridge derives that, so it is never stated twice (C). Independent of `blocks` (D) |
| `examples` | `Example{args_json, caption}`, repeated (field 13; 12 is reserved) | worked calls, for a caller's help: each the request as a JSON argument object, which a bridge type-checks against the method's parameters and renders its own way (Lua for Substrate, JSON for Agora's MCP face: B, D); `check_descriptor` refuses `example-mismatch` when it isn't a JSON object of the request's top-level fields |
| `ask_within_secs` | `optional uint32` (field 14), seconds | how long a call may hold an Ask open (§8.1 `an-ask-is-bounded-in-the-call`): absent reads as 45, never 0; a method that never asks says 0; past 120 `check_descriptor` refuses `ask-bound-over-cap` with the value. `MethodOptions::ask_within()` reads it, so no bridge applies the default itself |

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

<!-- claim: every-bridge-refuses-a-descriptor-alike -->
**Every bridge refuses a descriptor alike** (B, D, H, 2026-11-07). `descriptor::check_descriptor`
takes the encoded `FileDescriptorSet` that `Describe/Get` serves and answers either every offered
method (path, parsed `MethodOptions`, a keyed route's field, and the request's top-level fields with
name, number, kind and repeated, from which a bridge builds the op's parameters: B, C) or EVERY refusal, each named with its
method: `callers-missing`, `permission-missing`, `person-act-allowed`, `result-missing`,
`unbounded-collection` (an output with a repeated field declared `one`), `effect-missing`, `ask-bound-over-cap`,
`presence-missing`, `description-missing`, `route-key`, `input-unknown`, `options-malformed`. A
method with no anemnez options is not offered, so nothing gets a face by omission. Substrate's Lua
mount, Agora's MCP face and the conformance proof all call it. What is a bridge's own (a Lua
identifier, reserved names, version ranges against the manifest) stays the bridge's. The options are
protobuf extensions, which `prost_types` drops on decode, so it reads them from each method's raw
option bytes; a parity test holds that reader to the proto's generated code.

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

<!-- claim: a-products-events-are-read-through-one-call -->
**A product's events are read through one call** (D, A, H, 2026-11-14). The face every product's
events go through is `hydramq-events` (fabric EVENTS.md §6a): a keyed log per producer, read from a
position or at its last value per subject. Its served face is bytes (`hydramq_events::wire`), and
this contract is its gRPC transport, defined once: `Events/Call(CallRequest{request}) ->
CallReply{reply}` carries one `wire` request and its answer, opaquely. This crate never parses them
and never depends on hydramq-events, so the open kernel still builds from git alone. A producer
answers with `hydramq_events::wire::serve` on its blocking pool. A consumer reads with
`AsyncRemote::new(anemnez_extension_client::events::EventsCall::new(channel, clock))`: the client
crate's adapter, bounded on the consumer's own injected clock, so it writes nothing of its own. The
two messages are `describe::CallRequest` and `CallReply` in this crate, held to the proto. A product whose events
are not on that log yet refuses the call `events-call-unserved`, as the serving face's Journal does.
**Retiring:** until every product reads and writes through `Call`, a consumer has two ways to read a
product's events, the `Events(after)` stream and `Call`, and the serving face keeps a second log, its
`Journal`. Both retire onto `Call` and the producer's hydramq-events log. Step 1 is this release:
`Call` is added and the Journal refuses it by name. Step 2 moves each product's events and the
serving face onto the log, and removes `Events(after)` and the Journal. Ends 2026-12-15 (owner K, from H).

<!-- claim: a-products-events-are-pushed-through-one-watch -->
**A product's events are pushed through one watch** (A, D, H, 2026-11-14; house rule 14: no long
polls). `Events/Watch(WatchRequest{request}) -> stream WatchFrame{frame}` is the same face's push:
the request is one `hydramq_events::wire::watch_request` (a position, at most `max` events a frame,
and a heartbeat), and each frame one of the producer's `wire::watch` frames (events, a heartbeat
after each quiet beat, or the end), carried opaquely, as `Call`'s bytes are. The producer drives its
`wire::watch` on a thread of its own (it waits on the log's clock, never on a runtime worker) into a
bounded channel, so a slow consumer stalls only its own stream. A consumer reads with
`hydramq_events::wire::Watcher::new(EventsFrames::open(channel, clock, &request, bound).await?, from,
beat)`: each pull is bounded on its injected clock and answers a frame, silence or the end, and
silence past the heartbeat plus the hop margin is the producer gone, named, never a hang. The
watcher's `at()` is the cursor a consumer commits once it has acted. A Journal refuses it
`events-call-unserved`, as it refuses `Call`, and it retires on the same row. A consumer is told that
apart, typed, never as an Unclear it would retry: the client answers an unserved `Watch` with
`Refused::ByProducer` named `watch-unserved`, and an unserved `Call` with `events-call-unserved`, so a
consumer such as Stoa's bridge says once that this product does not serve it and reads another way (D).

<!-- claim: health-is-state-with-parts -->
**Health is state, with parts, and events after it.** `Health/Get` answers `{state, parts, as_of}`:
`state` one of `STARTING`, `SERVING`, `DEGRADED`, `FAILING`, `DRAINING`, `DRAINED`, `UNKNOWN`; `parts`
named by path (`memory/index`), nesting to the depth the descriptor declares, each `{name, state, detail,
since}`; `as_of` a cursor. A watcher reads it, then subscribes `Events(after: as_of, kinds = [health])`, so
it never races the snapshot and never replays history. To Fabric, `SERVING` and `DEGRADED` are both
serving: an upgrade verifies on either and records the degradation, because the product is the judge of
whether it can do its work, and a product that should be rolled back says `FAILING`. (Substrate's
`HealthStatus` and `Diagnostic` are replaced by these, not mapped: B, CONVERGENCE.md.)

<!-- claim: health-may-say-what-a-restart-costs -->
**Health may say what a restart costs** (D, 2026-11-10). `restart_cost`, optional, is `{line, secs}`:
`line` one sentence a person reads ("qwen-35b-mlx warm (35B, about 40 s to reload) and the embedder"),
`secs` the component's own measured estimate of time back to serving, unset when it has not measured.
It is live state (which models are warm, how many sessions are attached), so it is Health's, not the
descriptor's. Absent means "name the product only": a reader (Herald's announce of what a train
reloads) never guesses, and a product busy changing itself leaves it absent rather than saying
"unknown". The serve face's `Board::set_restart_cost` publishes it only on a change.

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
replayed as another. The algorithm tag is framed from day one, so a new algorithm needs no new framing: `ed25519`
(principal keys, Iris devices) and `p256` (a Fabric node's identity attesting a record, later a
Secure Enclave key; a SEC1 key and a fixed 64-byte `r ‖ s` signature, never DER, B 2026-11-07). `signed::verify` checks the
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
the key a thief holds. A RETIRED key verifies only a token issued before its `retired_at_ms`. Either
refusal is `unknown-key`. Every time in a signed record is Unix MILLISECONDS, named with the unit
(the token's `issued_ms` and `expires_ms`; the record's `made_at_ms`, `retired_at_ms`,
`revoked_at_ms`; D, 2026-11-07, matching `fabric.devices` and the Iris answer), and a time outside
2020 to 2100 (`principal::PLAUSIBLE_MS`) is a unit slip: refused `malformed` in a token and
`unattested` in a record, rather than compared off by a thousand.

<!-- claim: a-token-binds-the-bytes-as-received -->
**A token binds the request bytes as received**, so a request carrying a field the product's build
does not know still verifies, and a request whose bytes changed in flight is refused `call-mismatch`.

<!-- claim: a-nonce-is-durable-before-the-method-runs -->
**A nonce is durable before the method runs.** `Verifier::verify` answers only after
`Nonces::record` has, so a method never runs on a token whose nonce a restart could forget. The crate
ships that store, `FileNonces::open(data_dir, clock)` (K, D), in its one edge module (ASYNCIFICATION
§4): one writer thread owns an append-only file and each caller awaits a oneshot. It group-commits:
it takes every record already queued, appends them, makes them durable with ONE full sync (std's `sync_data`, which
is `F_FULLFSYNC` on macOS, where a plain `fsync` leaves data in the drive's cache: D, J), and only then answers each, so a burst costs one fsync. At open it keeps what has not passed its hold (a torn
last line, never answered, is dropped), and it compacts by atomic rename once dead entries dominate.
A write that fails refuses the batch, never admits it. An acknowledged nonce survives a crash: a
child process records one, is answered, and aborts, and the reopened store still refuses it.
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

## 3. The fixture: `anemnez-extension-fixture`

<!-- claim: every-bridge-mounts-the-same-fixture -->
**Every bridge is proven against the same fixture product** (B, C, D, H, 2026-11-07), so a bridge
and the contract cannot drift. `anemnez_extension_fixture::serve(dir, clock, Variant)` runs a tiny
extension in-process on a Unix socket with one of each method shape: `Items` (a cursor Sequence,
route `any`), `Recent` (`bound(5)`), `Get` (`one`), `Hold` (a keyed-leader write that `blocks`),
`Run` (`runs_to_completion`), `Wait(timeout_ms)` (owns its budget by its field, not the flag: C),
and `Ping` with no anemnez options (not exposed: D), plus the contract's Describe, Health and
Events. Describe serves the variant's real protoc `FileDescriptorSet`. The refusing variants are
`Unbounded` and `PersonActAllow`, which `check_descriptor` refuses, and `WrongMajor`,
`CollidingNames` and `ReservedName`, which it admits for the bridge to refuse, since those rules are
the bridge's. Every unary reply echoes the caller's principal token's digest as
`anemnez-principal-digest`, so a call path can be proven to carry the token end to end; tokens are
not verified by the fixture yet.

## 4. Calling an extension: `anemnez-extension-client`

<!-- claim: every-bridge-encodes-a-call-alike -->
**Every bridge encodes a call alike** (B, D, H, 2026-11-07). `Calls::new(file_descriptor_set)` reads
the descriptor a product serves. `encode(path, args_json)` turns JSON arguments (proto3 JSON mapping)
into the request's bytes, strictly: a key the request has no field for, a value of the wrong type, or
anything but one JSON object is refused `args-mismatch` by name, never dropped, since an agent's typo
must not become a different act (D). The bytes it answers are the bytes a prost client would send,
and the caller hands those same bytes to the principal token's body digest and to `unary`, never
re-encoding. `unary` calls the exact method path over a raw-bytes codec, and `decode` turns the reply
back into JSON with the proto's own field names and every field present. Substrate's Lua mount and
Agora's MCP face both call it.

## 5. A name a person reads: `label::plain_label` (D, H, 2026-11-09)

<!-- claim: a-name-is-judged-by-one-check-from-unicodes-own-data -->
**A name is judged by one check, from Unicode's own data.** Every name a person reads as an identity
(Fabric's swarm names, Iris's identity and account, Stoa's heads, intake's frames) goes through
`label::plain_label`, so no product keeps its own list and none falls short (A found one already
short). A label is 1 to 64 characters with no control character, no U+2028 or U+2029, no
`Bidi_Control`, no `Default_Ignorable_Code_Point` (the invisible characters, Hangul fillers,
variation selectors, tags), no `White_Space` other than U+0020, no separator lookalike, and no space
at either end. The separator lookalikes are what products put between a name and its standing
(`G · vouched`) and what reads as it: U+00B7, U+0387, U+2022, U+2219, U+22C5, U+30FB, U+FF65, so a
name can't carry a standing it hasn't (C, I). Each refusal is named (`empty`, `too-long`,
`control`, `line-break`, `bidi`, `ignorable`, `space`, `separator`, `edge-space`) with the code
point and the release (`label::UNICODE`, today `16.0.0`); the characters are checked left to
right, each by those rules in that order, then the length and the edges. The properties are never
typed by hand: `unicode/<release>/` keeps Unicode's lines verbatim under their headers, the
separator set is one named list in the generator (no Unicode property names it), and one generator (`label_tests`) renders both `src/label_tables.rs` and the Swift copy
Iris.app vendors, `swift/PlainLabelTables.swift`, failing if either committed file differs (D), so
the Swift check shares the generated table and the vectors hold its logic. A release bump can only add
refusals, so it is a reviewed change: `label::VECTORS`, the vector file every caller runs, lists each
name and its verdict, and a bump that changes one fails, listing all that changed. A named limit: it
judges what a name contains, not what it looks like. A visible confusable (a fullwidth `Ｇ`, a Cyrillic
letter shaped like a Latin one) passes; telling lookalikes apart needs NFKC and Unicode's confusables
data, a separate check.

## 6. One exposure for every face: `exposure` (D, C, H, B, A, 2026-11-10)

<!-- claim: one-exposure-for-every-face -->
**What a method's options mean to an agent face lives once.** `exposure::exposure(&MethodOptions)`
answers `{face, consent}`: `face` is `Agent` only for a `callers: agent` method whose presence is not
`hidden`, and `Hidden` otherwise (a product's or an owner's method is never an agent's tool or Lua
function); `consent` is the method's DEFAULT, its `permission`, never Allow for a `person_act` method.
Agora's MCP face and Substrate's Lua mount both call it, so the two surfaces never disagree on what is
exposed or what asks (house rule 2). No row or grant changes `face`: a hidden method stays hidden.

<!-- claim: a-methods-default-is-a-floor -->
**A method's default is a floor, never a ceiling** (D). Consent that depends on the call (a
post to another user's member, say) cannot ride a per-method option, so the product's own per-call
check may raise a call to Ask, or refuse it, whatever the method's default; it never lowers one. A
face reads `consent: Allow` as "this method asks nothing by itself", never as "a call never asks": it
relays the product's Ask as it relays any other answer.

<!-- claim: a-row-tightens-freely-and-loosens-only-by-a-persons-grant -->
**A person's row tightens freely and loosens only with their signed grant.**
`exposure::effective(default, row, grant)` is the consent a call is held to: the default resolved
through the caller's own row (Substrate's affordance stores, which a person sets; Agora's face has none
and uses the default). A row at or past the default's strictness (Allow < Ask < Deny) stands; a row
that loosens it stands only with a `PersonsGrant`, a person's signed answer the caller verified
(Iris), and otherwise the default stands. A `PersonsGrant` is never made from what a Lua or MCP caller
sends: the mount's own proof (Substrate's, B) holds that a Lua call trying to loosen an Ask is refused.


## 7. One flattened face for MCP: `mcp` (B, C, D, E, H, 2026-11-13)

A product writes one contract, and two faces are made from it: Substrate's Lua mount (the rich
face) and the MCP tools an outside client sees (the flattened face; substrate
`docs/specs/PRODUCT_BRIDGE.md` "The two faces"). The flattened face is made by one pure function
here, so Agora, and any product that serves its own MCP face (Prova, Archetect), make the same tools
from the same descriptor, and a check of a hand-written face is a comparison against this
function's output.

<!-- claim: one-mapping-flattens-a-descriptor -->
**One mapping flattens a descriptor into MCP tools.** `mcp::flatten(&ServiceOptions, &[Method])`
answers one `Tool` per method whose `exposure` face is `Agent` (a product's, an owner's or a hidden
method is never a tool), then the `events` tool when the service keeps events, or every refusal. A tool's name is `mcp::op_name` of the method's
name, its snake case (`GetURL` → `get_url`), the same name a Lua mount gives the op; the host
qualifies it with the namespace (`<contract>__get_url`). Its description is the method's, then each
example as `caption: args_json`. Its input schema is the request's top-level fields in the proto3
JSON mapping, with no other key allowed, since a typo must not become a different act (§4): 32-bit
integers are `integer`, 64-bit ones `integer` or `string`, `bytes` a base64 `string`, an enum its
name or number, `Timestamp`, `Duration` and `FieldMask` a `string`, a wrapper its scalar, a map (a field whose entry
the descriptor marks `map_entry`, never guessed from its name: D) an `object`, and any other message an `object` that `encode` checks strictly. A tool also carries what
the host needs and the schema cannot say: the method's path, its `effect`, its default consent
(`exposure`), and whether it is long (`blocks` or `runs_to_completion`): work that can outlast a
turn, which the face answers with a job, never a wait (house rule 14). Each contract feature has one flattened shape, built here and carried on the tool (`result`,
`session`, `long`, listed under `_meta.anemnez`): a `cursor` result is a page tool whose request
carries the cursor and whose reply the next one; `bound(n)` a list; a `session` an id that `opens`
answers and `within` and `closes` take as a field of their request; `blocks` and
`runs_to_completion` a job (below).
A product's flattened face is the same on every host.

<!-- claim: long-work-flattens-to-a-job-and-an-event -->
**Long work flattens to a job and an event** (house rule 14; D, H, 2026-11-14). A `long` method's
MCP tool returns at once with a named job: `{job, method}`, the job naming that one call. Its outcome
(the reply, or a refusal by name) is an event on the host's own event log (`hydramq-events`):
subject `job/<id>`, kind `outcome`, with the recorded lane in its body. Stoa's bridge, when present,
tells it to that lane exactly once, keyed by (producer, log, offset), and Substrate's runtime can
consume it directly. The host never posts to Stoa itself and names no consumer (fabric EVENTS.md §1
rule 8; A, m/11156). **The lane is the caller the host authenticated
for the call, recorded with the job, and the outcome is told only there** (D). For Agora, that is the
lane on its stamp, vouched by that lane's lease on this host (`leased`, not lapsed), never an
argument, a field of the tool's input, or a lane the client merely claims. Otherwise a call could
route a job's reply into another lane's room. A long call with no vouched lane gets no job, and is
refused `long-needs-a-lane`. No MCP client waits on a verb whose own work can
outlast a turn, polls for it, or keeps a call open to learn it. The host owns the job as a machine
(rule 1), and its bounds are events (rule 4). A host still holds a short, bounded wait inside one
call, such as a server between builds (R9's bound), because that is rule 4's, not long work.
**Retiring:** the face first written here answered a long method with one call bounded by its
progress. That call retires: step 1 is this change, the spec. Step 2 is the job shape built in the
mapping and its host, with the progress-bounded long call removed. No host generates a long tool
yet, so nothing waits on it today. Ends 2026-12-15 (owner K, from H).

<!-- claim: a-flattened-name-is-one-tool -->
**A flattened name is one tool.** Two offered methods whose names flatten alike (`GetURL` and
`GetUrl`, or one name in two services) are refused `name-collision`, naming both paths, never
one picked; and `events` is reserved for the events tool, so a method that would flatten to it
is refused `reserved-name`. The fixture's `CollidingNames` variant is refused, and every refusal
is answered, not the first.

<!-- claim: events-flatten-to-one-bounded-read -->
**Events flatten to one bounded read.** A product whose service keeps events (`events_kept` above
0) has the `events` tool, and one keeping none has none, since it would answer nothing (D): `{after, kinds?, max?}` in, a batch of events up to the head and the
cursor to pass next out. It is the flattened face of the product's own `Events` stream, read by
the host for at most `max` events and a bounded wait, never a second events API, and it is for
outside clients only: an agent of ours never polls (Stoa delivers). A refusal the stream ends with
(`cursor-not-mine`, `out-of-range`, `data-loss`, `expired`) is answered by name.

<!-- claim: an-mcp-call-names-one-method -->
**An MCP call names one method.** `mcp::resolve(&tools, name)` answers the tool a call names, and
so the path its `arguments` are encoded for by `encode` (§4), which refuses anything but a JSON
object of the request's fields; a name no tool has is `no-such-tool`. Nothing the client sends
picks the path.

<!-- claim: an-external-caller-is-parsed-here-and-trusted-only-from-its-peer -->
**An external caller's stamp is parsed here and trusted only from its peer** (D, H, 2026-11-13).
Agora stamps every call to one of our servers with who called: always an external agent, the lane
the client says it works on, the place the kernel proved and the lane's lease (agora
`AGORA.md#a-caller-through-agora-is-external`). Over MCP it rides `_meta.anemnez.caller`; when
Agora calls a product's socket itself (the generated face), the same JSON rides the binary metadata
key `mcp::STAMP_METADATA_KEY` (`anemnez-caller-bin`), so both faces carry one value. `mcp::Stamp`
is that value, read and written here once. `Stamp::from_metadata` only parses, into an
`Untrusted<Stamp>`: what a peer sent says nothing until the server knows the peer is Agora's
daemon, and deciding that needs peer identity, which is not in this crate. So nothing here
trusts a stamp: `Untrusted::vouched_by_peer` is called only by the closed constructor that read
the peer at accept (anemnez-service's peer module, with a census that nothing else calls it), and
from any other peer a call reads `Stamp::unvouched()`, external with no lane. A malformed stamp,
a lane that is not a `plain_label`, or a lease with no lane reads as no lane too: it fails
closed, never into a guess. The lane attributes a call and authorizes nothing.

## 8. Two planes per product (D, H, I, E, K, 2026-11-16)

At the User's ask (K m/13414), ruled by D (m/13418) with H (m/13417, m/13419, m/13421), I
(m/13416, m/13420) and E (m/13415, enforcement), and D on the Ask bound (m/13422) and the grant
event (m/13431). One Zero Trust standard that every product complies with: a product serves two sockets,
each judged at accept by its own policy, and every call on the functional one carries a principal
that the product itself decides on.

<!-- claim: a-product-serves-two-planes -->
**A product serves two planes, and only two.**

| | Control plane | API plane |
|---|---|---|
| Socket | the instance socket (`{run}/<product>.sock`, fabric's) | the one `access` entry that declares `"plane": "api"` (`{grpc, <contract>, socket, {run}/<name>-api.sock, plane = api}`), discovered through Fabric and read by `discover::Instance::api_plane()` alone (below) |
| Serves | `Lifecycle` (Health, Drain), migrate | `Describe`, the product's own calls (its descriptor), `Events` (Call, Watch), and a read-only twin of `Health` |
| Admits at accept | **fabricd alone** | the product's own policy (below); fabricd for `Describe` only |
| Decides per call | nothing more: fabricd is the only peer | the product's engine, every call (anemnez-consent against `iris/grant`) |
| Reached by | fabricd on this host | local callers, Agora's generated face (§7), Substrate's native mount, and `Fabric.Dial` |

<!-- claim: one-reader-finds-the-api-plane -->
**One declared field marks the API plane, and one function finds it** (B m/13726, K m/13728, D m/13732).
An `access` entry says `"plane": "api"` (absent means it isn't an API plane, so no existing entry changes
meaning). The key restricts: `Access` refuses unknown fields, so an older fabricd can't read it. So a
plane-declaring manifest needs contract N (`PLANE_FROM`), and an older fabricd refuses it by name ("needs
contract N"), never with a parse error (rule 8; D m/13737, m/13746, after M read the code, m/13736). A
product declares it only once its hosts' fabricd read N. A product has at most one. The manifest check
refuses two, `two-api-planes`; an `api` entry that isn't kind grpc via a socket, `api-plane-not-a-socket`;
and one whose socket is the instance (control) socket, `api-plane-is-control`, since `access` never names
the control socket. anemnez-service's `discover::Instance::api_plane() -> Option<&Access>` is the
one reader: fabricd's Describe read (J), Agora's faces host (I), Substrate's native mount (B) and
`product::planes`, which binds the API socket from the same entry, all call it, and none matches on a
name. A product with none stays dumb, said by name. The field travels the wire: `plane` is on
discover.proto's `Access`, and fabricd's declared and discovered copies relay it (readers first), so a
row reads `api_plane()` back from another host's discovery.

`access` never names the control socket. The descriptor describes the API plane alone, so a method
needs no "plane" option: everything `check_descriptor` offers is served on the API plane, and the
control socket registers `Lifecycle` (and `Describe` until step 2, below) and nothing else.

<!-- claim: control-admits-fabricd-alone -->
**Control admits fabricd alone** (I m/13416, D m/12966, m/13418). It is judged at accept by code
identity (anemnez-service `product::peer`), never by a uid or a path. A team-signed CLI is a door,
not an identity: any agent with a shell can run it, so admitting it on control would let any Bash
call drain or migrate any product. A CLI's control verbs go through fabricd (`fabric instance drain
<instance>`), Fabric's guarded path, with Iris kinds for the dangerous ones (IRIS §8.7–8.8). A
team-signed peer that is not fabricd is refused on control by name, `control-is-fabricd-only`.

<!-- claim: the-api-plane-never-serves-lifecycle -->
**The API plane never serves Lifecycle, Drain or migrate**, and the control plane never serves a
functional method. **Health has one source and two readers** (D): fabricd reads it on control, and
agents and products that check health read the same answer through a read-only twin on the API
plane. Both are made by one `Board` (§2), so they can't disagree.

<!-- claim: the-constructor-takes-both-accept-policies -->
**One constructor builds both planes, and it can't be called without both accept policies**
(D, H). anemnez-service's two-plane constructor (`product::planes`; its exact name is its review's)
binds each socket through `product::bind_socket` and judges each connection at accept by the policy
for its plane. Neither policy has a default, and neither defaults to allow. It hands anemnez-extension-serve's
one serving function (`serve`, K's since H moved to Lethe) not a bare listener but **a stream of accepted connections, each
carrying its judged `Peer`** (code identity, from `product::peer`), and -serve puts that peer on every
request beside the principal (H m/13432). The two rules decided per call by the peer then have one
home: -serve's method router refuses fabricd's functional calls (`fabricd-reads-describe-only`), and
the stamp parser reads the peer from there (Agora's stamp is believed only from Agora's peer). No
product re-derives the peer. The constructor is anemnez-service's, behind a feature that takes
anemnez-extension-serve, so a product with no API plane builds without it, and -serve never depends
on anemnez-service (the graph stays acyclic). The `anemnez-product` archetype renders it (E), so a new
product starts compliant. No product binds a product socket with a raw `bind_socket` of its own.

<!-- claim: the-judged-peer-is-on-every-request -->
The boundary between the two halves (K, D, M, 2026-11-16): `Peer`, `Code`, `Role` and `Verdict` are plain data in this
crate (`anemnez_extension::peer`), since both halves already depend on it. Each plane's policy is a function from
`&Peer` to `Verdict::Admit(Role)` or `Verdict::Refuse(name)`, where `Role` is `Fabricd`, `Agora` or `Other`
(`#[non_exhaustive]`). The connection type, `Accepted { io, peer, role }`, lives in -serve beside `serve()`, its one
reader. It implements tonic's `Connected` with `ConnectInfo = Judged { peer, role }`, so tonic puts both on every
request with no code path to forget; a router reads it with `judged(&request)`, and a request that came in on any
other connection reads `None`, an unjudged peer, which a router refuses wherever a judged one is needed. -serve's router acts on the `Role` the accept decided and never holds the trust table
or matches a team or identifier itself. The peer token is read **once, at accept**, and kept with the connection:
macOS answers LOCAL_PEERTOKEN for whoever holds the socket's peer pid when it is asked, so `pidversion` guards
against a reused pid, and nothing reads the token again per request. A peer whose code identity couldn't be read
is admitted only when the plane's policy says so explicitly. Control's policy refuses it by name, `peer's code
identity unreadable`.

<!-- claim: the-api-plane-applies-its-rules-below-every-service -->
**The API plane's per-call rules are one layer, below every service** (K, D m/13648). -serve's
`ApiPlaneLayer` reads the `Judged` peer on each request and answers PERMISSION_DENIED by name before
any service runs: a call on a connection not judged at accept is refused `connection not judged at
accept`, never served as an ordinary caller; fabricd's peer gets `Describe` only, and anything else is
refused `fabricd-reads-describe-only`; and a call that carries a caller's stamp from any peer but
Agora's is refused `stamp-not-from-agora`, not ignored. `api_admits` is that decision, pure, so each
rule has its own row. A role this build doesn't know is not Agora, so its stamp is refused too. Which
principal the call then runs as, and whether a grant covers it, stays the product's decision (below).

<!-- claim: one-function-serves-the-api-plane -->
**One function serves the API plane** (K, 2026-11-16; D m/15034, E m/15035). -serve's `serve(faces, product,
token_checked, incoming, stop)` is the only server a product builds for it. It takes the contract's faces as one
required `ApiFaces` (`Describe`, the read-only `Health` twin from the product's one `Board`, and `Events`), the
product's own calls as `ProductRoutes`, the paths it checks a principal token on, a stream whose items are
`Accepted` connections (so a raw listener doesn't compile), and how it stops. It layers `ApiPlaneLayer`
outermost, then the request digest for `Events` and each token-checked path, and registers nothing of the
control plane's. A product reads the caller with anemnez-service `product::peer::caller`, the stamp's one
reader (M m/14147).
- **A product can't shadow a face or add Lifecycle.** `ProductRoutes::add_service` refuses a service under a
  reserved package (`anemnez.extension.`, which holds Describe, Health and Events, and `anemnez.fabric.`, which
  holds Lifecycle) as `reserved-service`, and one added twice as `service-named-twice`. Both are refused by name
  before any server is built, never with a panic.
- **A stop is bounded** (rule 4). Once the stop's signal resolves, the plane takes no new connection and lets
  the calls in hand finish within the product's drain window, a deadline on `Host` read at each look. Past it,
  each remaining connection is cut, its calls ending unanswered (`drain-window-passed`), and `serve` returns
  `Stopped::Forced { open }`, "forced: N connection(s) with calls in hand past the drain window", never a clean
  stop. A connection still open after a graceful stop is one with a call in hand, since an idle one closes at once.

### 8.1 Who is admitted, and who decides

<!-- claim: one-decision-per-call-at-the-product -->
**One decision per call, at the product that owns the resource** (I m/13416, H m/13419, D). On the
API plane every call is resolved by one engine (anemnez-consent, IRIS §4.4) against `iris/grant`,
in the product. Agora does not decide as well for our products, since two enforcement points on one
call disagree or ask twice, and "identical asks share one card" holds only if one place asks. Agora
keeps deciding for third-party (dumb) servers, which have no plane of their own. A method's
descriptor default stays a floor (§6).

<!-- claim: every-api-call-has-a-principal -->
**Every API call carries a principal.** A Substrate-signed principal token (§1) or, from Agora's
peer only, Agora's stamp (§7 `an-external-caller-is-parsed-here-and-trusted-only-from-its-peer`):
Agora's stamp is believed only when the API plane's accept read Agora's code identity
(`product::peer::caller`, the one place). A stamp from any other peer is refused by name, and each
call records both the peer and the stamp's lane (H m/13080). A door (a team-signed CLI) is admitted
by its signature, and its call's subject is the harness that drove it, resolved through
anemnez-consent. A call with no principal is refused, subject only to §2's rollout rule
(`Keys::admit_absent`).

<!-- claim: a-grant-is-scoped-with-reach -->
**A grant's scope is (product, the method's capability, resource, principal, reach)** (I). The
capability comes from the method's descriptor. Reach is `local` or `dial-from <node>`, so a local
grant never covers a call that came through `Fabric.Dial` (m/12771).

<!-- claim: an-uncovered-principal-is-never-served -->
**A call whose principal no grant covers is refused or asked, never served** (I), and **an Ask raised
on the API plane carries the plane's principal, never the stamp's claimed lane** (IRIS
`a-claimed-lane-grants-nothing`).

<!-- claim: an-ask-is-bounded-in-the-call -->
**An Ask is bounded inside the call, and the bound is in the descriptor** (I m/13420, H m/13421,
D m/13422). A method's options gain `ask_within_secs`, an additive field (H's anemnez-extension
review): how long a call of it may hold an Ask open. An older reader skips it. Absent reads as the
default, **45** (Claude Code's hook hold, the longest a person-facing call waits today), never 0. A
method that never asks says **0**. The standard caps it at **120** (E), and `check_descriptor` refuses
a value past the cap by name, `ask-bound-over-cap`. An unanswered card ends the call `awaiting-consent`,
never a hang. Nested bounds (rule 4): a caller's own per-call bound must exceed the method's
`ask_within_secs` plus its margin. Agora checks it by value at mount and refuses a mount that inverts
them by name (AGORA §5c, the Describe-reading slice). For a long method (`blocks` or
`runs_to_completion`), Agora answers the MCP call at once with a job (§7), so the MCP call never holds
the Ask: the bound checked against `ask_within_secs` is the job's, not the MCP call's (H m/13432).

<!-- claim: refusals-pass-through-by-name -->
**Refusals pass through by name** (H m/13419). `awaiting-consent`, `not-permitted` and the
visible-but-denied reasons reach an MCP caller as the product's own codes, never re-worded.
`mcp::resolve` (§7) carries them once here, so every face gets them.

<!-- claim: describe-answers-per-caller -->
**Describe answers per caller** (I, IRIS §4.3). The descriptor and its digest (§1) stay
caller-independent, and fabricd's SHA read is exactly that. Beside them, the API plane's `Describe`
answers what *this* principal is shown, from the same engine that decides its calls: each method
`Hidden` or `Shown(presence, permission)`, so a caller sees which methods it may call, which ask, and
which are refused and why. A visible denied method still can't be called (`a-denied-call-never-raises-an-ask`).
On the wire it is additive (K m/13694): `DescribeReply.shown` (field 3) holds one `MethodShown { path, hidden |
shown { presence, permission }, why }` per offered method, never part of `digest`, and an older reader skips it.
An empty list means the product doesn't answer per caller, so a reader falls back to the descriptor's own
defaults, and an entry with no `seen` reads as hidden. -serve's `Describe::per_caller` takes a function from the
call (its judged peer and principal are on it) to that list, answered by the product's own engine from what it
already holds. -serve makes the answer whole (D m/15074, E m/15072): one entry per offered method, in the
descriptor's order, a method the function didn't name going out hidden, "not stated by the product", and an
entry for a path not offered dropped. So a per-caller reply is empty only when nothing is offered, and empty
never stands for "show everything". fabricd's peer, which has no principal, always gets the list empty, and the
function is never asked about it.

<!-- claim: a-grant-change-is-an-event -->
**A grant change is an event** (I m/13420, H m/13421, D m/13431, m/13436). So a per-caller face is
cached by (descriptor SHA, principal) and re-read only when it changes. The event is iris-client's
`grants_changed()`, a watch on Iris's own grant records, with the grant key's encoding kept in Iris
(iris-kernel `keys::GrantKey`). It says each write or delete as `Changed`. After a gap it re-lists,
says what changed during the gap, then `Resynced`, and the reader re-reads every face for that
principal, so it never assumes nothing changed across a gap. A stale face is only noise, never
authority: the product's per-call decision is the authority.

<!-- claim: dial-lands-only-on-the-api-plane -->
**`Fabric.Dial` lands only on the API plane** (D), checked against the product's own record (its
`access` entry), never a path compare.

<!-- claim: fabricd-dials-the-api-plane-for-describe-only -->
**fabricd may dial the API plane for `Describe` only** (H m/13417, D). Its discovery read of the
descriptor SHA is caller-independent. A functional call from fabricd's peer is refused by name,
`fabricd-reads-describe-only`.

### 8.2 Describe moves in two steps (rule 8)

<!-- claim: describe-moves-to-the-api-plane-in-two-steps -->
**Step 1:** for one release a product serves `Describe` on both planes. fabricd's SHA read, Agora's
Describe read (built against the API plane from the start, H) and Substrate's native mount move to
the API plane. **Step 2:** the control socket drops `Describe`. A product adopting §8 for the first
time (Minion) serves it on the API plane alone from the start. Every other product's step 2 ends
2027-01-15 (owner K, from H), once each reader dials the API plane: fabricd's SHA read (D, with J), Agora's
Describe read (H) and Substrate's native mount (B).

### 8.3 The drain machine: control outlives API

<!-- claim: control-outlives-api-in-a-drain -->
**In a drain the control plane outlives the API plane** (D). This is one machine, SHARED row 9's
anemnez-service drain machine (owed there, owner D; today each product has its own copy), never per
product. A sans-IO kernel, the product's host does the I/O:

| State | Means | Leaves on |
|---|---|---|
| `Serving` | both planes open | fabricd's `Drain` accepted → `ApiDraining` |
| `ApiDraining { in_hand, deadline }` | the window is what fabricd's deadline leaves once everything after it is set aside (invariant 6); the API plane refuses new calls `draining` (UNAVAILABLE, retryable: the caller goes to another instance); in-flight calls run to their end; each open `Events` stream gets its last frames and then `end` | `in_hand` reaches 0 → `ApiClosed` (`Handing` when a hand-off is declared); `deadline` passes → `Cancelling` |
| `Cancelling { in_hand, cancel_deadline }` | every call still in hand has its work cancelled at its next cancellation point, not only its reply; its caller is told `drain-window-passed`, an *unanswered* outcome (Unclear: it may or may not have applied, rule 5), never "failed"; the overrun is named in Health. `cancel_deadline` is `cancel_within` (short) past entry, on `Host`, and never later than fabricd's deadline less the reply margin (invariant 6) | the last cancelled call's work has stopped → `ApiClosed` (`Handing` when a hand-off is declared); `cancel_deadline` passes → `WorkStuck` |
| `WorkStuck { calls }` | work that never reached a cancellation point (a blocking syscall, a hung child) still runs; the machine never answers `DRAINED`, and Health names each call whose work didn't stop | its work stops → `ApiClosed` (`Handing` when a hand-off is declared); otherwise fabricd's own stop bound ends the process as a **forced** stop, recorded as such in fabricd's record (`forced: work didn't stop: <calls>`), never as a clean drain |
| `Handing { deadline }` | a product that declares a pre-close hand-off (a resign through the shared Holder, rule 6; a flush) enters it instead of `ApiClosed`, by each of the three edges that empty the API plane: from `ApiDraining`, `Cancelling` or `WorkStuck`. The API listener closes on entering it, so nothing a call started runs while it waits. The machine starts the hand-off and waits, bounded by `handing_within` past entry on `Host`, never later than fabricd's deadline less the reply margin (D m/13557, K m/13574) | it is done → `ApiClosed`; it is Unclear (a resign unanswered: the lease's own expiry bounds it) or `deadline` passes → `ApiClosed` with the hand-off unfinished, and Health's `DRAINED` names why, `hand-off unfinished: <why>` |
| `ApiClosed` | the API listener is closed, and nothing a call started still runs; control still answers | the machine answers `DRAINED` to a Health read, moving to `ControlDrained` in that answer → `ControlDrained` |
| `ControlDrained` | fabricd may stop the process | the control listener closes (terminal) |

Events in: `DrainAccepted { within }`, `CallStarted`, `CallEnded`, `StreamEnded`, `DeadlinePassed`,
`WorkStopped`, `CancelDeadlinePassed`, `HandOffEnded`, `HealthRead`. Actions out: refuse, close the API listener, cancel the remaining calls'
work and tell each caller Unclear by name, start the hand-off, answer `DRAINED`, close control. The kernel returns its deadline, read on the `Host` clock, since a laptop may sleep through a drain (rule 3). The window is
`min(drain_window_secs, fabricd's deadline − (cancel_within + handing_within + reply margin))`: fabricd's deadline is the
outer bound, and everything the product does after the window must fit inside it (nested bounds, rule 4; D m/13557).
`DrainAccepted`'s `within` is fabricd's deadline as a span from acceptance, read on `Host` (`DrainRequest.deadline_secs`);
with none named (0), the window is `drain_window_secs`. A deadline at or under `cancel_within + handing_within + reply
margin` leaves no window: the calls in hand go straight to `Cancelling`, and the Drain's answer says so by name,
"fabricd's deadline leaves this product no drain window" (D m/13571), so a fabricd bound too short for the product is
seen as a configuration fact, never as calls cancelled for no reason. Invariants:

1. Control never closes while the API plane holds a call or a stream.
2. Health says `DRAINED` only in `ControlDrained`, so fabricd never stops a product that has work in
   hand.
3. After `Serving`, the API plane admits no new call. The only way back is a new process.
4. A deadline passing is a named end (`drain-window-passed`, an unanswered outcome), never a silent
   drop, a hang, or a call reported failed.
5. Nothing a call started still runs in `ControlDrained`, and a stop with work still running is
   forced, and named (D m/13436, m/13445): fabricd's stop never passes off cut work as a clean drain.
   Every state that waits has a deadline (`deadline`, `cancel_deadline`, the hand-off's
   `deadline`, then fabricd's stop bound), so no drain holds for ever (rule 4).
6. No deadline the machine returns is later than fabricd's own (D m/13557), so the machine answers before fabricd's
   stop, and a forced stop happens only when work really is stuck. It holds by construction, not only when there is
   room (K m/13574): every deadline the machine sets is `min(its own bound, fabricd's deadline − reply margin)`.

### 8.4 Enforcement (E)

<!-- claim: every-product-is-held-to-two-planes -->
**Every product is held to §8 by its gate, without a flag day** (E m/13415).

1. **Static rows in `prova-project-standards`**, in every Herald-gated repo from its next tag:
   `access` never names the instance socket; every descriptor method states `callers`, `permission`
   and (once H's field lands) `ask_within_secs`, within the cap; the control socket's router registers
   only `Lifecycle` (and `Describe` until step 2); a product socket is bound only through the two-plane
   constructor. Each starts as a dated `ends` per repo, then is red by name.
2. **A shared Prova package, `prova-planes`**, run by each product's own hermetic harness against its
   own test daemon, never the host's live Fabric (rule 10). One package, plugged in per product as a
   predicate (rule 2). Rows:
   - a team-signed non-fabricd peer is refused on control (`control-is-fabricd-only`);
   - a peer whose code identity is unreadable is refused on control by name (`peer's code identity unreadable`);
   - on the API plane, a peer whose code identity is unreadable is admitted or refused only as the product's policy
     says, explicitly, never by a default;
   - each plane reads the peer token once, at accept: a request on a connection whose peer pid was reused by
     another process is still judged as the peer that connected;
   - `Lifecycle` and `Drain` are absent from the API plane;
   - a call with no principal is refused;
   - a call whose principal no grant covers is refused or asked, never served;
   - an Ask raised on the API plane carries the plane's principal, not the stamp's claimed lane;
   - a stamp from a peer that is not Agora is refused;
   - fabricd's peer gets `Describe` and is refused a functional call;
   - a drain refuses new API calls, finishes those in hand, and reports `DRAINED` only after the API
     plane is closed;
   - a call holding past the drain window is told Unclear, and its effect, read back by value after
     the stop, is either whole or absent;
   - work that never reaches a cancellation point holds the machine past `cancel_within`
     not-DRAINED, Health names the call, and the stop that follows is recorded forced, by name;
   - a declared hand-off runs after the API plane closes and before `DRAINED`; one left unanswered past its
     bound still drains, and `DRAINED` names it unfinished;
   - no deadline the drain machine keeps is later than fabricd's own, including for a Drain whose deadline is
     shorter than `cancel_within + handing_within`;
   - a Drain whose deadline leaves no window is answered "fabricd's deadline leaves this product no drain window",
     and its calls in hand are cancelled and told Unclear at once.
3. **The archetype** (`anemnez-product`) renders the constructor and the package's wiring (E's owed tag).
4. **Themis** asks at plan and review: which plane, who is judged at accept, and which grant decides
   the call. The answers are recorded in the hearing (E words the question).

**Adoption order:** Minion first (its control surface is built to §8; its gate proves `prova-planes`
before any other product's), then Typhon, then Iris (irisd's Raise, Answer and Get move off the
instance socket). Owed, each with its owner: the constructor (D and H review), -serve's serving
function and `ask_within_secs` (H), `grants_changed()` (I), the static rows,
`prova-planes`, the archetype and the Themis question (E), and the drain machine (D, SHARED row 9).
