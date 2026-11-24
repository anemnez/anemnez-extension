# Orientation: a product's guidance as an affordance

**Status:** v0.2, design; v0.1 approved by D, I, O, K and C (m/29758, m/29760, m/29762, m/29765, m/29766), with their notes folded in here | **Owner:** lane H (the User, 2026-11-24) | **Reviewers:** K (this contract),
D (architect), I (Agora), C (affordances and presence), O (Ergon, the first product to carry it)

## 0. In one paragraph

An agent that starts fresh (a new session, a `/clear`, a compaction) should be oriented by the products
around it, mechanically, and cheaply. Each product already describes its methods (CONTRACT.md §1). It
gains one more affordance: its **guidance**. Guidance has two halves:
- **a static half:** what the product is, what it is to an agent, and how to learn the rest just in time;
- **a volatile half:** what matters to this caller now, as counts and pointers, answered by one bounded
  method.

The static half sits where a prompt cache keeps it, and the volatile half sits behind one call. So a
`/clear` costs a cached prefix and one call, and the hand-off note thins to what only the agent knows.
Guidance has a presence of its own, apart from its methods' presence. A product whose tools are Minimal
but whose guidance is Full is understood at once, and its tools are learned through their help when
they're needed.

## 1. Why, by value

- **Lethe re-derived other products' state at every clear** (lethe `docs/specs/LETHE.md`): Horai's owed,
  Stoa's posts and working copies, read and stamped by a third party. Each product already knows its own
  "now". Asking each product directly is one implementation of each thing (rule 2), not a copy per
  harness.
- **The cost of a clear is what can't be cached.** Substrate orders a window so that what is stable comes
  first and what changes comes last (substrate `docs/specs/CACHE_LAYERING.md`, layers L0 to L5). An MCP
  client's prefix is the same: the system prompt, the tools and each server's `instructions`. Text that
  is byte-stable per build costs nothing to carry across a clear. A count or a timestamp in it breaks
  the cache on every turn.
- **Presence already exists, twice.** Methods carry Substrate's four tiers (CONTRACT.md §1, `presence`),
  and Agora keeps its own per-server `full | brief | omitted` for outside clients (agora `AGORA.md`,
  `presence-is-an-outside-clients-view`). Guidance gives both one model.

## 2. The guidance affordance

