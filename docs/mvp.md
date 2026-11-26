# [ATE-1005] MVP Implementation Plan

```
================================================================================
STANDARD       : ATE-1000 (Automated Technical Engineering Standard Series)
DOCUMENT ID    : ATE-1005-MVP
REVISION       : 0.2.0
STATUS         : Working plan (implementers: agents; verifier: maintainer).
                 Tasks M0-M13 landed; status lives in TODO.md, not here.
OWNS           : Per-task deliverables and acceptance (M1-M13), implementer
                 rules, dependency allowlist, report template
SECTIONS       : 2 rules | 3 layout + deps | 4 common acceptance | 5 report |
                 6 tasks (M4 = provider adapter, M12 = key resolution)
SPEC           : ATE-1001-PRD (FR/NFR/Gates), ATE-1000-ARCH (design)
NOTES          : docs/research/*.md are informative; the specs are normative
================================================================================
```

Read only the task you are working on plus §2, §4, §5. Spec sections are cited per task; open those, not the whole spec.

## 1. Goal

A CLI an engineer can run on a real repository that shows the thesis: a **minimal, verified, reversible** change at **low token cost**. macOS and Linux only. Windows, `debug`/`security` profiles, installer, and naming come after the MVP (TODO.md §4).

## 2. Rules for implementers

1. One task at a time, in order. Do not start the next until the current one passes §4.
2. Smallest complete change. No speculative options or abstractions. No code for post-MVP items.
3. Standard library first. Add a dependency only when it clearly beats writing it (§3). Justify every dependency in the task report; unlisted ones need approval.
4. Tests never call a paid or networked LLM and never touch real credentials. Use `ScriptedModel` (M4). A manual smoke test against a real provider needs maintainer approval first.
5. Never read real `.env*` files. Never log API keys.
6. Do not edit `docs/prd.md` or `docs/devcode.md` while implementing. If the spec is wrong or silent, stop and report under "Spec questions".
7. No commits unless asked. Keep generated or large files out of the repo.
8. Done means every acceptance item passes and the §5 report is written.

## 3. Layout and dependencies

One Cargo package `devcode` (library + binary). Modules, not crates, until a split is justified. Layout is the `src/` tree itself; task ownership by directory: `guard/` M2, `manifest/` M3, `agent/` M4, `profile.rs` M5, `tools/` M6, `gate/` M7, `approval/` M8, `discipline/` M9, `credentials.rs`+`guard/tty.rs` M12–M13. One test file per task in `tests/`.

Starting dependency list (edition 2021, stable Rust):

| Need | Use | Why not std |
| :--- | :--- | :--- |
| CLI args | `clap` (derive) | Subcommands, help, exit codes |
| JSON | `serde`, `serde_json` | No JSON in std |
| Hashing | `sha2` | No SHA-256 in std |
| Ignore rules and walking | `ignore` | Correct `.gitignore` semantics |
| Regex | `regex` | No regex in std |
| Unified diff | `similar` | Diff algorithm |
| Process group kill | `libc` (`killpg`) | Std has `process_group` but no group kill |
| HTTP (M4 adapter only) | `ureq` (blocking) | No HTTP client in std; avoids an async runtime |
| Tests | `tempfile` (dev) | Temp dirs |

Not allowed without approval: async runtimes, `anyhow`/`thiserror` (use a plain error enum), logging frameworks (use `eprintln!`), tokenizers (use provider-reported usage).

## 4. Common acceptance (every task)

- `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`, `cargo test` pass on macOS and Linux (CI matrix).
- Each acceptance bullet has a deterministic test that runs in temp directories.
- No `unsafe` except calls into `libc` in `guard/`, each with a comment stating why it is sound.
- Public functions return `Result`; no `unwrap`/`expect` outside tests.

## 5. Task report

Paste into the PR or TODO.md under the task:

```
Task: Mx
Changed: <files>
Dependencies added: <crate: reason>   (or "none")
Commands run: <fmt/clippy/test summary>
Acceptance: <each bullet: pass/fail + test name>
Measured: <numbers, with the command that produced them>
Not done / limits: <explicit>
Spec questions: <explicit>
Not checked: <explicit>
```

## 6. Tasks

Tests: `tests/mN_*.rs`. Each task lists only what is not obvious from the spec or the code.

### M0. Decisions (maintainer)
First provider is **NVIDIA NIM** through one OpenAI-compatible chat-completions adapter (`kind = "openai_compat"`). The same adapter serves any endpoint with that API shape (NIM hosted or self-hosted, OpenAI, OpenRouter, vLLM, Ollama) by configuration only. Native adapters for different API shapes (Anthropic Messages, Gemini) plug into the same `Model` trait. Open: pick the NIM model (M4).

