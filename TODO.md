# DevCode To-Do

Rule: one item at a time, in order. Item done = edit made, verified, "not checked" reported. No commits unless asked. `references/` stays gitignored.

## Done

### Spec / research (closed)
- [x] D1 `.gitignore`: `references/` ignored (user-set)
- [x] D2 `.devcode/` write exclusion + tamper-evident audit log (prd FR-2.3, FR-4.1, Gate 2; arch §5.2)
- [x] D3 Cross-platform matrix + curl/PowerShell installer spec (arch §6.2–6.3; prd FR-6, Gates 6–7; README)
- [x] D4 Reference selection review (`docs/research/reference-selection.md`, RES-007)
- [x] D5 `ask` profile added (README, prd FR-1.2/1.3, arch tenet 1, §3 table, §3.1)
- [x] T1 RES-008 rollback peers → steal/reject vs FR-3
- [x] T2 Shell write policy (FR-3.2 / arch §6.4); Windows included
- [x] T3 `security` profile patch path: harness applies approved diffs
- [x] T4 Command blacklist reframed as UX hint; policy + approvals are control
- [x] T5 Class B empty in v1
- [x] T6 RES-009 Windows runtime → arch §6.2
- [x] T7 Spec number fixes (Tier 2 300; NFR-6/8 rows)
- [x] T8 Audit log truncation anchor (state dir + `refs/devcode/audit-tail`)
- [x] T9 Reference catalog upkeep (status / last-push; drop continue/openhands/stakpak from active study)
- [x] T10 Implementation language: Rust (prd FR-6.3)
- [x] D6 Shell edits recoverable: guard-set snapshot before mutation-capable shell commands (arch §6.4.5, prd FR-3.2, NFR-9, Gate 8); review item 3
- [x] D7 Windows path canonicalization: strip `\\?\` prefix instead of rejecting; new-file, containment, device-path rules (arch §6.2, §6.4.4, RES-009); review item 4
- [x] D8 Overclaims removed: Rust no longer "guarantees" NFR-1/2; no SLSA level claimed; static-link wording fixed (prd FR-6.3/6.4, arch §6.3, README); review item 5
- [x] D9 Revisions bumped (arch/prd 0.4.0, CBU 0.3.1); `docs/research/README.md` removed (duplicated per-note headers); review item 6
- [x] T11 Minisign signing spec (prd FR-6.4). Host was invented; replaced by placeholders (see R1)
- [x] T12 Demand evidence rewritten (RES-010 0.2.0): 0.1.0 misread sources; gate demand = hypothesis, rollback/approvals stronger
- [x] T13 Doc ID tidy: ATE-100x normative vs DEVCODE-RES-00x informative
- [x] T14 Arch §4 stale “capture or block all subprocess writes” aligned with §6.4 / FR-3.2
- [x] T15 TODO hygiene: completed Ts moved to Done; implementation queue opened

## MVP scope (proposed; confirm before building)
Goal: a working CLI an engineer can run on a real repo, showing the thesis: minimal verified change, reversible, low token use.
- Platforms: macOS and Linux first. Windows native after MVP (spec keeps it Tier 1).
- Profiles: `ask`, `plan`, `build`. `debug` and `security` after MVP.
- Model provider: NVIDIA NIM first; Gemini second via same `openai_compat` adapter (M0/P1–P8).
- Tools: read, glob, grep (JIT only), write/edit, run_command.
- Guard: workspace realpath allowlist, `.devcode/` deny, env credential strip, process-tree timeout kill.
- Rollback: step manifests plus shell guard-set snapshot; `devcode rollback`.
- Gate v0: run the configured test command after edits; bind result to the diff hash; refuse "done" if it did not run or is stale; show a harness-built verification summary (not model text).
- Discipline v0: declare edit scope up front; flag edits outside it and new dependencies for approval.
- Tokens v0: count tokens per task; compaction at 75% (arch §7.1).
- Approvals: diff once/always/reject; destructive commands always ask.

## MVP build order
- [x] M0 **Decide**: provider = NVIDIA NIM via OpenAI-compatible adapter (decided); confirm scope above; pick NIM model. Gemini second preset via same adapter.
- [x] M1 (I1) Rust workspace scaffold: binary crate, CLI stub, CI skeleton (fmt/clippy/test) for macOS and Linux.

```
Task: M1
Changed: Cargo.toml, Cargo.lock, .gitignore, src/main.rs, src/lib.rs, src/cli.rs, src/config.rs, tests/m1_scaffold.rs, .github/workflows/ci.yml
Dependencies added:
- clap (v4.5, features=["derive"]): CLI parsing with subcommands, version/help flags, and exit codes
- serde (v1.0, features=["derive"]): serialization and deserialization
- serde_json (v1.0): JSON config parsing
- tempfile (v3.17, dev): isolated temporary directories for config and integration tests
Commands run:
- cargo fmt --check: passed
- cargo clippy --all-targets -- -D warnings: passed
- cargo test: passed (9 tests, 0 failures)
Acceptance:
- `devcode --version` prints the package version, exit 0: PASS (test_cli_version_flag_via_binary)
- unknown flag exits 2: PASS (test_cli_unknown_flag_exits_2)
- Config tests - valid file: PASS (test_config_valid_file)
- Config tests - missing file (defaults): PASS (test_config_missing_file_gives_defaults)
- Config tests - unknown field (error naming the field): PASS (test_config_unknown_field_names_the_field)
- Config tests - wrong type (error): PASS (test_config_wrong_type)
- CLI help flag exits 0: PASS (test_cli_help_flag_exits_0)
- Subcommands & root stub exit 1 with "not implemented": PASS (test_cli_stub_subcommand_exits_1)
Measured:
- Integration test suite execution time: 0.09s (`cargo test --test m1_scaffold`)
Not done / limits:
- Subcommand execution and agent loop are stubs returning "not implemented" (scheduled for M2-M9).
- Network calls, tools, REPL, models are non-goals for M1.
Spec questions:
- None for M1.
Not checked:
- Linux CI run on GitHub Actions (CI workflow defined in `.github/workflows/ci.yml`; awaiting push).
```

- [x] M2 (I3) Runtime guard v1 (POSIX only).

```
Task: M2
Changed: Cargo.toml, src/lib.rs, src/guard/mod.rs, src/guard/path.rs, src/guard/env.rs, src/guard/process.rs, tests/m2_guard.rs
Dependencies added:
- libc (v0.2): required for process group signaling (killpg, SIGTERM, SIGKILL) and OS pipe/dup descriptors
Commands run:
- cargo fmt --check: passed
- cargo clippy --all-targets -- -D warnings: passed
- cargo test: passed (17 tests across m1_scaffold and m2_guard, 0 failures)
Acceptance:
- Symlink pointing outside root: denied: PASS (test_symlink_pointing_outside_root_denied)
- `../` escape: denied: PASS (test_parent_dir_escape_denied)
- Write path under `.DevCode/x`: denied: PASS (test_devcode_write_denied_case_insensitively)
- New file in an existing subdirectory: allowed: PASS (test_new_file_in_existing_subdirectory_allowed)
- `strip_env` removes each pattern and keeps `PATH`, `HOME`: PASS (test_strip_env_removes_patterns_and_keeps_path_home)
- Timeout test: `sh -c 'sleep 1000 & sleep 1000 & wait'` with 1 s timeout leaves 0 processes: PASS (test_run_bounded_timeout_kills_process_group)
- Output cap: 1 MB output returns at most cap with head, tail, and marker: PASS (test_output_cap_truncation, test_run_bounded_1mb_output_enforces_cap)
Measured:
- NFR-8 watchdog kill duration (timeout to all-dead): 15.42 ms (target <= 100 ms) via `cargo test --test m2_guard test_run_bounded_timeout_kills_process_group -- --nocapture`
- Test suite runtime: 1.03s (`cargo test --test m2_guard`)
Not done / limits:
- Windows Job Objects and Windows path canonicalization are deferred to post-MVP (A1).
- Network isolation (Linux bwrap) is deferred to post-MVP.
Spec questions:
- None for M2.
Not checked:
- Direct `/bin/ps` check inside restrictive local sandbox environment (checked via `libc::killpg(pid, 0)` verified dead; ps checked when environment permits).
```

- [x] M3 (I4) Step-manifest engine incl. shell guard-set snapshot.

```
Task: M3
Changed: Cargo.toml, Cargo.lock, src/lib.rs, src/manifest/mod.rs, src/manifest/blob.rs, src/manifest/record.rs, src/manifest/rollback.rs, src/manifest/snapshot.rs, tests/m3_manifest.rs
Dependencies added:
- sha2 (v0.10): content-addressed blob hashing (SHA-256)
- ignore (v0.4): gitignore rule traversal and workspace file enumeration
Commands run:
- cargo fmt --check: passed
- cargo clippy --all-targets -- -D warnings: passed
- cargo test: passed (24 tests across m1_scaffold, m2_guard, and m3_manifest, 0 failures)
Acceptance:
- Gate 1: partially staged, dirty unstaged, untracked, unrelated dirty files restored with exact bytes, mode, untouched index, porcelain status preserved: PASS (test_gate1_checkpoint_invariance_and_persistence, test_gate1_in_repo_with_no_commits)
- Gate 4: external edit conflict preflight aborts without modifying disk and retains backups: PASS (test_gate4_external_edit_conflict_and_injected_failures)
- Gate 4: injected capture failure (read-only blob dir) refuses mutation: PASS (test_gate4_external_edit_conflict_and_injected_failures)
- Gate 4: injected restore failure reports partial recovery with backups retained: PASS (test_gate4_external_edit_conflict_and_injected_failures)
- Gate 8: reconcile detects in-place edit, redirect, delete, create; rollback restores them; gitignored files unprotected: PASS (test_gate8_shell_mutation_reconcile_and_recovery)
- Gate 8 in non-git directory with derived dirs excluded: PASS (test_gate8_in_non_git_directory)
- Budget exceeded (5000 files / 50MB blobs) returns BudgetExceeded: PASS (test_budget_exceeded_returns_error)
Measured:
- NFR-9 snapshot benchmark on 1,200-file fixture: first run = 5.07s, incremental = 33.58ms (target <= 250ms incremental) via `cargo test --test m3_manifest test_measured_snapshot_time_1200_files -- --nocapture`
Not done / limits:
- Manifest pruning and retention helpers (`devcode gc`) are out of scope for MVP.
- Windows file locking exponential backoff is deferred to post-MVP (A1).
Spec questions:
- None for M3.
Not checked:
- Linux CI execution on GitHub Actions (workflow configured; awaiting push).
```

- [x] M4 (I5) Single agent loop + provider adapter; limits (step/cost/time); format-error recovery.

```
Task: M4
Changed: src/lib.rs, src/agent/mod.rs, src/agent/model.rs, src/agent/scripted.rs, src/agent/tool.rs, src/agent/loop_runner.rs, tests/m4_agent.rs
Dependencies added: none
Commands run:
- cargo fmt --check: passed
- cargo clippy --all-targets -- -D warnings: passed
- cargo test: passed (30 tests across m1_scaffold, m2_guard, m3_manifest, and m4_agent, 0 failures)
Acceptance:
- Normal run ends on a no-tool response: PASS (test_normal_run_ends_on_no_tool_response)
- Step limit ends run with matching reason: PASS (test_step_limit_ends_run_with_matching_reason)
- Wall-time limit ends run with matching reason: PASS (test_wall_time_limit_ends_run_with_matching_reason)
- 3 consecutive format errors ends run with matching reason: PASS (test_consecutive_format_errors_limit_ends_run)
- Tool errors become observations, not crashes: PASS (test_tool_error_becomes_observation_not_crash)
- Token totals equal scripted usage: PASS (test_token_totals_equal_scripted_usage)
Measured:
- Agent loop execution time with ScriptedModel: <0.01s (`cargo test --test m4_agent`)
Not done / limits:
- Real network LLM provider adapter (`ureq`) deferred until provider decision in M0.
- Cost limit enforcement deferred until provider pricing is finalized.
- Streaming and multiple providers are non-goals for MVP.
Spec questions:
- Provider for real adapter still undecided (M0 open).
Not checked:
- Network calls against live provider (blocked by rule 4).
```

- [x] M5 (I2) Profile controller for `ask`/`plan`/`build`.

```
Task: M5
Changed: src/lib.rs, src/cli.rs, src/profile.rs, src/agent/loop_runner.rs, tests/m4_agent.rs, tests/m5_profiles.rs
Dependencies added: none
Commands run:
- cargo fmt --check: passed
- cargo clippy --all-targets -- -D warnings: passed
- cargo test: passed (35 tests across m1, m2, m3, m4, m5, 0 failures)
Acceptance:
- Tool list for ask and plan contains no mutating tool: PASS (test_ask_and_plan_tool_list_contains_no_mutating_tools)
- Forced call to write_file in ask returns error observation and changes nothing on disk: PASS (test_forced_call_to_write_file_in_ask_returns_error_and_does_not_mutate)
- Slash commands switch profiles: PASS (test_slash_commands_switch_profiles)
- Plan prompt requires structured specification: PASS (test_plan_prompt_requires_structured_specification)
- Ask prompt names profile switch: PASS (test_ask_prompt_names_profile_switch)
Measured:
- Profile test suite execution time: <0.01s (`cargo test --test m5_profiles`)
Not done / limits:
- `debug` and `security` profiles are deferred to post-MVP (A2).
Spec questions:
- None for M5.
Not checked:
- Live terminal input with interactive TTY prompt loop (tested via `handle_slash_command` unit tests and scripted loop integration).
```

- [x] M6 (I6) JIT context: glob, grep, surgical reads.

```
Task: M6
Changed: Cargo.toml, Cargo.lock, src/lib.rs, src/tools/mod.rs, src/tools/cap.rs, src/tools/glob.rs, src/tools/grep.rs, src/tools/read.rs, tests/m6_tools.rs
Dependencies added:
- regex (v1.11): regular expression parsing and matching for grep tool
- globset (v0.4): glob pattern matching for workspace glob tool
Commands run:
- cargo fmt --check: passed
- cargo clippy --all-targets -- -D warnings: passed
- cargo test: passed (41 tests across m1, m2, m3, m4, m5, m6, 0 failures)
Acceptance:
- `.gitignore` is respected: PASS (test_gitignore_is_respected)
- Path outside root is denied: PASS (test_path_outside_root_is_denied)
- Caps are enforced (24,000 bytes max with truncation marker): PASS (test_caps_are_enforced)
- Line slicing is exact 1-based inclusive: PASS (test_line_slicing_is_exact_1_based_inclusive)
- Binary files are reported, not dumped: PASS (test_binary_files_are_reported_not_dumped)
- Grep latency on 1,200-file fixture warm: PASS (test_grep_latency_on_1200_files_fixture_warm)
Measured:
- CBU §4.1 warm grep latency on 1,200-file fixture: 17.41 ms (spec target <= 150 ms) via `cargo test --test m6_tools test_grep_latency_on_1200_files_fixture_warm -- --nocapture`
- Tools test suite execution time: 0.13s (`cargo test --test m6_tools`)
Not done / limits:
- PageRank, vector search, and AST repo maps are deferred to post-MVP (Tier 2).
Spec questions:
- None for M6.
Not checked:
- None.
```

- [x] M7 (I7) Gate v0 + verification summary + `--skip-tests` audit entry.

```
Task: M7
Changed: src/lib.rs, src/gate/mod.rs, src/gate/classifier.rs, src/gate/tool_log.rs, src/gate/audit.rs, tests/m7_gate.rs
Dependencies added: none
Commands run:
- cargo fmt --check: passed
- cargo clippy --all-targets -- -D warnings: passed
- cargo test: passed (48 tests across m1, m2, m3, m4, m5, m6, m7, 0 failures)
Acceptance:
- Source edit with no test change: blocked with a reason: PASS (test_source_edit_no_test_change_blocked)
- Source plus test edit and passing command after last edit: allowed: PASS (test_source_plus_test_edit_and_passing_command_allowed)
- Edit after passing run: blocked (stale): PASS (test_edit_after_passing_run_blocked_as_stale)
- Test command not run: blocked ("not run"): PASS (test_test_command_not_run_blocked)
- Whitespace-only change is Class C and allowed with no test: PASS (test_whitespace_only_change_is_class_c_and_allowed_without_test)
- `--skip-tests` appends a valid chained entry; editing or deleting an earlier line breaks chain and blocks: PASS (test_skip_tests_audit_chain_integrity)
- Summary output marks stale and not-run correctly: PASS (test_summary_output_marks_stale_and_not_run_correctly)
Measured:
- Gate test suite execution time: 0.01s (`cargo test --test m7_gate`)
Not done / limits:
- Class A approximation: test file matching is based on test glob patterns; full semantic test assertion execution detection is post-MVP.
- Class B is empty per PRD FR-2.1.
- Audit truncation anchors (refs/devcode/audit-tail) deferred to post-MVP (A3).
Spec questions:
- None for M7.
Not checked:
- None.
```

- [x] M8 (I8) Approval + diff UX; shell lexical guard.

```
Task: M8
Changed: Cargo.toml, Cargo.lock, src/lib.rs, src/cli.rs, src/manifest/mod.rs, src/manifest/rollback.rs, src/approval/mod.rs, src/approval/diff.rs, src/approval/prompt.rs, src/approval/lexical.rs, src/approval/runner.rs, tests/m8_approval.rs
Dependencies added:
- similar (v2.7): unified diff computation and formatting
Commands run:
- cargo fmt --check: passed
- cargo clippy --all-targets -- -D warnings: passed
- cargo test: passed (54 tests across m1, m2, m3, m4, m5, m6, m7, m8, 0 failures)
Acceptance:
- Table test of >= 25 command strings (allowed, denied, ask) including heredocs containing `>`, multiple heredocs, quoted `>`, and redirects into `.devcode/`: PASS (test_lexical_guard_table_test_25_cases, 39 cases tested)
- Scripted run: a `run_command` that edits a tracked file via `python -c` is not blocked lexically but is detected by reconcile, invalidates the gate, and is rolled back by `devcode rollback`: PASS (test_scripted_run_reconcile_and_rollback, test_cli_rollback_subcommand_execution)
- Approval flow with piped stdin: once, always, reject: PASS (test_approval_flow_piped_stdin_once_always_reject)
- Guarded file edit workflow with boundary check, unified diff approval, and manifest pre/post capture: PASS (test_guarded_file_edit_workflow)
- Unified diff formatting: PASS (test_diff_formatting)
Measured:
- Approval test suite execution time: 0.15s (`cargo test --test m8_approval`)
Not done / limits:
- PowerShell/CMD specific cmdlets (Out-File, Set-Content) and herestrings are deferred to Windows native (A1).
- Shell AST parsing: lexical tokenization and heredoc stripping used per PRD FR-3.2 / arch §6.4. Gaps (e.g. dynamic runtime open in python/ruby/perl) backstopped by M3 guard-set snapshot and post-command reconcile.
Spec questions:
- None for M8.
Not checked:
- None.
```

- [x] M9 (P2/P3) Discipline v0 (scope, dependency flag) and token counter; PRD text for both.

```
Task: M9
Changed: src/lib.rs, src/agent/loop_runner.rs, src/gate/tool_log.rs, src/discipline/mod.rs, src/discipline/scope.rs, src/discipline/dependency.rs, src/discipline/compaction.rs, src/discipline/summary.rs, tests/m4_agent.rs, tests/m9_discipline.rs
Dependencies added: none
Commands run:
- cargo fmt --check: passed
- cargo clippy --all-targets -- -D warnings: passed
- cargo test: passed (58 tests across m1, m2, m3, m4, m5, m6, m7, m8, m9, 0 failures)
Acceptance:
- Out-of-scope edit prompts; in-scope edit does not: PASS (test_scope_contract_in_scope_and_out_of_scope)
- Dependency add prompts and logs the justification: PASS (test_dependency_addition_prompts_and_logs_justification)
- Compaction triggers at the threshold, keeps the pinned items, and preserves the last test result verbatim: PASS (test_compaction_triggers_at_threshold_and_preserves_pinned_and_test_result)
- End-of-task line numbers match the scripted run: PASS (test_end_of_task_summary_line_matches_scripted_run)
Measured:
- Discipline test suite execution time: 0.05s (`cargo test --test m9_discipline`)
Not done / limits:
- LLM-written summaries (compaction uses deterministic harness-built summary without extra model call).
- Automated tokens-per-task benchmark against a peer is deferred to post-MVP.
- Dependency "better than std" judgment (human decides at the prompt).
Spec questions:
- None for M9.
Not checked:
- None.
```

## After MVP (deferred; do not start before M1–M9 work)
- [ ] A1 Windows native: Job Object, path canonicalization, installer `.ps1` (arch §6.2, §6.4.4).
- [ ] A2 `debug` and `security` profiles (I9); gitleaks/semgrep/osv-scanner orchestration.
- [ ] A3 Audit-log truncation anchors; full Class A/C classifier details.
- [ ] A4 Installer and release: `install.sh`/`install.ps1`, checksums, minisign, Cloudflare Pages host (I10).
- [ ] A5 Product name; then claim names (GitHub repo, crate, Cloudflare Pages project). Until then the working title stays `DevCode`.
- [ ] A6 Optional: ATE-1000/ATSE-1000 as named agent-facing and user-facing languages (P1/P3/P4); details not final.
- [ ] A7 Tier 2 PageRank (CBU §4.4 evidence first).

## Current board (honest vs peers)

**Done rule:** `[x]` only when the feature **matches or beats** a named peer (codex / opencode / pi / gemini-cli) on the same job. Landed code without peer parity stays **started** or **partial**. Evidence notes (RES-*) record measurements; they do not auto-check boxes.

Peers for the bar: account UX → OpenCode/pi; OAuth/keyring → Codex/gemini-cli; sandbox → Codex; TUI streaming → Codex/gemini-cli; thinking → pi `/thinking`.

### Landed (code present; not peer-done)

MVP harness M0–M13 is on main (CLI loop, guard, manifest/rollback, gate, approvals, discipline, NIM live RES-012). That is product scaffolding, not a claim of peer parity.

### Started / partial — accounts & providers (RES-014)

| ID | Status | Landed | Still short of peer |
| :--- | :--- | :--- | :--- |
| P1 | partial | `--provider` + `apply_named_preset` for presets | OpenCode multi-integration map + switch accounts |
| P2 | partial | Gemini + NIM env keys | gemini-cli OAuth + keychain |
| P3 | partial | Plain `/provider` `/model` `/effort` | pi `/thinking` (all providers); effort is Gemini-only |
| P4–P5 | partial | `gemini_smoke` / `nim_smoke` (ignored live) | Not a peer bake-off; smoke ≠ product auth UX |
| P8 | partial | Default `gemini-3-flash-preview` | Catalog churn; no peer-grade model registry |
| P9 | partial | Prefetch + `App.models` cache on connect/switch | OpenCode durable model registry / variants |
| P10 | partial | Account rows stored\|env\|missing; connect persists; `openai` + `openrouter` presets | **No** OAuth, keyring, `auth switch`, native Claude/Anthropic |

- [ ] **P10a** Native Anthropic Messages adapter (not OpenRouter proxy) — bar: OpenCode/Cline Anthropic
- [ ] **P10b** OAuth / device-code login for at least one provider — bar: Codex / gemini-cli
- [ ] **P10c** OS keyring backend (optional file) — bar: Codex / gemini-cli HybridTokenStorage
- [ ] **P10d** Multi-credential + `auth switch` — bar: OpenCode `auth switch`
- [ ] **P10e** Decide env-vs-stored precedence vs pi (spec today: env wins)

### Started / partial — TUI (peer-quality; TDD; no OpenTUI port)

| ID | Status | Landed | Still short of peer |
| :--- | :--- | :--- | :--- |
| U1 | partial | Scroll pin + Ctrl-End | Full transcript UX vs Codex/opencode |
| U2 | started | Progress merge into one bubble | **SSE / true token stream** deferred |
| U3 | partial | Ctrl-T tool cards | Richer tool timeline vs peers |
| U4 | partial | Diff modal scroll clamp | Codex-level permission/diff chrome |
| U5 | partial | Ctrl-K command palette | OpenCode slash surface depth |
| U6 | partial | In-repo fuzzy scorer | Peer fuzzy quality + ranking polish |
| U7 | partial | Idle/working/blocked strip | |
| U8 | partial | Shift/Alt+Enter multiline | |
| U11 | partial | History vs scroll split (keys) | Full-screen ratatui E2E dogfood incomplete (RES-013) |
| U12 | partial | Identical tool-loop → NoProgress | |

- [ ] **U2a** Real SSE / streaming assistant tokens — bar: Codex / gemini-cli streaming
- [ ] **U13** Interactive full-screen TUI dogfood with live provider (not only `App` unit + `--plain`) — RES-013 gap

P6/P7 (scripted dogfood + harder Gemini build) = **evidence**, not peer-done checkboxes.

### Started / partial — runtime (not A-list yet)

- [ ] Sandbox = path allowlist + env strip + PGID kill only; **no** Seatbelt/bwrap/Landlock — bar: Codex
- [ ] Installer scripts exist; **no** minisign / peer-grade signed channel — bar: Codex
- [ ] Windows native — not started (A1)
- [ ] `debug` / `security` profiles — not started (A2)

### After MVP (deferred; still open)

- [ ] A1 Windows native: Job Object, path canonicalization, installer `.ps1`
- [ ] A2 `debug` and `security` profiles; gitleaks/semgrep/osv-scanner
- [ ] A3 Audit-log truncation anchors; Class A/C classifier depth
- [ ] A4 Installer + checksums + **minisign** + release host
- [ ] A5 Product name / claim names
- [ ] A6 Optional ATE/ATSE languages
- [ ] A7 Tier 2 PageRank (CBU evidence first)
- [ ] Ignore `__pycache__` in command-change counting (RES-013 #3)
- [ ] Peer tokens-per-task comparison

### Next work order (peer bar first)

1. P10a–P10d (accounts to OpenCode/Codex level) **or** U2a (real streaming) — pick one track
2. Sandbox toward Codex (bwrap/Seatbelt) when accounts/stream track is peer-grade
3. A4 signing when install path is real
