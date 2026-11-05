-- The starter suite a retrofit can honestly ship: one true fact, and one promise.
--
-- The retrofit knows nothing about what this project DOES, so it does not pretend to prove any
-- behavior. What it can prove today is that the artifacts exist to be driven; what the project
-- owes is stated as a PROMISE — an executable spec that reports as its own outcome (PROMISED,
-- green in CI, visible in `prova owed`) until a real body proves it, at which point it demands
-- graduation. Replace the promise with proofs that drive what this project actually does:
-- black-box, through the built artifacts (`lib.build` + `lib.bin("<name>")`), never through
-- the internals.

local lib = require("lib") -- the project's shared library package (.prova/packages/lib)

prova.test("the workspace builds — the artifacts black-box proofs drive exist", {
	requires = { "cargo" },
	locks = { prova.writes("cargo") },
}, function(t)
	t:use(lib.build)
end)

prova.test("the project's behavior is proven black-box, through its real artifacts", {
	promises = "prova init rust-project rendered this placeholder — replace it with proofs that drive what this project actually does",
}, function(t)
	-- The promised body fails today, by design; the moment it passes, prova fails it demanding
	-- graduation (change `promises` to `proves = "<context>"`). Write the real thing: boot the
	-- binary, hit its surface, assert on what a user of this project would observe.
	t:expect(false, "no black-box proofs of this project's behavior exist yet"):is_true()
end)
