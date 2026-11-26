--- The contract's Swift client (swift/, AnemnezEvents; ERGON-UI.md §4a, M m/33097): its own tests, run in the ut leg
--- so a Swift change is gated like a Rust one. They include the envelope's proto staying a link to the crate's, and
--- the envelope carrying the wire's bytes exactly as -serve's prost does, read from the shared vector that a Rust row
--- holds to prost's own encoding (testdata/events-envelope.json); and the watch link's rows and its walk of event
--- orders (WatchLink, ergon ERGON-UI.md the-facts-watch-wakes-the-glass-by-cursor).
---
--- Hermetic through the standards' one SwiftPM helper (standards v2.18 `swiftpm`, SHARED row 44): the pins resolved
--- first under their own bound from the host's cache, named as infrastructure when that fails, then the tests with
--- automatic resolution off.

local standards = require("project-standards")

prova.test("AnemnezEvents' Swift tests pass", {
	requires = { "swift", "protoc" },
	-- Builds the Swift package: a writer under the build house rule.
	locks = { prova.writes("swift") },
	-- Everything the leg reads: the package, its pins, the Swift sources and tests, the crate's protos it links, and
	-- the shared vector. A train touching none of these carries the last pass forward and never runs the leg.
	inputs = { "Package.swift", "Package.resolved", "swift/**", "crates/anemnez-extension/proto/**", "testdata/**" },
}, function(t)
	standards.swiftpm.test(t, { root = prova.root })
end)
