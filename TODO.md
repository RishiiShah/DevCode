# [ATE-1004] DevCode To-Do: Work Tracker

```
================================================================================
STANDARD       : ATE-1000 (Automated Technical Engineering Standard Series)
DOCUMENT ID    : ATE-1004-TODO
REVISION       : 0.3.0
STATUS         : Living tracker (non-normative; specs win on conflict)
AUTHOR         : DevCode Core Engineering Group
OWNS           : What is open, what is partial, what comes next
SECTIONS       : 2 open work (read this) | 3 landed, unverified | 4 post-MVP |
                 5 closed ledger (skip unless tracing history)
RELATED DOCS   : [docs/mvp.md](docs/mvp.md), [docs/prd.md](docs/prd.md),
                 [docs/devcode.md](docs/devcode.md), [AGENTS.md](AGENTS.md)
UPDATED        : 2026-11-26
================================================================================
```

---

## 1.0 Rules

1. One item at a time, in order. An item is done when the edit is made, verified, and "not checked" is reported (report template: `docs/mvp.md` §5).
2. No commits unless asked. `references/` stays gitignored.
3. **Peer-bar rule**: `[x]` on the board only when the feature **matches or beats** a named peer (codex, opencode, pi, gemini-cli) on the same job. Landed code without parity stays `[ ]` with a **Landed** note. Research notes (`RES-*`) record evidence; they never auto-check a box.
4. Peers by area: account UX = OpenCode/pi; OAuth and keyring = Codex/gemini-cli; sandbox = Codex; TUI streaming = Codex/gemini-cli; thinking = pi `/thinking`.
5. Keep entries to one line plus pointers: spec section, test file, RES note. Full receipts belong in the PR or in git history, not here.

---

## 2.0 Open Work (Peer Bar)

### 2.1 Next work order
1. U13: full-screen TUI dogfood with a live provider. User dogfood found the streaming duplicates and the compaction wrong-question bug; unit tests and render previews did not.
2. P11: real context window per model. The fixed 32k preset window made compaction run nearly every step.
3. U2a remainder: streamed tool-argument UI, `--plain` streaming, live keyed run.
4. P10a–P10d OAuth breadth (Anthropic/Codex OAuth, device code, keyring default): no user-hit gap yet; after 1–3.
5. Sandbox toward Codex (bwrap / Seatbelt); A4 signing when the install path is real.

Landed baseline: MVP harness M0–M13 (CLI loop, guard, manifest/rollback, gate, approvals, discipline, NIM live per RES-012). That is scaffolding, not peer parity.

### 2.2 Accounts and providers (RES-014)

| ID | Status | Landed | Short of peer |
| :--- | :--- | :--- | :--- |
| P1 | partial | `--provider` + `apply_named_preset` | OpenCode multi-integration map, account switching |
| P2 | partial | Gemini + NIM env keys | gemini-cli OAuth + keychain |
| P3 | partial | `/provider` `/model` `/effort` (Gemini; Anthropic `thinking`; OpenAI/OpenRouter `reasoning_effort`) | pi `/thinking`; per-provider custom model metadata; live keyed Anthropic/OpenAI runs |
| P4–P5 | partial | `gemini_smoke` / `nim_smoke` (ignored, live) | Smoke is not a peer bake-off or auth UX |
| P8 | partial | Default `gemini-3-flash-preview` | Catalog churn; no peer-grade model registry |
| P9 | partial | Prefetch + `App.models` cache on connect/switch | OpenCode durable registry and variants |
| P10 | partial | Account rows stored\|env\|missing; connect persists; `openai` + `openrouter` presets; Gemini OAuth; opt-in keyring; named slots + `auth switch` | See P10a–d |
| P11 | open | `context_window` per provider config; presets default 32k; status line shows "% context left" | Per-model window (catalog metadata or table); too-small window triggers compaction every step |
| P12 | open | Model picker lists the provider catalog | Hide or mark non-chat models (embedding, guard, detector) |

