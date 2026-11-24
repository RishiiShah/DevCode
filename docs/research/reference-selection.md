# Research Note: Reference Selection Review (Pass 7)

```
================================================================================
DOCUMENT ID    : DEVCODE-RES-007
REVISION       : 0.1.0
STATUS         : Working notes (reference study)
SOURCES        : references/sources.tsv; GitHub REST API (unauthenticated), 2026-11-23; local clones
RELATED        : references/README.md, docs/research/*.md
================================================================================
```

## Question

Does `sources.tsv` hold the right harnesses for DevCode needs (loop, gates, rollback, sandbox, repo map, Windows, installer, TUI)? What is each best for, where to read, what do its issues and PRs say?

## Method and limits

- Evidence labels: **code** (local clone), **prod data** (GitHub API, 2026-11-23), **guess** (mine, unverified).
- Open counts below = issues + PRs (`open_issues_count`) and open PRs (`/pulls` last page). Codex ~28k open is almost all issues.
- "Hot threads" = 8 most-commented open items per repo. Biased to old threads. Not a defect-rate measure.
- Not checked: closed or merged PR velocity, issue labels, component licenses, claims about forks.

## Verdict per repo

| Repo | Lang | Stars | Open iss+PR / PR | Last push | Best for DevCode | Verdict |
| :--- | :--- | ---: | :--- | :--- | :--- | :--- |
| mini-swe-agent | Py | 10k | 154 / 107 | 11-16 | Minimal loop (221 files) | **Core** |
| codex | Rust | 136k | 28417 / 159 | 11-23 | Sandbox, exec policy, Windows sandbox, signed-sum installer | **Core** |
| gemini-cli | TS | 109k | 736 / 268 | 11-23 | Policy engine, sandbox manager, checkpointing, evals | **Core** |
| cline | TS | 72k | 1821 / 877 | 11-23 | Checkpoint restore + diff, SDK hooks | **Core** (rollback) |
| aider | Py | 50k | 1995 / 574 | **05-22** | Repo map, PageRank, edit formats | **Core, frozen** |
| pi | TS | 124k | 304 / 45 | 11-23 | TUI, extensions, Windows lessons | **Core** |
| opencode | TS+Rust | 229k | 4030 / 1313 | 11-23 | Sessions, TUI, diff approval | **Core** (UX) |
| kimi-code | TS | 8k | 1953 / 598 | 11-23 | Undo service, BOM/EOL, path redaction | Narrow |
| qwen-code | TS | 31k | 2544 / 563 | 11-23 | Shell/heredoc worktree guard | Narrow |
| mistral-vibe | Py+Rust | 5k | 540 / 196 | 11-20 | Permission decide, mode switching bugs | Narrow |
| swe-agent | Py | 21k | 221 / 151 | 11-16 | Trajectories, tool configs, untrusted-input RFC | Narrow |
| kilocode | TS | 28k | 965 / 310 | 11-23 | Snapshot, scoped approval settings | Narrow (overlaps opencode) |
| goose | Rust | 57k | 502 / 122 | 11-23 | MCP/extension model, shell output cap | Narrow |
| roo-code | TS | 24k | 1025 / 480 | 05-15 | Mode schema (already used) | **Archived**: frozen reference |
| continue | TS | 37k | 856 / 587 | 11-23 | IDE integration | **Drop candidate** |
| openhands | TS | 94k | 1243 / 594 | 11-23 | GUI platform | **Replace candidate** |
| stakpak | Rust | 2k | 45 / 15 | 07-06 | Small Rust CLI; secret-leak lesson | **Drop candidate** |

## Per-repo: best for, explore, issue/PR signal

