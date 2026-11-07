--- The deputed unit-test leg: `prova run ut` conducts cargo nextest ONCE for the whole workspace —
--- compilation and execution both live in a file-scoped fixture that emits one junit artifact —
--- then one proof adopts every case into the account, and sibling readers can bind claims to
--- named cases at zero additional compilations (conduct once, read many).
---
--- The `ut` switch (suite.lua) is intent; `cargo-nextest` stays a `requires` world fact: two facts
--- with two remedies. The profile `must_run`s the deputy, so `prova run ut` fails rather than
--- skips when nextest is missing — a profile is a contract, not a courtesy.
---
--- A workspace with ZERO unit tests reads RED here, deliberately: `prova run ut` asked for the
--- unit-test account and there is none to adopt. On a legacy project that has never written unit
--- tests, that red is the leg telling you where to start — not a gate to weaken.

-- Conduct the deputy once. The stale artifact is removed FIRST, so a deputy that dies before
-- emitting (a compile error) leaves nothing behind and the adoption fails loudly on "matched
-- nothing" — never a previous run's verdicts wearing this run's face. The deputy's exit code is
-- deliberately not asserted here: the adopting proof reports red with the deputed cases' own
-- names, which a fixture death would hide.
-- The `prova` profile (junit emission, fail-fast off) ships in .prova/nextest.toml and reaches
-- nextest as TOOL config — composing under, never colliding with, the project's own
-- .config/nextest.toml. A repo-defined [profile.prova] would win, deliberately.
local deputy = prova.fixture("nextest-junit", Scope.File, function()
	local artifact = prova.root .. "/target/nextest/prova/junit.xml"
	fs.remove_all(artifact)
	shell.run(
		{ "cargo", "nextest", "run", "--workspace", "--profile", "prova",
			"--tool-config-file", "prova:" .. prova.root .. "/.prova/nextest.toml" },
		{ cwd = prova.root, merge_stderr = true, timeout = "1800s" }
	)
	return artifact
end)

prova.test("the workspace's unit-test account holds — every nextest case adopted", {
	requires = { "cargo-nextest" },
	-- The house rule, framework-enforced (prova learn locks): cargo takes process-wide locks
	-- of its own, so every conduct that compiles declares the writer hold — bound across every
	-- prova instance at this home, so two agents and a human never race a build.
	locks = { prova.writes("cargo") },
}, function(t)
	junit.verify(t, { results = t:use(deputy) })
end)

-- Readers: one claim, one named unit test — a prose claim discharged by a specific case in the
-- deputy's own account, without a second run. As claims accrete, bind them like this:
--
--   prova.test("the parser rejects unterminated input, spoken for by its unit test", {
--   	requires = { "cargo-nextest" },
--   }, function(t)
--   	local report = junit.load(t:use(deputy))
--   	for _, c in ipairs(report.cases) do
--   		if c.name == "parser::tests::rejects_unterminated_input" then
--   			t:expect(c.outcome, c.name):equals("passed")
--   			return
--   		end
--   	end
--   	t:expect(false, "parser::tests::rejects_unterminated_input is missing from the account"):is_true()
--   end)

-- The serving face's claims (CONTRACT.md §2; substrate docs/plans/ASYNCIFICATION.md names the
-- `async-…` ones), each spoken for by its named case in the deputy's account.
local function spoken_for(claim, case)
	prova.test(claim .. ", spoken for by " .. case, {
		requires = { "cargo-nextest" },
		locks = { prova.writes("cargo") },
	}, function(t)
		for _, c in ipairs(junit.load(t:use(deputy)).cases) do
			if c.name == case then
				t:expect(c.outcome, c.name):equals("passed")
				return
			end
		end
		t:expect(false, case .. " is missing from the account"):is_true()
	end)
end

spoken_for("async-stream-never-awaits-a-subscriber", "serve_tests::a_publisher_never_waits_on_a_subscriber_that_never_reads")
spoken_for("a-stream-ends-at-its-tokens-expiry-on-the-injected-clock", "serve_tests::a_stream_ends_with_expired_at_its_tokens_expiry_on_the_injected_clock")
spoken_for("async-cancelled-only-by-its-owner", "serve_tests::a_dropped_subscriber_cancels_nothing")
spoken_for("health-is-state-with-parts", "serve_tests::a_watcher_reads_health_then_sees_only_later_changes")
spoken_for("a-product-pushes-events-as-a-stream-it-serves", "serve_tests::a_foreign_cursor_is_not_mine_and_an_old_one_is_out_of_range")
spoken_for("a-keyed-route-names-a-string-field", "options::tests::a_keyed_route_names_a_top_level_string_field")
spoken_for("the-serving-face-runs-the-whole-principal-ladder", "principal_tests::each_step_refuses_by_its_own_name")
spoken_for("the-serving-face-runs-the-whole-principal-ladder (replay)", "principal_tests::a_token_accepted_once_is_refused_replay_after")
spoken_for("a-nonce-is-durable-before-the-method-runs", "principal_tests::the_verifier_answers_only_after_the_nonce_is_recorded")
spoken_for("a-token-binds-the-bytes-as-received", "serve_tests::a_request_with_a_field_this_build_does_not_know_still_verifies")
spoken_for("a-token-binds-the-bytes-as-received (in flight)", "serve_tests::the_refusals_reach_a_grpc_caller_by_name")
spoken_for("the-verifier-applies-retired-versus-revoked", "principal_tests::a_revoked_key_refuses_even_a_backdated_token_and_a_retired_key_only_later_ones")
spoken_for("the-verifier-applies-retired-versus-revoked (units)", "principal_tests::a_record_time_in_the_wrong_unit_is_refused_as_implausible")