- [ ] **P10a** Native Anthropic Messages adapter (bar: OpenCode/pi). *Landed*: `src/agent/anthropic.rs`, preset `anthropic`, object `tool_choice` (omitted with empty `tools`), `interleaved-thinking` beta header, `/effort` → `thinking.budget_tokens`; tests `m20_anthropic`. *Missing*: Anthropic OAuth, forced SSE on `complete()`, live keyed dogfood.
- [ ] **P10b** OAuth / device-code login for at least one provider (bar: Codex, gemini-cli). *Landed*: Google OAuth in `src/auth.rs` (`auth login gemini --oauth`, loopback callback, `NO_BROWSER=1` / `--with-stdin` paste-code), `oauth-gemini.json`, auto refresh on expiry and `401`, Code Assist backend `src/agent/code_assist.rs` (`gemini:oauth` slot or fallback); tests `m21_oauth`, `m25_code_assist`. *Missing*: device-code flow, non-Gemini OAuth (Codex, Anthropic), live Google OAuth dogfood.
- [ ] **P10c** OS keyring backend (bar: Codex, gemini-cli HybridTokenStorage). *Landed*: opt-in `DEVCODE_CREDENTIALS_STORE=auto` (service `devcode`, account `credentials`), falling back to `credentials.json`; unit tests use an in-memory blob. *Missing*: file is still the default; no Windows backend; `oauth-gemini.json` stays a file; one keyring blob.
- [ ] **P10d** Multi-credential + `auth switch` (bar: OpenCode). *Landed*: `nim` or `nim:alias` slots; `UserSettings.active_slot`; `auth switch` selects a stored key without changing the provider; `/connect nim:work`. *Missing*: env still wins without a switch, OAuth only for exact `gemini`, one keyring blob, no OpenCode multi-account map.
- [x] **P10e** Env-vs-stored precedence. *Decision (deliberate difference from pi/OpenCode)*: an explicit `auth switch` or `/connect nim:alias` beats the env var; with no switch, env beats stored (12-factor/CI). `credentials::switched_key`; tests `m22_auth_switch`, `m12_provider`.

### 2.3 TUI (peer quality; TDD; no OpenTUI port)

| ID | Status | Landed | Short of peer |
| :--- | :--- | :--- | :--- |
| U1 | partial | Scroll pin, Ctrl-End | Full transcript UX vs Codex/opencode |
| U2 | started | One progress bubble; SSE text + reasoning deltas (OpenAI, Anthropic, Code Assist); Ctrl-C cancel | Streamed tool-argument UI (U2a) |
| U3 | partial | Grouped tool lines (Explored/Ran/Edited, +N −M), Ctrl-T expands to cards and diffs | Expand any historical group, not only the latest |
| U4 | partial | Codex-style docked approval panel (numbered diff, option list, Enter = No on risky commands); edit diffs in transcript | Scoped always-allow command rules |
| U5 | partial | Ctrl-K palette; live slash suggestions; docked pickers open on the current choice | OpenCode slash-surface depth |
| U6 | partial | In-repo fuzzy scorer | Peer fuzzy quality and ranking |
| U7 | partial | Activity row above input; status line under it (`/status`, `/statusline`: model, context left, tokens, branch, folder, background processes) | n/a |
| U8 | partial | Shift/Alt+Enter multiline | n/a |
| U11 | partial | History vs scroll key split | Full-screen ratatui E2E dogfood (RES-013) |
| U12 | partial | Identical tool-loop → NoProgress | n/a |

- [ ] **U2a** Real SSE streaming of assistant tokens (bar: Codex, gemini-cli). *Landed*: `Model::complete_stream_cancellable`, `src/agent/sse.rs`, OpenAI (`reasoning_content`/`reasoning`), Anthropic (`text_delta`/`thinking_delta`/`input_json_delta`), Code Assist (`thought` + text + `functionCall`) with chunk-safe key redaction, `CancelToken` mid-stream/mid-command Ctrl-C (`ExitReason::Cancelled`), `Item::Reasoning`; TUI-only, off via `DEVCODE_STREAM=0`; tests `m24_*`, `m25_code_assist`. *Missing*: streamed tool-argument UI, live keyed run, `--plain` streaming.
- [ ] **U13** Interactive full-screen TUI dogfood with a live provider (not only `App` unit tests and `--plain`). Gap from RES-013.
- [ ] **U14** Syntax highlighting & themes (bar: Codex, OpenCode, Mistral Vibe). *Missing*: `syntect` code-fence and diff highlighting in `src/tui/markdown.rs` and `src/tui/view.rs`, italics/links/task-lists, `/theme` picker, `OSC 11` light/dark background auto-detection.
- [ ] **U15** Composer `@` mentions, `!` shell mode & editor ergonomics (bar: Codex, OpenCode, Pi, Gemini CLI). *Missing*: `@` fuzzy file-mention popup, `!` shell execution mode, `$EDITOR` prompt launch, `[paste #N]` collapse, clipboard image paste, `Ctrl-R` persistent history search, optional Vim mode.
- [x] **U16a** Conversations saved per prompt to `<state>/conversations/`; `devcode -c`, `--resume [ID]`, `/resume` picker replays the transcript (ARCH §4.3; `src/conversation.rs`, `tests/m28_resume.rs`). Harness state moved out of the workspace after `rm -rf` deleted `.devcode/blobs` (`tests/m27_snapshots.rs`).
- [ ] **U16** Session & compaction UI (bar: OpenCode, Pi, Codex, Cline; `RES-005`). *Landed*: `/resume` picker (U16a). *Missing*: `/fork` (`/tree`) branching, `/compact` UI trigger (core in `src/discipline/compaction.rs`), `/export`, double-`Esc` prompt `/rewind`.
- [ ] **U17** Approval, diff & tool-card depth (bar: OpenCode, Codex, Kimi Code, Stakpak). *Landed*: reject then type the correction, delivered mid-run (`SteerInbox`). *Missing*: scoped command-pattern always-allow rules, standalone `/diff` viewer (unified + split), expanding/collapsing arbitrary historical tool cards beyond latest `Ctrl-T`.
- [ ] **U18** Terminal protocols & polish (bar: Codex, Pi, Mistral Vibe, OpenCode). *Missing*: synchronized output (`\x1b[?2026h`/`l`), transcript scrollbar thumb, double/triple-click selection, `OSC 8` clickable links, Kitty keyboard protocol, `OSC 9`/`777`/BEL completion alerts, `--no-alt-screen` inline scrollback mode.