- **mini-swe-agent**: read first, fully. `src/minisweagent/agents/default.py`. Hot: #829 bash tool launches Dash on Ubuntu (POSIX `sh` portability, matches installer rule); #953 RFC "deterministic execution boundaries for tool calls (task text is untrusted input)"; #1003 litellm release broke every tool run (dependency risk).
- **codex** (code): `codex-rs/sandboxing`, `linux-sandbox`, `windows-sandbox-rs`, `windows-sandbox-service`, `mxc-sandbox`, `core/src/exec.rs`, `git-utils`, `scripts/install/install.{sh,ps1}`. Hot: #51601 Windows sandbox setup sharing violation; #20214, #51524 Windows freezes and tool gaps; #9203 "make /undo back" (users want rollback; supports FR-3); #41220 quota accounting.
- **gemini-cli**: `packages/core/src/policy`, `services/sandboxManager.ts`, `integration-tests/checkpointing.test.ts`, `evals/`. Hot: #16750 "Allow for session" approves only once for shell commands containing a path (approval-cache bug class); PR #29396 AST-aware structural search tool (Tier 2 evidence); PR #29393 persistent task tracking.
- **cline**: `sdk/packages/core/src/session/checkpoint-restore.ts`, `checkpoint-diff.ts`, `hooks/checkpoint-hooks.ts`. PRs: #15049 edits limited to project directory via Microsoft MXC (same name as codex `mxc-sandbox`; relation unverified); #12543 repeated tool-call loop detection; #14609 queued prompt during teardown.
- **aider**: `aider/repomap.py`, `coders/*`. Frozen 6 months while peers push daily; 574 PRs open. Use as algorithm reference only. Hot: #3362 "Inspiration from Claude Code" (project itself pointing to the harness class DevCode targets).
- **pi**: `packages/tui`, `packages/coding-agent/examples/extensions/git-checkpoint.ts`, `packages/tui/src/undo-stack.ts`. Lowest PR backlog (45) = curated. Hot: #7547 "How do you use Pi on Windows?" (100 comments; Windows pain evidence); #7730 high CPU on macOS in long sessions; #9549 full re-render per frame (TUI cost).
- **opencode**: `packages/tui`, session execution. 1313 open PRs = slow merge. Hot: #4714 session search; #59387 free-tier thread (cost, not design). Source of billing and quota complaints, not harness design.
- **kimi-code**: `packages/agent-core-v2/src/agent/undo/{undoService,turnUndo}.ts`. PR #3946 preserve BOM and line endings on Edit/Write (byte-exact; feeds NFR-6); PR #2516 redact UNC and `C:/` paths in telemetry (Windows paths); PR #4655 auto review.
- **qwen-code**: PR #9417 "fail closed on expanding heredoc bodies in the worktree guard" (70 comments; direct input to shell write policy, gap 3); `integration-tests/interactive/tui-checkpoint-restore.test.ts`. Issue #14724 is a CI gate-failure tracker with 545 comments (noise). Likely gemini-cli derivative (guess); read after gemini-cli.
- **mistral-vibe**: `vibe/app_server/_unified_permissions.py`, `harness/core/src/core/turn.rs`. Hot: #1144 cannot return to plan agent after switching (profile-switch state bug class); #255 consecutive user messages after interrupt.
- **swe-agent**: `sweagent/`, `tools/`, `config/`. #1526 same untrusted-input RFC as mini-swe-agent #953. #1240 broken cost control.
- **kilocode**: `packages/core/src/snapshot.ts`. PR #13897 "Security Auto Mode"; PR #14228 scoped config edit approval; PR #14811 serialize index writes. Layout resembles opencode (guess).
- **goose**: `crates/goose/src/agents`, `test_acp_client.py`. PR #11795 caps shell capture memory (NFR-4 analog); PR #12968 per-model compact limit; installer has no checksum step (code).
- **roo-code**: `archived: true` on GitHub (prod data); `references/README.md` does not say so. RES-004 conclusions stand; no upstream fixes will arrive.
- **continue**: IDE extension; oldest hot issues from 2024 (#1729 API keys in plain text, #917 Neovim). Earlier catalog says upstream read-only; API shows pushes today and 587 open PRs; unverified which is true.
- **openhands**: local clone has 18 Python files vs 1299 TS: this repo is now mostly GUI/frontend (code). Agent core likely lives elsewhere (guess; e.g. an SDK repo, unverified).
- **stakpak**: smallest and least adopted; 15 open PRs. Issue #744 API key leaked in error log (lesson for env stripping and log redaction).

## Gaps in the current set (not yet studied, already on disk)

1. **Rollback peers**: cline, gemini-cli, kimi-code, qwen-code, codex `git-utils`, pi `git-checkpoint.ts`. RES-002 studied only aider (rejected). FR-3 is the core differentiator and has no peer comparison.
2. **Windows**: codex `windows-sandbox-rs`, pi #7547, kimi-code path redaction. No Windows note exists; arch §6.2 is design-only.
3. **Untrusted repo content**: mini-swe-agent #953 and swe-agent #1526 (same RFC) match arch tenet 3.

## How to explore each (cheap)

1. One question per pass; list target files from this note; Haiku scouts return `file:line` facts only; Opus reviews the decision.
2. Prefer repos' own tests as spec (`checkpointing.test.ts`, `tui-checkpoint-restore.test.ts`, `test_install_sh.py`).
3. Read hot issues for a given feature with the search API (`repos/<o>/<r>/issues?q=...`); unauthenticated limit is 60 requests/hour, so batch.
4. Record per note: SHA, file:line, steal/reject, stop condition (existing RES format).

## Actions proposed (not applied)

- Add archived flag and last-push column to `references/README.md` and `sources.tsv`.
- Next pass RES-008: rollback peers. Then RES-009: Windows runtime.
- Decide drop/replace for continue, openhands, stakpak. Clones total 3.4G; nothing deleted.