### M1. Scaffold
Spec: ARCH §7.4, PRD FR-1.3, FR-6.1.
- Package `devcode`, lib + bin, CI (macOS and Linux: fmt, clippy, test).
- CLI: `devcode [--profile ask|plan|build] [PROMPT]`, `devcode rollback [--step N]`, `--version`, `--help`. Unimplemented bodies return "not implemented", exit 1.
- `.devcode.json` loader: `verification.test_command`, `verification.test_globs`, `sandbox.timeout_seconds` (default 30). Unknown fields are an error naming the field; missing file gives defaults.
- Acceptance: `--version` exits 0; unknown flag exits 2; config tests for valid, missing, unknown field, wrong type.

### M2. Runtime guard (POSIX)
Spec: PRD FR-4.1–4.3, NFR-8; ARCH §6, §6.4.
- `canonicalize(root, path)`: resolves symlinks and `..`; for a missing path canonicalizes the nearest existing parent then appends the checked remainder; outside the canonical root is an error.
- `.devcode/` deny: any canonical path inside `<root>/.devcode` is denied for agent writes, compared case-insensitively on every OS.
- `strip_env`: removes `AWS_*`, `GITHUB_TOKEN`, `*_API_KEY`, `*_TOKEN`, `SSH_*`; keeps `PATH`, `HOME`.
- `run_bounded(argv, cwd, timeout, out_cap)`: `process_group(0)`, stripped env, merged stdout/stderr; timeout sends SIGTERM to the group then SIGKILL after a grace period; output beyond `out_cap` keeps head and tail with a marker. Default cap 24,000 bytes (about NFR-4's 6,000 tokens) [guess; configurable].
- Acceptance: symlink-out, `../`, and `.DevCode/x` denied; new file in an existing subdir allowed; `sh -c 'sleep 1000 & sleep 1000 & wait'` with a 1 s timeout leaves no process (report time to all-dead; NFR-8 target 100 ms is reported, not asserted); 1 MB output returns at most the cap.
- Non-goals: Windows Job Object, network isolation, command policy.

### M3. Step-manifest engine
Spec: PRD FR-3.1–3.5, NFR-6/7/9, Gates 1, 4, 8; ARCH §4, §6.4.5; RES-008.
- Storage: `.devcode/manifests/<session_id>/<step_id>.json`; blobs content-addressed under `.devcode/blobs/<sha256>`. Write blobs and manifest, flush, fsync file and directory before the mutation proceeds; capture failure refuses the mutation.
- Per path: existence, type, symlink target, mode bits, SHA-256, blob reference; post-step state after the mutation.
- `capture`, `record_post`, `rollback(session, step)`: reverse order; preflight against post-state and abort unchanged on any mismatch or incomplete record; restore bytes, type, mode; remove an agent-created path only if unchanged; never read `HEAD` or the index, never run index-mutating Git.
- Guard-set snapshot: `ignore` enumerates non-ignored files (without Git use the derived-directory list `target`, `node_modules`, `dist`, `build`, `__pycache__`, `.cache`), excluding `.devcode/`; 10 MB per-file cap; fingerprint (type, size, mtime ns, mode); store only changed paths; budget 5,000 files and 50 MB blobs (NFR-7), over budget returns `BudgetExceeded`. `reconcile(before)` returns `{created, modified, deleted}`.
- Acceptance: Gates 1, 4, and 8 (manifest side) per PRD §5.0, including fresh-process persistence, no-commit repo, non-Git directory, injected capture and restore failures. Report snapshot time on a 1,200-file fixture (first and incremental; NFR-9 reported, not asserted).
- Non-goals: audit log (M7), `gc`, Windows.

### M4. Agent loop and provider trait
Spec: PRD FR-1.1; ARCH §7.1; RES-001.
- `trait Model { fn complete(&self, req: &Request) -> Result<Response, ModelError>; }`. `Request`: system prompt, messages, tool schemas. `Response`: text, tool calls, `usage { input_tokens, output_tokens }`. `ScriptedModel` replays a JSON script for tests.
- Loop: query, parse tool calls, execute via the registry, append observations, repeat until no tool call or a limit trips. Defaults are guesses: 50 steps, 30 min wall time, 3 consecutive format errors; cost not enforced until pricing is known. Each limit ends the run with a distinct exit reason.
- Token accounting: sum provider-reported usage; one end line (steps, input/output tokens).
- **Provider config** (also settable in a user-level file; project values win):
  ```json
  { "provider": { "kind": "openai_compat",
                  "base_url": "https://integrate.api.nvidia.com/v1",
                  "api_key_env": "NVIDIA_API_KEY",
                  "model": "<model id from the provider's catalog>",
                  "context_window": 128000 } }
  ```
  `kind` accepts `openai_compat` and `anthropic` (native Messages API, preset `anthropic`); other values error with the supported list. `--provider nim` is a preset filling `base_url` and `api_key_env`. `model` and `context_window` have no default (catalogs and limits change). `api_key_env` names a variable (may be empty for a keyless local endpoint); a key value in config is rejected. Unknown fields inside `provider` are an error.
- **OpenAI-compatible adapter** (`src/agent/openai_compat.rs`, `ureq`):
  - Request: `POST {base_url}/chat/completions`, `Authorization: Bearer <key>` (omitted when `api_key_env` is empty), body `{ model, messages, tools, tool_choice: "auto", stream: false }`; tools use `{"type":"function","function":{name,description,parameters}}`. With a progress sink (TUI) and `DEVCODE_STREAM` not in `0/false/off/no`, `complete_stream` sends `stream: true` with `stream_options.include_usage` and parses SSE deltas; non-SSE replies and HTTP 400 fall back to `complete`.
  - Response: `choices[0].message.content` and `tool_calls[].function.{name, arguments}` (`arguments` is a JSON **string**); `usage.prompt_tokens`/`completion_tokens` map to input/output tokens; read `finish_reason`.
  - Failures: 429 and 5xx retry with exponential backoff (honor `Retry-After`; at most 5 tries for 429, 2 for 5xx); other 4xx fail at once with the body's message; connect timeout 10 s, read timeout 120 s. Invalid `arguments` JSON, an unknown tool name, or a response with neither text nor tool calls is a format error (3 in a row ends the run). A model that never returns tool calls gets a message that it may lack tool-calling support.
  - Keys: read the env var at startup; never read `.env` files; never log or print the key; redact it from error text; `strip_env` keeps it out of child processes.
  - Rate limits: hosted NIM trial accounts are reported at about 40 requests/minute and 1,000 credits (forum posts, unconfirmed). Optional `provider.max_rpm` spaces requests; 429 handling is the backstop.
  - Tests use a fake server on `std::net::TcpListener` (no new dependency). A live NIM smoke test needs maintainer approval first.
  - Other OpenAI-shaped providers are config-only (untested, confirm against docs): NIM self-hosted `http://localhost:8000/v1` (no key), OpenAI `https://api.openai.com/v1` (`OPENAI_API_KEY`), OpenRouter `https://openrouter.ai/api/v1` (`OPENROUTER_API_KEY`), Ollama `http://localhost:11434/v1` (no key).
- Acceptance (`ScriptedModel`): request body and tool schema fields; key only in the header, in no log or error text; 200 with tool calls parses; non-JSON `arguments` is a format error; 429 with `Retry-After` retries then succeeds; 429 over the cap fails clearly; 401 fails at once; connect timeout fails cleanly; unsupported `kind` rejected at startup; no-tool response ends the run; step, wall-time, and format-error limits end the run with matching reasons; tool errors become observations; token totals equal scripted usage.
- Non-goals at M4: streaming, non-OpenAI-shaped adapters, cost enforcement (streaming and Anthropic landed later; see TODO.md).

### M5. Profiles
Spec: PRD FR-1.2, FR-1.3; ARCH §3, §3.1; RES-004.
- `Profile::{Ask, Plan, Build}` each with a tool allowlist. Tools outside it are absent from the schema and rejected at execution if called anyway.
- `ask` and `plan`: read-only tools only (`read_file`, `glob`, `grep`). `plan` prompt requires Goal, Files, Steps, Risks, Proof. `ask` has no artifact and names the profile to switch to; it never switches itself.
- `build`: all tools; gate and approvals apply. REPL `/ask`, `/plan`, `/build`; default `build`.
- Acceptance: no mutating tool in the `ask`/`plan` request; forced `write_file` in `ask` returns an error observation and changes nothing; slash commands switch.
- Non-goals: `debug`, `security`.

### M6. JIT context tools
Spec: PRD NFR-3/4; CBU §4.1.
- `glob(pattern)`, `grep(pattern, path?, max_results)` (`ignore` + `regex`), `read_file(path, start_line?, end_line?)`, all through `guard::canonicalize`. Per-call cap 24,000 bytes with a truncation marker; binary files reported, not dumped.
- Acceptance: `.gitignore` respected; outside-root denied; caps enforced; slicing exact (1-based, inclusive); report warm grep latency on the 1,200-file fixture (target 150 ms, reported not asserted).
- Non-goals: PageRank, vector search, repo map.

### M7. Gate v0, verification summary, audit chain
Spec: PRD FR-2.1–2.3, Gate 2; ARCH §5; RES-010 (gate demand is a hypothesis; ship the summary first).
- Tool log: every `run_command` and file edit with order, command, exit code, timestamp.
- Classifier v0: **Class C** only when equal after stripping whitespace; everything else Class A; Class B empty; unknown is A.
- Build mode runs the configured test command at least once before the first `write_file`/`edit_file`; a failing baseline is recorded as red.
- Class A rule v0: the test command must pass after the last edit, and either the diff touches a test glob (default `tests/**`, `**/*_test.*`, `**/*.spec.*`, `**/test_*.*`) or that command failed, then an edit occurred, then it passed this session. Passing results bind to a SHA-256 workspace fingerprint checked at finish. A file returning to content seen twice earlier stops the run with no progress.
- Summary lists agent-edited test paths and net added/removed lines with a review notice; red-edit-green without agent test edits names the existing tests. Printed from the tool log, never from model text: each check is fresh, stale (edits after it), or not run.
- `--skip-tests`: runs remaining checks and appends one JSON entry to `.devcode/audit.log` (timestamp, actor, files, diff SHA, reason, `prev_entry_sha`). Chain verified at start and before finalizing; a broken chain blocks. Anchors are post-MVP (TODO A3).
- Acceptance: source edit with no test change blocked with a reason; source plus test edit with a passing command after the last edit allowed; edit after a passing run blocked (stale); test command not run blocked; whitespace-only is Class C; `--skip-tests` appends a valid entry and editing or deleting an earlier line blocks the next run; summary marks stale and not-run correctly.
- Non-goals: language-aware refactor detection, Class B, anchors, semgrep/gitleaks.

### M8. Approvals, diff UX, shell guard
Spec: PRD FR-3.2; ARCH §6.4 (all); RES-003, RES-005.
- Before every edit-tool write: unified diff (`similar`), prompt once / always (this session) / reject. `--yes` auto-approves edits only; no TTY and no `--yes` rejects edits.
- Destructive patterns (list in ARCH §6.4.5) always ask, even with `--yes`; denied with no TTY.
- Lexical guard before every `run_command`: strip heredoc bodies (several per line), deny `>`, `>>`, `&>`, `>|` and `tee`/`cp`/`mv` whose target resolves into `.devcode/` or a guard-set file, telling the agent to use file tools. Known gaps (for example `python -c "open(...).write"`) are backstopped by post-command reconcile (M3).
- Every mutation-capable `run_command`: snapshot (M3), run (M2), reconcile (M3), invalidate gates if tracked or guard-set source changed.
- Acceptance: table test of >= 25 command strings (allowed, denied, ask) including heredocs containing `>`, multiple heredocs, quoted `>`, redirects into `.devcode/`; a `python -c` edit to a tracked file is not blocked lexically but is detected by reconcile, invalidates the gate, and is rolled back by `devcode rollback`; approval flow with piped stdin (once, always, reject).
- Non-goals: policy files, per-command allowlists beyond the lists above.

### M9. Discipline v0 and token frugality v0
Spec: new; RES-011 (demand is real for token burn and false completion; overbuilding weakly evidenced).
- Scope contract: in `build`, after the baseline command and before the first edit the model calls `declare_scope(files[], reason)`. Edits to undeclared paths prompt with "outside declared scope".
- Dependency flag: a diff adding a dependency to `Cargo.toml`, `package.json`, `pyproject.toml`, `requirements*.txt`, or `go.mod` needs approval and a one-line justification in the tool log.
- End-of-task line: steps, tokens in/out, files edited, files outside scope, dependencies added.
- Compaction: above 75% of the configured context window, replace old tool outputs with a deterministic harness-built summary (goals, files modified, last test result; head and tail of long outputs). No extra model call. Pin active manifest, current diff, config (ARCH §7.1).
- Acceptance: out-of-scope prompts, in-scope does not; dependency add prompts and logs justification; compaction triggers at the threshold, keeps pinned items, preserves the last test result verbatim; end-of-task numbers match the scripted run.
- Non-goals: LLM-written summaries, peer tokens-per-task benchmark, "better than std" judgment (human decides at the prompt).

### M10–M11. Live-run fixes and test reporting
Landed after first live runs (RES-012) and the Gemini harder task (RES-013). Evidence and findings are in those notes; behavior is pinned by `tests/m10a_*` to `m10h_*` and `tests/m11_test_reporting.rs` (M11: agent-changed tests listed in the verification summary).

### M12. Stored credentials and `devcode auth`
Spec: new. Reference practice (code): Codex credentials file or OS keyring behind a backend switch (`codex-rs/login`); OpenCode `auth` command. Neither copied.
Goal: use a provider without an exported env var or `.env` file.
- `devcode auth login [NAME]`: NAME defaults to the config preset. On a TTY, hidden prompt. Without a TTY `--with-stdin` reads one line; otherwise fail. The key is never accepted as an argument (shell history, `ps`).
- `auth list`: names and "stored", nothing from the key. `auth logout NAME`: idempotent.
- Storage: `credentials.json` in `$DEVCODE_CONFIG_DIR`, else `$XDG_CONFIG_HOME/devcode`, else `~/.config/devcode`. Directory 0700, file 0600, atomic write (temp file in the same directory, then rename). A group- or world-readable file is refused with a fix message. Never inside a workspace.
- Config: provider block gets optional `"connector": "<name>"`; `--provider nim` sets `connector = "nim"`; `api_key_env` stays optional.
- **Key resolution order**: (0) an explicit `auth switch` or `/connect NAME:alias` for this connector wins over env (P10e); (1) env var named by `api_key_env`, when set and non-empty; (2) stored credential named by `connector`; (3) fail naming both: "run `devcode auth login <name>` or set <ENV>". An endpoint with empty `api_key_env` and no `connector` needs no key.
- The key never appears in output, logs, traces, summaries, error text, manifests, or the audit log.
- Agent exposure: the lexical guard denies the credentials path pattern (`devcode/credentials`, `DEVCODE_CONFIG_DIR`). This is an advisory rail, not isolation; real isolation is the post-MVP sandbox.
- TTY no-echo needs termios: `unsafe` lives in `src/guard/tty.rs` with a soundness comment; no new crate.
- Acceptance (tests set `DEVCODE_CONFIG_DIR` to a temp dir, never the real home): stdin login stores the key at 0600/0700; `list` shows no key characters; second `logout` succeeds; group-readable file refused; env beats stored, unset env uses stored, neither fails with the two-way message; the key appears nowhere across login, list, a scripted run, and a 401 whose body echoes the key; `cat` of the credentials path denied by the guard; key as an argument rejected.
- Superseded in part (still partial vs peers; TODO.md P10): OAuth is now Gemini only, keyring is opt-in (`DEVCODE_CREDENTIALS_STORE=auto`), named slots `nim:alias` plus `auth switch` exist. Still non-goals: Windows ACLs, encryption at rest.

### M13. Safer login input and `/connect`
Spec: new. Problem found live: a key pasted with the command was echoed before the hidden prompt started.
- `auth login` on a TTY discards typed-ahead input (`tcflush(TCIFLUSH)`, `src/guard/tty.rs`) before printing "Paste the key now. Input is hidden." `--help` warns not to paste the key with the command.
- REPL `/connect [NAME]` runs the same hidden prompt (TTY required), stores the key, rebuilds the provider from current config, and continues; a missing `provider.model` is named, not deferred. `/connections` lists stored names (same as `auth list`).
- Acceptance: typed-ahead bytes discarded (PTY test via `openpty` with SAFETY comments, or state plainly that a PTY test was impractical); `/connect` with non-TTY stdin refuses clearly, with a TTY seam stores the key and the next scripted request carries it; `/connections` shows no key characters; the key appears in no output, trace, or error.
- Non-goals: TUI modal, OAuth, keyring.

## 7. What the verifier checks

1. Run the three §4 commands on macOS and read the Linux CI result.
2. Re-run each task's tests; read them for weak spots: assertions that cannot fail, mocks of the guard, rollback never exercised in a separate process.
3. Manually: temp repo with staged, dirty, and untracked files; scripted `build` session; roll back; compare hashes.
4. Grep for `unwrap(`, `unsafe`, `println!` of secrets, and dependencies absent from §3.
5. Confirm the report lists measured numbers with commands and a non-empty "Not checked".

## 8. Open items

- NIM model choice: tool-calling support and context window are per model; record the chosen `context_window` (M4, M9).
- Whether the gate's test-file heuristic (M7) is acceptable.
- Cost limits per provider (not enforced).
- Non-OpenAI-shaped adapters: Anthropic native and Gemini Code Assist exist; others post-MVP.