<!-- claim: guidance-is-a-service-option -->
**A product's guidance is a service option, written for the agent that reads it.** `ServiceOptions`
gains `guidance`: plain text of at most 1,200 characters. It is per product, not per contract: it sits
on one service in the descriptor, the product's own, so a product with two contracts says itself once
(K, I). Its **first line** is its summary, at most 160 characters: the Minimal line, split by rule and
never by finding a sentence's end (C). `check_descriptor` refuses `guidance-twice`, `guidance-too-long`
and `guidance-summary-too-long`, by name. It says:
- what the product is;
- what it is to an agent working beside it (when to reach for it, what it owns);
- how to learn the rest (each tool's help, `system.describe`, `substrate.help`).

It names no state: no counts, no dates, no hosts. It is part of the descriptor, so it changes only with
the build, and the digest says when it changed. Its quality is a proof in the product's gate, as a
method's `description` is.

<!-- claim: orient-is-one-bounded-read -->
**A product may mark one method `orient`: what matters to the caller now.**
- It is unary and takes no argument (O): it renders the same for the same caller and state, and a focused
  read is the product's own `get`. It has `effect: read` and exposure `Agent`.
- Its bounds are the contract's existing options (K), plus one cap of its own:
  - its result is `bound`, the count of replies, which is a count and never characters. One reply is
    `bound: 1`. `check_descriptor` refuses an orient with no bound, or one past 1,500;
  - its text, the reply itself, is at most 1,500 characters (`ORIENT_BOUND_MAX`), capped by the product,
    and a cut text says it was cut and where the rest is (AGENTIC_EXPERIENCE.md AX6; Ergon's orient, O);
  - its time is its `ask_within_secs`, the one number a fan-out's inner bound reads.
- It answers about the authenticated caller (its stamp, CONTRACT.md §7), never a lane named in its input.
  On a carried call that is the claimed lane, once the product lists `orient` in its ClaimedMethods.
  Until then a carried orient answers as for an unattributed caller, never someone's book (K). A claim is
  enough here because an orient reply is read-only and grants nothing (D). No face may take it as proof of
  who is asking.
- It carries counts and pointers ("2 owed: `ergon owed`; active: ergon:H/x"), never history and never
  another product's state.
- It never answers empty, because empty reads as "nothing to do" (O). A caller with nothing yet is said so
  ("no book here yet: `ergon add` opens one"). A product that can't answer right now refuses by name and
  says why ("ergon: no leader right now, reloading"), as a Refuse-with-Teaching (anemnez-service
  `AGENTIC_EXPERIENCE.md` AX3).

`check_descriptor` refuses a second `orient`, one that writes, and one with required fields, each by name.

<!-- claim: guidance-has-its-own-presence -->
**Guidance has a presence of its own** (the four tiers). The default is Full. In Substrate it is an
affordance row keyed by the namespace and a reserved method name, `<ns>.guidance`. It is resolved by
`cos-kernels-presence` like any row, through the three stores (Agent Defaults, Agents, Roles) and their
overrides, so there is no new store and no new settings path (C). `orient` has its own row: guidance at
Hidden doesn't hide `orient`, and `orient` at Hidden doesn't hide guidance (C).
- **Full:** the static half is in the prefix, and `orient` is reached.
- **Minimal:** the guidance's first line, beside the product's Minimal tool summaries.
- **Index:** the product's name in an index of what is present.
- **Hidden:** nothing.

A method's presence is unchanged by it. Guidance at Full over tools at Minimal is the intended default
for a product an agent works beside but seldom calls.

## 3. The cache rule

<!-- claim: the-static-half-is-byte-stable -->
**The static half goes only where a cache keeps it, and is byte-stable per build, built from descriptors,
never from liveness (I):** an MCP server's
`instructions`, or Substrate's profile and bootstrap layers. **The volatile half is reached only by a
call.** Its answer lands in the conversation's history, where it is cached from then on, and never in a
prefix. A face that puts a count, a time or a live read into the static half is red by a row that
renders it twice, across a change of state and across a product going down and back, and compares the
bytes. The row lives once, in anemnez-extension-serve, and each product's gate runs it (K).

"Byte-stable per build" doesn't mean only per build. A change to guidance's presence, or to the products
present, is a settings change, as a method's presence is. It re-renders the prefix, deliberately, at
the next turn boundary and never mid-request (C).

## 4. The faces

**Agora (I).** Guidance at Full over Minimal tools gives this shape: the server's `instructions` are its guidance,
and its tools ship with one-line descriptions. Agora's `instructions` are every present product's
guidance, in the servers' stable-key order, built from the descriptor Agora last read for each product.
A client reads them once, at initialize. A product that is down at a session's start keeps its block, so a
restart never changes the prefix (I).

The prefix grows with the suite: fifteen products at Full would be about 18k characters. So Agora keeps
guidance presence per caller, the per-caller faces it already keeps. If step 7 finds that a client caps a
server's instructions, the default is Minimal guidance (each product's first sentence) for every
product, and Full only for the products a caller works beside (I).

Agora offers one tool of its own, `agora__orient`:
- it calls each present product's `orient` at once, with Agora's stamp for that MCP caller, never a lane
  from the tool's input. A product on another host gets the stamp carried, and its plane decides what it
  is worth (I);
- it is bounded twice, nested and inner shorter (rule 4): each product's call by that product's
  `ask_within_secs`, all of them by one total, Agora's tool bound minus a margin (D, I);
- it answers one block, ordered by the servers' stable keys;
- a product that doesn't answer in time is said by name ("ergon: no answer in 2 s"), and so is one that
  refused fast ("ergon: no leader right now, reloading"). Neither is left out as though it had nothing,
  and nothing is awaited past the total (D, O).

Agora's `full | brief | omitted` becomes guidance's presence for outside clients, so one model
remains: full is Full, brief is Minimal and omitted is Hidden. The instructions name `agora__orient`
first, so a fresh agent's first call orients it.

**Substrate (B's bridge, C's affordances).** A product's guidance is the system's description in
`system.describe` and `substrate.help(namespace)`, and heads its Minimal block. `orient` is an
affordance row like any method, resolved by `cos-kernels-presence` with no new tier. Substrate's own
prime (cos-daemon `continuity_prime.rs`) may call `orient` for the products present, in place of reading
their state itself. It calls as the agent, with the agent's own claim through the bridge, never as
substrated: otherwise every agent would be oriented as the daemon. The answer enters as a turn input in
the history, never in the profile or bootstrap layers. Its bound sits inside the prime's own (rule 4),
and a product past it reads "not answered within Ns". The row: two agents' primes get their own answers,
and the prefix's bytes don't change across a change of state (C).

**Lua.** `substrate.help("<namespace>")` leads with the guidance.

## 5. The hand-off, thinned

With guidance in place, what a fresh agent can't learn from the products around it is only its own
judgment: why it chose what it chose, what is half-done and must not be redone, what a person said that
it may not have answered, and what it ruled out. That stays in the agent's memory and, for its work,
in Ergon. Lethe retires once a `/clear` orients through Agora alone (§6, step 7). Substrate keeps its
own continuity for its own seats (B's Seats).

## 6. Steps (each with one owner, and the shared crate it lands in)

1. **This design agreed:** H, with K, D, I, C and O.
2. **The contract, readers first (K stewards):**
   - additive fields in `options.proto`: `ServiceOptions.guidance` and the `orient` mark;
   - `check_descriptor`'s refusals (`guidance-twice`, `guidance-too-long`, a second or a writing
     `orient`, an `orient` with no `result: Bound`);
   - `mcp::flatten` carrying guidance beside the tools;
   - the byte-stability row in -serve;
   - ClaimedMethods for a carried orient (K's -serve head, which lands first).

   The kernel, `check_descriptor` and `flatten` release first, the faces read them, and only then do
   products set them (rule 8).
3. **Agora's face:** instructions from guidance, `agora__orient`, and the presence mapping (agora; I).
4. **Ergon first:** its guidance text and its `orient` (ergon; O).
5. **Then the others, each by its owner:** Stoa and HydraMQ (A), Horai and Herald (L, with E), Prova
   and Archetect (E), Themis and Ladon (G), Typhon (K), Prosopon (N), Iris (I), Artichoke (F) and Fabric (D, J).
6. **Substrate's bridge:** guidance in `system.describe` and `substrate.help`, and `orient` as an
   affordance (B's bridge, C's presence).
7. **The proof, through the reader's path:** a fresh Claude Code session after a `/clear` is oriented
   from its cached prefix and one `agora__orient` call. Measured: the prefix's cache reads, the calls it
   takes to orient, and the tokens. Then Lethe is unwoven from the launcher (H, with G).
8. **Themis (G):** a product's plan hearing asks for its guidance, and Themis carries its own.

## 7. Open

- Ruled: guidance is per product (I, K), and `orient` takes no argument (O, C). Add a focus only if step 7
  shows that a second call costs.
- What should the bounds be? 1,200 and 1,500 characters are a first guess. Step 7 measures them, along
  with what a client keeps of a server's instructions (I).
