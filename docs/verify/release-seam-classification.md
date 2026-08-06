# Release-profile suite — 24 failing tests classified (2026-08-06)

Run: `cargo test --release --workspace --no-fail-fast -- --test-threads=3` at `32ae96a`'s
tree state (post doctorcmd gating) → **964 passed / 24 failed / 4 ignored**. Debug at the
same tree: 987/0/4. Every failure classified against source (Opus classification round;
independently re-reproduced by the plan-exit gate: same totals, same 24 names, 3 seams read
in source, 1 empirical probe + positive control). Classes: SEAM = test drives a
`#[cfg(debug_assertions)]`-gated seam stripped from release by design; TIMING = wall-clock
bound differs; REAL = actual release-mode product defect.

**Result: 24 SEAM / 0 TIMING / 0 REAL.**

| test | binary | seam (file:symbol) |
|---|---|---|
| purge_log_duplicates_aborts_on_concurrent_growth | hardening_cli | `purgecmd.rs::test_pause_before_log_rewrite_delay` (`AGENTREC_TEST_PAUSE_BEFORE_LOG_REWRITE_MS`) |
| purge_rewrite_never_loses_concurrent_append | hardening_cli | `purgecmd.rs::test_pause_before_memory_rewrite_delay` — 3000 ms injected, 12.03 ms observed ⇒ sleep compiled out |
| remember_waits_or_fails_cleanly_during_purge | hardening_cli | same pause seam (15.96 ms observed) |
| intervening_edit_between_snapshot_and_second_edit_is_rejected_as_stale | import_claude | `importcmd.rs::debug_dump_entries_enabled` (`AGENTREC_IMPORT_DEBUG_ENTRIES`) |
| missing_old_string_classifies_t15_unverified_not_content_proven | import_claude | same |
| redundant_reannouncement_of_same_backup_name_does_not_clear_staleness | import_claude | same |
| relative_key_harvested_before_cwd_is_known_still_resolves | import_claude | same |
| session_level_cwd_survives_a_headless_summary_line | import_claude | same |
| stale_backup_blob_is_rejected_not_classified_t15 | import_claude | same |
| t15_relative_backup_key_resolves_against_session_cwd | import_claude | same |
| t1_and_t15_resolve_with_matching_sha256 | import_claude | same (gate's empirical probe: fails at "no debug_entries entry") |
| t2_candidate_vs_t3_classification | import_claude | same |
| persist::ac5b_oracle_fixture_negative_git_blob_disagrees_with_true_before | import_claude | `importcmd.rs::t2_oracle_enabled` (`AGENTREC_IMPORT_T2_ORACLE`) |
| persist::ac5b_oracle_fixture_positive_git_blob_matches_true_before | import_claude | same |
| dry_run_report_keys_match_import_claude | import_codex | expected key list hardcodes `debug_entries` (`import_codex.rs`, key-parity list) — debug_entries family |
| add_and_delete_both_classify_t1 | import_codex | `debug_dump_entries_enabled` |
| update_classifies_t2_candidate_vs_t3_by_git_tracking | import_codex | same |
| daemon_eviction_keeps_protected_refs | integration | `daemon.rs::effective_evict_interval` (`AGENTREC_TEST_EVICT_INTERVAL_MS`) + `cmds.rs::effective_store_budget` (`AGENTREC_TEST_STORE_BUDGET_BYTES`) |
| daemon_periodic_tick_evicts_after_startup_pass | integration | same pair |
| daemon_mid_tick_survives_config_corruption_after_startup | integration | same pair (release stderr nonetheless shows the daemon degrading to defaults, not crashing) |
| hook_recall_bails_at_injected_deadline | integration | `cmds.rs::recall_deadline` forced-deadline seam |
| hook_stats_lines_carry_elapsed_ms_on_either_bail_site | integration | same |
| hook_recall_hard_wall_deadline | integration | `memory.rs::test_slow_pin_read_delay` (`AGENTREC_TEST_SLOW_PIN_READ_MS`) — no sleep ⇒ recall legitimately completes |
| json_contracts::status_and_status_json_are_both_zero_write_on_over_budget_store | integration | `cmds.rs::effective_store_budget` — release uses the 2 GiB default so `over_budget` never becomes true |

Family arithmetic: hardening_cli 3 + import_claude 11 (9 debug_entries + 2 oracle) +
import_codex 3 (debug_entries family, incl. the key-parity list) + integration 7
(3 budget/evict + 3 recall-deadline/slow-pin + 1 json_contracts budget) = **24**.

Positive control: `persist::ac5b_oracle_seam_disabled_in_release_even_with_env_set`
**passes** in release — the seam-off path is itself pinned.

Coverage honesty (carried from the classification): ~20 of the 24 fail at a seam-gated
*precondition*, so their subject invariant is UNEXERCISED in release — not proven
equivalent. Four subjects are visibly confirmed in the captured release output despite the
test failing (daemon mid-tick corruption degrades to defaults; codex/claude report key
parity at 14 keys; two tier-classification runs print correct `tier_counts`). Notably the
zero-write status parity assertion is never reached in release. This is a coverage gap
release-mode testing creates, not a defect.

Single-machine caveat: one macOS machine; no second-machine or CI release-suite leg exists.
