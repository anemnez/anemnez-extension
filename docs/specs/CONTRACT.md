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
`unbounded-collection` (an output with a repeated field declared `one`), `effect-missing`,
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
serving face onto the log, and removes `Events(after)` and the Journal. Ends 2026-12-15 (owner H).

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
yet, so nothing waits on it today. Ends 2026-12-15 (owner H).

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
