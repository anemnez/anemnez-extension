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
spoken_for("a-stream-asked-to-end-by-the-drain-ends-draining", "serve_tests::a_stream_asked_to_end_by_the_drain_sends_what_it_holds_then_ends_draining")
spoken_for("an-issuers-record-is-read-once-per-key-age", "key_cache::tests::a_held_record_is_used_until_its_age_then_read_again")
spoken_for("an-issuers-record-is-read-once-per-key-age", "key_cache::tests::an_age_passed_while_asleep_reads_again")
spoken_for("an-issuers-record-is-read-once-per-key-age", "key_cache::tests::a_records_age_runs_from_its_ask")
spoken_for("an-issuers-record-is-read-once-per-key-age", "key_cache::tests::concurrent_lookups_make_one_read_and_join_its_flight")
spoken_for("an-issuers-record-is-read-once-per-key-age", "key_cache::tests::a_flight_past_its_deadline_is_gone_and_its_answer_dropped")
spoken_for("an-issuers-record-is-read-once-per-key-age", "key_cache::tests::a_late_answer_never_overwrites_a_newer_record")
spoken_for("an-issuers-record-is-read-once-per-key-age", "key_cache::tests::a_stale_record_whose_refetch_is_unclear_is_never_used")
spoken_for("an-issuers-record-is-read-once-per-key-age", "key_cache::tests::unclear_is_never_a_record_and_is_retried_after_its_backoff")
spoken_for("an-issuers-record-is-read-once-per-key-age", "key_cache::tests::a_refusal_is_answered_from_the_cache_for_its_age_counted_in_reads")
spoken_for("an-issuers-record-is-read-once-per-key-age", "key_cache::tests::a_key_id_the_record_lacks_reads_again_at_most_once_per_second")
spoken_for("an-issuers-record-is-read-once-per-key-age", "key_cache::tests::held_records_are_bounded_and_the_least_recently_used_goes")
spoken_for("an-issuers-record-is-read-once-per-key-age", "key_cache::tests::a_flood_of_unknown_issuers_never_evicts_a_held_record")
spoken_for("an-issuers-record-is-read-once-per-key-age", "key_cache::tests::flights_past_their_cap_are_unclear_at_once")
spoken_for("an-issuers-record-is-read-once-per-key-age", "key_cache::tests::an_outage_costs_one_read_per_second_per_issuer_not_one_per_call")
spoken_for("an-issuers-record-is-read-once-per-key-age", "key_cache::tests::a_backoff_never_shadows_a_fresh_record")
spoken_for("an-issuers-record-is-read-once-per-key-age", "key_cache::tests::backoffs_are_bounded_and_never_make_an_issuer_known")
spoken_for("an-issuers-record-is-read-once-per-key-age", "key_cache::tests::a_lapsed_backoff_is_dropped_by_the_lookup_that_meets_it")
spoken_for("an-issuers-record-is-read-once-per-key-age", "principal_tests::tokens_at_once_from_one_issuer_make_one_read")
spoken_for("an-issuers-record-is-read-once-per-key-age", "principal_tests::tokens_waiting_on_a_hung_read_are_unclear_at_its_deadline")
spoken_for("an-issuers-record-is-read-once-per-key-age", "principal_tests::a_host_asleep_past_the_key_age_reads_the_record_again_and_sees_a_revoke")
spoken_for("an-issuers-record-is-read-once-per-key-age", "key_cache_sim_tests::tokens_on_one_flight_are_admitted_on_one_read_or_unclear_at_its_deadline_and_each_seed_replays")
spoken_for("a-stream-asked-to-end-by-the-drain-ends-draining", "serve_tests::the_events_service_ends_a_stream_by_the_end_in_its_request")
spoken_for("a-stream-asked-to-end-by-the-drain-ends-draining", "serve_tests::a_draining_stream_ends_at_the_journal_s_place_when_asked_however_busy")
spoken_for("a-products-own-stream-ends-at-its-drain", "journal::streams_tests::a_products_stream_ends_draining_when_its_drain_says")
spoken_for("async-cancelled-only-by-its-owner", "serve_tests::a_dropped_subscriber_cancels_nothing")
spoken_for("health-is-state-with-parts", "serve_tests::a_watcher_reads_health_then_sees_only_later_changes")
spoken_for("health-is-state-with-parts (a part's name is one shape)", "health::tests::a_part_name_is_a_path_of_printable_segments")
spoken_for("health-is-state-with-parts (-serve says a malformed part, still serving it)", "health::tests::a_malformed_part_name_is_served_and_said")
spoken_for("health-is-read-through-one-asker", "health::tests::a_products_health_reads_back_by_value_through_the_one_asker")
spoken_for("health-is-read-through-one-asker (bound)", "health::tests::a_health_that_never_answers_is_unanswered_at_its_bound")
spoken_for("health-is-read-through-one-asker (refused)", "health::tests::a_refused_health_ask_is_said_refused_by_its_name")
spoken_for("health-is-read-through-one-asker (wire)", "health::tests::the_kernels_health_wire_is_the_served_reply_byte_for_byte")
spoken_for("health-may-say-what-a-restart-costs", "health::tests::a_restart_cost_is_said_once_per_change_and_can_be_taken_back")
spoken_for("a-product-pushes-events-as-a-stream-it-serves", "serve_tests::a_foreign_cursor_is_not_mine_and_an_old_one_is_out_of_range")
spoken_for("a-keyed-route-names-a-string-field", "options::tests::a_keyed_route_names_a_top_level_string_field")
spoken_for("a-route-a-reader-doesnt-know-is-refused-never-defaulted", "descriptor::tests::node_and_keyed_via_are_read_and_an_unknown_route_is_never_local_first")
spoken_for("the-serving-face-runs-the-whole-principal-ladder", "principal_tests::each_step_refuses_by_its_own_name")
spoken_for("a-verifiers-waits-are-bounded (keys)", "principal_tests::a_keys_lookup_that_never_answers_is_refused_lookup_unanswered")
spoken_for("a-verifiers-waits-are-bounded (nonce)", "principal_tests::a_nonce_write_held_past_its_bound_is_refused_and_still_lands")
spoken_for("a-verifiers-waits-are-bounded (nested)", "principal::tests::a_verifiers_waits_sit_inside_the_smallest_ask_bound")
spoken_for("the-rollout-rule-admits-only-a-call-with-no-token (three states)", "principal_tests::a_call_carries_a_token_nothing_or_something_unreadable")
spoken_for("a-verifiers-waits-are-bounded (a lost write is unanswered)", "principal_tests::a_nonce_write_that_panics_is_unanswered_never_replay")
spoken_for("a-verifiers-waits-are-bounded (writes capped)", "principal_tests::a_stalled_store_fills_the_write_cap_and_the_next_token_is_refused_at_once")
spoken_for("a-verifiers-waits-are-bounded (rollout bounded)", "principal_tests::a_rollout_answer_that_never_comes_admits_nothing")
spoken_for("the-rollout-rule-admits-only-a-call-with-no-token", "principal_tests::an_absent_token_is_admitted_only_under_the_rollout_rule_and_counted")
spoken_for("the-rollout-rule-admits-only-a-call-with-no-token (unreadable)", "principal_tests::an_unreadable_token_header_is_refused_never_admitted_as_absent")
spoken_for("the-serving-face-runs-the-whole-principal-ladder (replay)", "principal_tests::a_token_accepted_once_is_refused_replay_after")
spoken_for("a-nonce-is-durable-before-the-method-runs", "principal_tests::the_verifier_answers_only_after_the_nonce_is_recorded")
spoken_for("a-token-binds-the-bytes-as-received", "serve_tests::a_request_with_a_field_this_build_does_not_know_still_verifies")
spoken_for("a-token-binds-the-bytes-as-received (in flight)", "serve_tests::the_refusals_reach_a_grpc_caller_by_name")
spoken_for("the-verifier-applies-retired-versus-revoked", "principal_tests::a_revoked_key_refuses_even_a_backdated_token_and_a_retired_key_only_later_ones")
spoken_for("the-verifier-applies-retired-versus-revoked (units)", "principal_tests::a_record_time_in_the_wrong_unit_is_refused_as_implausible")
spoken_for("a-nonce-is-durable-before-the-method-runs (a crash)", "edge_tests::an_acknowledged_nonce_survives_an_abort_and_a_reopen")
spoken_for("the-judged-peer-is-on-every-request", "accepted_tests::the_judged_peer_is_on_every_request")
spoken_for("the-judged-peer-is-on-every-request (unreadable code)", "accepted_tests::an_unreadable_code_identity_reaches_the_request_as_unreadable")
spoken_for("the-judged-peer-is-on-every-request (unjudged)", "accepted_tests::a_connection_not_accepted_through_planes_is_unjudged")
spoken_for("the-judged-peer-is-on-every-request (whole token)", "peer::tests::a_peers_pid_pair_is_its_tokens")
spoken_for("the-api-plane-applies-its-rules-below-every-service", "plane_tests::the_layer_applies_the_rules_before_any_service")
spoken_for("the-api-plane-applies-its-rules-below-every-service (unjudged)", "plane_tests::an_unjudged_connection_is_refused_by_name")
spoken_for("one-function-serves-the-api-plane", "serve_fn_tests::one_function_serves_the_contracts_faces_and_the_products_calls")
spoken_for("one-function-serves-the-api-plane (rules)", "serve_fn_tests::the_planes_rules_apply_to_the_products_calls")
spoken_for("one-function-serves-the-api-plane (stop)", "serve_fn_tests::the_plane_stops_cleanly_when_nothing_is_in_hand")
spoken_for("one-function-serves-the-api-plane (bounded stop)", "serve_fn_tests::a_call_that_never_ends_is_cut_at_the_window_and_the_stop_says_forced")
spoken_for("one-function-serves-the-api-plane (admission)", "serve_fn_tests::an_admission_layer_is_outermost_and_sees_every_call")
spoken_for("one-function-serves-the-api-plane (reserved)", "serve_fn_tests::a_reserved_or_twice_named_product_service_is_refused_by_name")
spoken_for("describe-answers-per-caller", "describe_tests::describe_answers_what_this_caller_is_shown")
spoken_for("describe-answers-per-caller (fabricd)", "describe_tests::fabricd_and_a_product_not_answering_per_caller_get_nothing_per_caller")
spoken_for("describe-answers-per-caller (whole)", "describe_tests::a_per_caller_answer_naming_nothing_hides_every_offered_method")
spoken_for("describe-answers-per-caller (wire)", "describe::tests::what_a_caller_is_shown_rides_beside_the_digest_never_in_it")
spoken_for("describe-answers-per-caller (parity)", "parity_tests::the_method_shown_messages_are_the_protos")
spoken_for("fabricd-dials-the-api-plane-for-describe-only", "plane_tests::fabricd_reads_describe_only")
spoken_for("every-api-call-has-a-principal (stamp)", "plane_tests::a_stamp_is_believed_only_from_agora")
spoken_for("an-ask-is-bounded-in-the-call", "descriptor::tests::an_ask_bound_reads_45_when_absent_and_is_capped_at_120")
spoken_for("an-ask-is-bounded-in-the-call (wire parity)", "serve_tests::the_descriptor_check_reads_the_protos_own_encoding")
spoken_for("guidance-is-a-service-option", "descriptor::tests::guidance_is_read_with_its_summary_and_refused_past_its_bounds")
spoken_for("guidance-is-a-service-option (instructions)", "mcp::tests::the_orient_tool_says_so_and_the_instructions_are_the_guidance")
spoken_for("orient-is-one-bounded-read", "descriptor::tests::an_orient_is_one_unary_agent_read_with_no_input_and_a_bound")
spoken_for("orient-is-one-bounded-read (wire parity)", "serve_tests::the_kernel_reads_an_orient_and_guidance_as_the_proto_writes_them")
spoken_for("the-static-half-is-byte-stable", "fixture_tests::the_fixtures_static_half_is_byte_stable")
spoken_for("the-static-half-is-byte-stable (red)", "fixture_tests::a_static_half_that_changes_across_a_restart_is_red")
spoken_for("the-static-half-is-byte-stable (tool list)", "stable::stable_tests::a_changed_tool_list_is_red_and_named")
spoken_for("the-static-half-is-byte-stable (bounded)", "stable::stable_tests::a_step_that_never_answers_is_named_at_its_bound")
spoken_for("the-static-half-is-byte-stable (a face)", "stable::stable_tests::a_face_refused_on_the_first_read_fails_the_row_by_name")
spoken_for("a-nonce-is-durable-before-the-method-runs (group commit)", "edge_tests::a_batch_is_one_sync")
spoken_for("a-nonce-is-durable-before-the-method-runs (schema)", "edge_tests::a_newer_schema_is_refused_by_name_and_schema_1_opens_said_or_not")
spoken_for("a-nonce-is-durable-before-the-method-runs (damage)", "edge_tests::a_bad_line_before_the_last_is_refused_by_name")
spoken_for("a-nonce-is-durable-before-the-method-runs (dirty)", "edge_tests::after_a_failed_write_the_next_commit_rewrites_the_file_whole_first")
spoken_for("every-bridge-refuses-a-descriptor-alike", "descriptor::tests::every_refusal_is_answered_by_name")
spoken_for("every-bridge-refuses-a-descriptor-alike (parity)", "serve_tests::the_descriptor_check_reads_the_protos_own_encoding")
spoken_for("every-bridge-mounts-the-same-fixture", "fixture_tests::the_good_fixture_answers_over_its_socket")
spoken_for("every-bridge-mounts-the-same-fixture (variants)", "fixture_tests::each_variant_is_refused_where_its_rule_lives")
spoken_for("every-bridge-refuses-a-descriptor-alike (examples)", "descriptor::tests::an_example_that_is_not_the_requests_fields_is_refused")
spoken_for("every-bridge-encodes-a-call-alike", "client_tests::a_call_goes_from_json_to_the_product_and_back")
spoken_for("every-bridge-encodes-a-call-alike (strict)", "client_tests::arguments_that_are_not_the_request_are_refused_by_name")
spoken_for("every-bridge-encodes-a-call-alike (bytes)", "client_tests::the_encoded_bytes_are_the_requests_own_encoding")
spoken_for("a-name-is-judged-by-one-check-from-unicodes-own-data", "label::label_tests::the_tables_are_unicodes_own_data")
spoken_for("a-name-is-judged-by-one-check-from-unicodes-own-data (vectors)", "label::label_tests::every_shared_vector_holds")
spoken_for("a-name-is-judged-by-one-check-from-unicodes-own-data (named)", "label::label_tests::a_refusal_names_the_rule_and_the_code_point")
spoken_for("a-name-is-judged-by-one-check-from-unicodes-own-data (generated)", "label::label_tests::the_generated_tables_are_current")
spoken_for("a-name-is-judged-by-one-check-from-unicodes-own-data (separator)", "label::label_tests::a_separator_lookalike_is_refused_by_name")
spoken_for("one-exposure-for-every-face", "exposure::tests::one_exposure_for_every_face")
spoken_for("a-row-tightens-freely-and-loosens-only-by-a-persons-grant", "exposure::tests::a_row_tightens_freely_and_loosens_only_by_a_persons_grant")
spoken_for("a-methods-default-is-a-floor", "exposure::tests::a_methods_default_is_a_floor")
spoken_for("one-mapping-flattens-a-descriptor", "mcp::tests::one_mapping_flattens_a_descriptor")
spoken_for("one-mapping-flattens-a-descriptor (field kinds)", "mcp::tests::each_field_kind_has_its_json_mapping")
spoken_for("one-mapping-flattens-a-descriptor (fixture)", "fixture_tests::the_fixture_flattens_and_its_colliding_names_do_not")
spoken_for("a-flattened-name-is-one-tool", "mcp::tests::a_flattened_name_is_one_tool")
spoken_for("events-flatten-to-one-bounded-read", "mcp::tests::events_flatten_to_one_bounded_read")
spoken_for("an-mcp-call-names-one-method", "mcp::tests::an_mcp_call_names_one_method")
spoken_for("one-describe-reply-for-every-product", "describe::tests::one_describe_reply_for_every_product")
spoken_for("one-describe-reply-for-every-product (parity)", "parity_tests::the_describe_reply_is_the_protos")
spoken_for("a-products-events-are-read-through-one-call", "serve_tests::a_journals_events_call_is_refused_by_name")
spoken_for("a-products-events-are-read-through-one-call (round trip)", "events::tests::a_consumer_reads_a_producers_events_through_one_call")
spoken_for("a-products-events-are-read-through-one-call (parity)", "parity_tests::the_events_call_messages_are_the_protos")
spoken_for("a-product-serves-its-own-log-through-the-face", "log_events_tests::a_call_reads_the_log_only_once_authorized")
spoken_for("a-product-serves-its-own-log-through-the-face (bound)", "log_events_tests::an_unanswered_authorization_is_refused_at_its_bound")
spoken_for("a-product-serves-its-own-log-through-the-face (Events, one bounded Call)", "log_events_tests::the_journals_stream_on_a_log_face_is_one_bounded_read_by_cursor")
spoken_for("a-product-serves-its-own-log-through-the-face (Events, kinds read past)", "log_events_tests::a_kind_filtered_read_reads_past_pages_of_other_kinds")
spoken_for("a-product-serves-its-own-log-through-the-face (Events, a compacted start)", "log_events_tests::a_compacted_logs_start_begins_past_its_floor_and_a_cursor_below_it_is_out_of_range")
spoken_for("a-product-serves-its-own-log-through-the-face (Events, a many-log face)", "log_events_tests::an_empty_cursor_on_a_many_log_face_is_refused_naming_its_logs")
spoken_for("a-product-serves-its-own-log-through-the-face (Events, a scan cut mid-read)", "log_events_tests::a_scan_the_deadline_cuts_mid_read_names_where_to_read_on")
spoken_for("a-call-is-bounded-by-one-deadline", "log_events_tests::a_late_authorization_leaves_the_read_only_what_remains")
spoken_for("a-call-is-bounded-by-one-deadline (cap)", "log_events_tests::calls_past_the_cap_are_refused_at_once")
spoken_for("a-log-watch-is-bounded-and-ends-stopped (kernel)", "watch_tests::every_exit_stops_the_wire_watch_first_and_once")
spoken_for("a-log-watch-is-bounded-and-ends-stopped (drain)", "log_events_tests::a_drain_ends_a_watch_draining_and_its_thread")
spoken_for("a-log-watch-is-bounded-and-ends-stopped (gone)", "log_events_tests::a_consumer_gone_stops_its_watch")
spoken_for("a-log-watch-is-bounded-and-ends-stopped (cap)", "log_events_tests::watches_past_the_cap_are_refused_and_none_starved")
spoken_for("a-log-watch-is-bounded-and-ends-stopped (writer)", "log_events_tests::a_held_consumer_never_blocks_the_writer")
spoken_for("a-revoked-grant-stops-a-watch-within-one-token-life", "log_events_tests::a_revoke_between_renewals_ends_the_watch_at_the_next")
spoken_for("a-bytes-field-may-say-it-is-an-image", "media::tests::a_tagged_bytes_field_says_its_media")
spoken_for("a-bytes-field-may-say-it-is-an-image (not bytes)", "media::tests::media_on_a_field_that_is_not_bytes_is_refused")
spoken_for("a-bytes-field-may-say-it-is-an-image (unknown)", "media::tests::an_unknown_media_is_refused")
spoken_for("a-bytes-field-may-say-it-is-an-image (untagged)", "media::tests::a_descriptor_without_media_is_unchanged")
spoken_for("a-bytes-field-may-say-it-is-an-image (connect)", "media::tests::check_descriptor_refuses_a_bad_media_tag")
spoken_for("a-watch-ends-when-its-authorization-does", "log_events_tests::a_revoke_mid_stream_ends_the_watch_unauthorized_after_what_was_sent")
spoken_for("a-watch-ends-when-its-authorization-does (until)", "log_events_tests::a_watch_admitted_until_an_end_ends_unauthorized_at_it")
spoken_for("a-watch-ends-when-its-authorization-does (kernel)", "watch_tests::a_revoke_or_a_lapse_ends_unauthorized_after_what_was_sent")
spoken_for("a-products-events-are-pushed-through-one-watch", "events::tests::a_consumer_is_pushed_a_producers_events_through_one_watch")
spoken_for("a-products-events-are-pushed-through-one-watch (journal)", "serve_tests::a_journals_events_watch_is_refused_by_name")
spoken_for("a-products-events-are-pushed-through-one-watch (unserved)", "events::tests::an_unserved_watch_or_call_is_refused_by_name_not_unclear")
spoken_for("one-mapping-flattens-a-descriptor (map_entry)", "descriptor::tests::a_map_field_is_known_by_its_entrys_map_entry")
spoken_for("an-external-caller-is-parsed-here-and-trusted-only-from-its-peer", "mcp::tests::an_external_caller_is_parsed_here_and_trusted_only_from_its_peer")