P6/P7 (scripted dogfood and the harder Gemini build) are **evidence**, not peer-done boxes.

### 2.4 Runtime and release
- [ ] Sandbox is path allowlist + env strip + PGID kill only; **no** Seatbelt/bwrap/Landlock (bar: Codex).
- [ ] Installer scripts exist; **no** minisign or peer-grade signed channel (bar: Codex).
- [ ] Windows native: not started (A1).
- [ ] `debug` / `security` profiles: not started (A2).
- [ ] Ignore `__pycache__` in command-change counting (RES-013 #3).
- [ ] Peer tokens-per-task comparison.

---

## 3.0 Landed in Working Tree, Not Verified

None. The 2026-11-26 TUI batches are committed (from `b8eae92`) and pass `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings` and `cargo test`; they are folded into §2.2–§2.3. Peer parity still needs U13.

---

## 4.0 After MVP (Deferred; Open)

- [ ] A1 Windows native: Job Object, path canonicalization, installer `.ps1` (ARCH §6.2, §6.4.4).
- [ ] A2 `debug` and `security` profiles (I9); gitleaks/semgrep/osv-scanner orchestration (CBU §5).
- [ ] A3 Audit-log truncation anchors (ARCH §5.2); full Class A/C classifier depth.
- [ ] A4 Installer and release: `install.sh`/`install.ps1`, checksums, **minisign**, release host (ARCH §6.3, I10).
- [ ] A5 Product name, then claim names (GitHub repo, crate, Cloudflare Pages project). Working title stays `DevCode`.
- [ ] A6 Optional: ATE/ATSE as named agent-facing and user-facing languages (P1/P3/P4); not final.
- [ ] A7 Tier 2 PageRank, only after CBU §4.4 evidence.

---

## 5.0 Closed Ledger (History; Skip Unless Tracing)

Full per-task receipts (commands, test counts, acceptance names) for M1–M9 were removed from this file in 0.2.0. Recover them with `git show 0eae334:TODO.md`.

### 5.1 Spec and research (closed)
- [x] D1 `.gitignore`: `references/` ignored (user-set).
- [x] D2 `.devcode/` write exclusion + tamper-evident audit log (PRD FR-2.3, FR-4.1, Gate 2; ARCH §5.2).
- [x] D3 Cross-platform matrix + curl/PowerShell installer spec (ARCH §6.2–6.3; PRD FR-6, Gates 6–7).
- [x] D4 Reference selection review (RES-007).
- [x] D5 `ask` profile added (PRD FR-1.2/1.3, ARCH §3, §3.1).
- [x] D6 Shell edits recoverable: guard-set snapshot before mutation-capable commands (ARCH §6.4.5, PRD FR-3.2, NFR-9, Gate 8).
- [x] D7 Windows path canonicalization: strip `\\?\` instead of rejecting; new-file, containment, device-path rules (ARCH §6.2, §6.4.4, RES-009).
- [x] D8 Overclaims removed: Rust does not "guarantee" NFR-1/2; no SLSA level; static-link wording (PRD FR-6.3/6.4, ARCH §6.3).
- [x] D9 Revisions bumped; duplicate `docs/research/README.md` removed.
- [x] T1 RES-008 rollback peers → steal/reject vs FR-3.
- [x] T2 Shell write policy (FR-3.2 / ARCH §6.4), Windows included.
- [x] T3 `security` profile patch path: harness applies approved diffs.
- [x] T4 Command blacklist reframed as UX hint; policy + approvals are control.
- [x] T5 Class B empty in v1.
- [x] T6 RES-009 Windows runtime → ARCH §6.2.
- [x] T7 Spec number fixes (Tier 2 300; NFR-6/8 rows).
- [x] T8 Audit-log truncation anchor (state dir + `refs/devcode/audit-tail`).
- [x] T9 Reference catalog upkeep (continue/openhands/stakpak dropped from active study).
- [x] T10 Implementation language: Rust (PRD FR-6.3).
- [x] T11 Minisign signing spec (PRD FR-6.4); invented host replaced by placeholders.
- [x] T12 Demand evidence rewritten (RES-010 0.2.0): gate demand is a hypothesis; rollback and approvals stronger.
- [x] T13 Doc IDs: `ATE-100x` normative, `DEVCODE-RES-00x` informative.
- [x] T14 ARCH §4 aligned with §6.4 / FR-3.2.
- [x] T15 TODO hygiene; implementation queue opened.
- [x] T16 Docs lightened (0.2.0): duplicated mechanism text replaced by section references; `AGENTS.md` read map added; `docs/mvp.md` → `ATE-1005-MVP`; this file → `ATE-1004-TODO`. Revisions: ARCH/PRD 0.4.1, CBU 0.3.2.

### 5.2 MVP build (all landed; spec in `docs/mvp.md`)
Plan: macOS and Linux first; profiles `ask`/`plan`/`build`; provider NVIDIA NIM via the OpenAI-compatible adapter, Gemini second.

- [x] M0 Decide: provider NIM; scope confirmed.
- [x] M1 Scaffold: CLI stub, config loader, CI skeleton. `tests/m1_scaffold.rs`. Not checked then: Linux CI run (awaiting push).
- [x] M2 Runtime guard (POSIX). `tests/m2_guard.rs`. Deferred: Windows Job Objects and paths (A1), Linux bwrap network isolation. Not checked then: `/bin/ps` inside the restrictive sandbox (verified via `killpg(pid, 0)`).
- [x] M3 Step-manifest engine incl. guard-set snapshot. `tests/m3_manifest.rs`. Gates 1, 4, 8 manifest side. Deferred: `devcode gc`, Windows lock backoff (A1).
- [x] M4 Agent loop + provider adapter; limits; format-error recovery. `tests/m4_agent.rs`, `tests/m10a_*`. Cost-limit enforcement deferred until pricing is known.
- [x] M5 Profile controller `ask`/`plan`/`build`. `tests/m5_profiles.rs`. Not checked then: live TTY prompt loop (covered by `handle_slash_command` and scripted tests).
- [x] M6 JIT tools: glob, grep, surgical read. `tests/m6_tools.rs`.
- [x] M7 Gate v0 + verification summary + `--skip-tests` audit chain. `tests/m7_gate.rs`. Class A approximation by test-glob; anchors deferred (A3).
- [x] M8 Approvals, diff UX, shell lexical guard (39-case table). `tests/m8_approval.rs`. PowerShell cmdlets deferred (A1); dynamic runtime writes backstopped by reconcile.
- [x] M9 Discipline v0 (scope, dependency flag), token counter, compaction. `tests/m9_discipline.rs`. No LLM-written summaries; no peer tokens-per-task benchmark.
- [x] M10a–h Live-run fixes after first NIM runs (RES-012). `tests/m10a_*` to `m10h_*`.
- [x] M11 Agent-changed tests reported in the verification summary. `tests/m11_test_reporting.rs`.
- [x] M12 Stored credentials and `devcode auth`. `tests/m12_credentials.rs`, `tests/m12_provider.rs`.
- [x] M13 Typed-ahead flush at login, `/connect`, `/connections`. `tests/m13_connect.rs`.
- [x] Post-MVP additions landed (see §2 for what is still short): multi-provider presets and user settings (`m15`, `m16`, `m18`), progress sink (`m14`), prompt style (`m17`), TUI dogfood (`m19`), Anthropic (`m20`), OAuth (`m21`), `auth switch` (`m22`), managed processes (`m23`), streaming (`m24_*`), Code Assist (`m25`).

### 5.3 Measured (MVP, with commands)

| Metric | Value | Target | Command |
| :--- | :--- | :--- | :--- |
| NFR-8 watchdog kill | 15.42 ms | <= 100 ms | `cargo test --test m2_guard test_run_bounded_timeout_kills_process_group -- --nocapture` |
| NFR-9 snapshot, 1,200 files | first 5.07 s, incremental 33.58 ms | first <= 2 s, incremental <= 250 ms | `cargo test --test m3_manifest test_measured_snapshot_time_1200_files -- --nocapture` |
| CBU §4.1 warm grep, 1,200 files | 17.41 ms | <= 150 ms | `cargo test --test m6_tools test_grep_latency_on_1200_files_fixture_warm -- --nocapture` |

The first-snapshot figure (5.07 s) is above the 2 s NFR-9 target. PRD §4.0 marks NFR-9 targets as unmeasured starting values, so this is a tuning item, not a failed gate.
