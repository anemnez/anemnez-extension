# Orientation: a product's guidance as an affordance

**Status:** design, for review | **Owner:** lane H (the User, 2026-11-24) | **Reviewers:** K (this contract),
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
gains `guidance`: plain text of at most 1,200 characters. It says:
- what the product is;
- what it is to an agent working beside it (when to reach for it, what it owns);
- how to learn the rest (each tool's help, `system.describe`, `substrate.help`).

It names no state: no counts, no dates, no hosts. It is part of the descriptor, so it changes only with
the build, and the digest says when it changed. Its quality is a proof in the product's gate, as a
method's `description` is.

<!-- claim: orient-is-one-bounded-read -->
**A product may mark one method `orient`: what matters to the caller now.**
- It is unary, with no required field, `effect: read` and exposure `Agent`.
- It answers about the authenticated caller (its stamp, CONTRACT.md §7), never a lane named in its input.
- Its reply is at most 1,500 characters. It carries counts and pointers ("2 owed: `ergon owed`;
  active: ergon:H/x"), never history and never another product's state.
- It answers within its bound (§4). A product with no "now" for the caller answers empty, never a
  placeholder.

`check_descriptor` refuses a second `orient`, one that writes, and one with required fields, each by name.

<!-- claim: guidance-has-its-own-presence -->
**Guidance has a presence of its own** (the four tiers). The default is Full.
- **Full:** the static half is in the prefix, and `orient` is reached.
- **Minimal:** one line, the guidance's first sentence, beside the product's Minimal tool summaries.
- **Index:** the product's name in an index of what is present.
- **Hidden:** nothing.

A method's presence is unchanged by it. Guidance at Full over tools at Minimal is the intended default
for a product an agent works beside but seldom calls.

## 3. The cache rule

<!-- claim: the-static-half-is-byte-stable -->
**The static half goes only where a cache keeps it, and is byte-stable per build:** an MCP server's
`instructions`, or Substrate's profile and bootstrap layers. **The volatile half is reached only by a
call.** Its answer lands in the conversation's history, where it is cached from then on, and never in a
prefix. A face that puts a count, a time or a live read into the static half is red by a row that
renders it twice, across a change of state, and compares the bytes.

## 4. The faces

**Agora (I).** Guidance at Full over Minimal tools gives this shape: the server's `instructions` are its guidance,
and its tools ship with one-line descriptions. Agora offers one tool of its own, `agora__orient`:
- it calls each present product's `orient` at once, each bounded inside Agora's own bound (inner
  shorter, rule 4);
- it answers one block, ordered by the servers' stable keys;
- a product that hasn't answered within its bound reads "not answered within Ns" (Unclear), never a hang
  and never "nothing".

Agora's `full | brief | omitted` becomes guidance's presence for outside clients, so one model
remains: full is Full, brief is Minimal and omitted is Hidden. The instructions name `agora__orient`
first, so a fresh agent's first call orients it.

**Substrate (B's bridge, C's affordances).** A product's guidance is the system's description in
`system.describe` and `substrate.help(namespace)`, and heads its Minimal block. `orient` is an
affordance row like any method, resolved by `cos-kernels-presence` with no new tier. Substrate's own
prime (cos-daemon `continuity_prime.rs`) may call `orient` for the products present, in place of reading
their state itself.

**Lua.** `substrate.help("<namespace>")` leads with the guidance.

## 5. The hand-off, thinned

With guidance in place, what a fresh agent can't learn from the products around it is only its own
judgment: why it chose what it chose, what is half-done and must not be redone, what a person said that
it may not have answered, and what it ruled out. That stays in the agent's memory and, for its work,
in Ergon. Lethe retires once a `/clear` orients through Agora alone (§6, step 7). Substrate keeps its
own continuity for its own seats (B's Seats).

## 6. Steps (each with one owner, and the shared crate it lands in)

1. **This design agreed:** H, with K, D, I, C and O.
2. **The contract:** `ServiceOptions.guidance`, the `orient` mark, `check_descriptor`'s refusals, and
   `mcp::flatten` carrying guidance beside the tools (anemnez-extension; K stewards, H builds).
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

- Is guidance per contract or per product, for a product that offers two contracts? (K)
- Should `orient` take an optional `focus` (an item, a branch), or stay argument-free? (O, C)
- What should the bounds be? 1,200 and 1,500 characters are a first guess. Step 7 measures them.