spoken_for("a-carried-call-reaches-reads-only", "plane_tests::a_carried_call_reaches_reads_only")
spoken_for("a-carried-call-reaches-reads-only (Health, house-signed)", "plane_tests::a_carried_health_ask_is_admitted_only_from_a_house_signed_caller")
spoken_for("a-carried-write-may-be-admitted-for-a-claimed-lane", "plane_tests::a_listed_carried_write_with_a_lane_is_admitted_with_its_claim")
spoken_for("a-carried-write-may-be-admitted-for-a-claimed-lane (writes only)", "plane_tests::only_a_descriptors_reads_and_writes_can_be_listed_for_claimed_lanes")
spoken_for("a-carried-write-may-be-admitted-for-a-claimed-lane (unlisted)", "plane_tests::an_unlisted_carried_write_is_refused_as_before")
spoken_for("a-carried-write-may-be-admitted-for-a-claimed-lane (a lane)", "plane_tests::a_carried_claim_the_product_doesnt_call_a_lane_or_not_shaped_as_one_is_refused")
spoken_for("a-carried-write-may-be-admitted-for-a-claimed-lane (preface)", "plane_tests::a_carried_write_with_no_preface_is_refused")
spoken_for("a-carried-write-may-be-admitted-for-a-claimed-lane (other peers)", "plane_tests::a_listed_write_from_any_other_peer_is_judged_as_before")
spoken_for("a-carried-write-may-be-admitted-for-a-claimed-lane (layer)", "plane_tests::the_layer_hands_the_service_the_claim")
spoken_for("a-bound-carried-call-reaches-a-method-its-product-decides", "plane_tests::a_bound_carried_call_reaches_only_a_method_its_product_decides")
spoken_for("a-bound-carried-call-reaches-a-method-its-product-decides (listed)", "plane_tests::only_a_products_own_agent_methods_can_be_decided_for_bound_callers")
spoken_for("a-bound-carried-call-reaches-a-method-its-product-decides (no other role)", "plane_tests::a_bound_principal_opens_no_other_role")
spoken_for("a-bound-carried-call-reaches-a-method-its-product-decides (layer)", "plane_tests::the_layer_serves_a_bound_carried_call_only_where_its_product_decides")
spoken_for("a-carried-product-caller-is-proven-by-its-code", "plane_tests::a_carried_product_caller_is_admitted_only_when_proven_the_houses_and_listed")
spoken_for("the-trust-anchor-takes-no-dependency", "tests::the_trust_anchor_takes_no_dependency")
spoken_for("the-trust-anchor-takes-no-dependency (shape)", "tests::the_team_is_a_team_identifier")
spoken_for("an-apps-requirement-has-one-writer", "tests::an_apps_requirement_compiles_identifier_first")
spoken_for("an-apps-requirement-has-one-writer (plain)", "tests::a_value_that_isnt_plain_is_never_spliced_into_a_requirement")
spoken_for("a-daemons-info-plist-has-one-writer", "tests::a_daemons_info_plist_is_fabrics_byte_for_byte")
spoken_for("a-carried-product-caller-is-proven-by-its-code (descriptor)", "plane_tests::the_methods_products_call_are_read_from_the_descriptor")
spoken_for("a-carried-product-caller-is-proven-by-its-code (layer)", "plane_tests::the_layer_admits_a_proven_product_and_hands_its_caller_to_the_service")
spoken_for("a-carried-product-caller-is-proven-by-its-code (another descriptor, the unnamed said)", "plane_tests::another_descriptors_product_methods_merge_and_the_unnamed_are_said")
spoken_for("a-carried-product-caller-is-proven-by-its-code (unchecked)", "plane_tests::methods_named_of_an_unchecked_descriptor_are_refused_as_unchecked_with_the_checkers_refusals")
spoken_for("a-carried-write-may-be-admitted-for-a-claimed-lane (reads)", "plane_tests::a_carried_read_with_a_stamp_is_admitted_as_a_read_and_its_stamp_unused")
spoken_for("a-carried-write-may-be-admitted-for-a-claimed-lane (attribution)", "plane_tests::a_listed_carried_read_is_attributed_when_its_claim_holds_and_served_when_it_doesnt")
spoken_for("a-carried-connection-says-its-preface-once", "serve_fn_tests::a_carried_connections_preface_rides_its_requests_and_http2_after_it_is_served")
spoken_for("a-carried-connection-says-its-preface-once (only a carried peer)", "opening::tests::only_a_carried_peer_may_open_with_a_preface_line")
spoken_for("a-carried-connection-says-its-preface-once (older aegisd)", "opening::tests::a_carried_peer_opening_with_http2_is_bare")
spoken_for("a-carried-connection-says-its-preface-once (bounded)", "opening::tests::a_carried_opening_that_doesnt_finish_closes_at_the_bound")
spoken_for("a-carried-connection-says-its-preface-once (codec)", "carried::tests::each_field_that_wont_parse_is_refused_by_name")
spoken_for("a-carried-connection-says-its-preface-once (the writer)", "carried::tests::encode_refuses_what_parse_would_by_the_same_name")
spoken_for("a-carried-connection-says-its-preface-once (the caller's code identity, 1.1)", "carried::tests::a_callers_code_identity_rides_as_the_fifth_part_and_a_line_without_it_still_reads")
spoken_for("a-connection-opens-within-its-bound", "opening::tests::silence_or_a_part_then_silence_closes_at_the_bound")
spoken_for("a-connection-opens-within-its-bound (in time)", "opening::tests::a_full_http2_preface_in_time_is_handed_on_whole")
spoken_for("a-connection-opens-within-its-bound (carried line, then HTTP/2)", "opening::tests::a_carried_line_then_the_http2_preface_is_read_whole")
spoken_for("a-connection-opens-within-its-bound (not HTTP/2)", "opening::tests::an_opening_that_isnt_http2_closes_by_name")
spoken_for("a-connection-opens-within-its-bound (a burst)", "opening::tests::a_burst_from_one_peer_all_opens")
spoken_for("a-connection-opens-within-its-bound (silence holds only its sender)", "opening::tests::silence_holds_only_its_senders_turns")
spoken_for("a-connection-opens-within-its-bound (crowded, unreadable share one)", "opening::tests::a_turn_that_never_comes_closes_crowded_and_the_unreadable_share_one")
spoken_for("a-connection-opens-within-its-bound (a close is said quietly)", "opening::tests::closes_are_said_at_most_once_a_minute_as_counts_by_end")
spoken_for("a-products-http-routes-ride-the-plane (a request reaches its route with its verdict)", "serve::http_routes_tests::an_http1_request_reaches_the_products_route_with_its_verdict")
spoken_for("a-products-http-routes-ride-the-plane (a refusal is 403, never 200)", "serve::http_routes_tests::a_refused_http_request_is_403_with_its_refusal_never_200")
spoken_for("a-products-http-routes-ride-the-plane (no http routes, no HTTP/1)", "serve::http_routes_tests::a_plane_without_http_routes_closes_an_http1_opening")
spoken_for("a-products-http-routes-ride-the-plane (a carried peer never opens HTTP/1)", "serve::http_routes_tests::a_carried_peer_never_opens_http1")
spoken_for("a-products-http-routes-ride-the-plane (expect-continue, a streamed upload)", "serve::http_routes_tests::an_expect_continue_upload_streams_to_the_handler")
spoken_for("a-products-http-routes-ride-the-plane (length and chunked smuggle nothing)", "serve::http_routes_tests::a_request_with_both_length_and_chunked_smuggles_nothing")
spoken_for("a-products-http-routes-ride-the-plane (the opening's HTTP/1 by name)", "opening::tests::an_http1_plane_opens_a_whole_request_head_and_refuses_the_rest_by_name")
spoken_for("a-log-is-read-through-one-seam", "log_events_tests::a_log_opened_per_read_serves_call_once_per_call_and_says_busy")
spoken_for("a-log-is-read-through-one-seam (a write between a read and its wait is never lost)", "reread::tests::a_write_between_the_read_and_the_wait_is_never_lost")
spoken_for("a-log-is-read-through-one-seam (the wait is outside the open)", "reread::tests::a_wait_is_outside_the_open_and_woken_by_changes_or_its_bound")
spoken_for("one-face-serves-several-logs-by-name", "many::tests::a_read_goes_to_the_log_its_position_names")
spoken_for("one-face-serves-several-logs-by-name (latest)", "many::tests::latest_on_a_many_log_face_reads_the_log_it_names_and_never_routes_by_spelling")
spoken_for("one-face-serves-several-logs-by-name (built)", "many::tests::over_many_names_each_log_once_and_a_call_reads_the_named_one")
spoken_for("a-followed-log-wakes-its-watch-as-its-file-moves", "follow::tests::a_write_and_a_replace_each_wake_a_reader_before_the_beat_and_the_watch_follows")
spoken_for("a-followed-log-wakes-its-watch-as-its-file-moves (absent)", "follow::tests::a_delete_wakes_at_once_and_a_recreated_file_is_found_at_the_beat")
spoken_for("a-followed-log-wakes-its-watch-as-its-file-moves (quiet)", "follow::tests::beats_and_stale_notes_wake_nobody")
spoken_for("a-followed-log-wakes-its-watch-as-its-file-moves (host)", "reread::tests::a_followed_file_wakes_a_waiter_through_a_replace_and_after_it")
spoken_for("an-unreadable-log-is-never-busy", "log_events_tests::a_log_that_cannot_be_read_is_unreadable_by_name_never_busy")
spoken_for("an-unreadable-log-is-never-busy (watch)", "reread::tests::a_reread_of_an_unreadable_log_is_refused_by_name_not_busy")
