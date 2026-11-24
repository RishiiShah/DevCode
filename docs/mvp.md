# MVP Implementation Plan

```
================================================================================
DOCUMENT ID    : DEVCODE-MVP
REVISION       : 0.1.0
STATUS         : Working plan (implementers: agents; verifier: maintainer)
SPEC           : docs/prd.md (FR/NFR/Gates), docs/devcode.md (architecture)
NOTES          : docs/research/*.md are informative; the specs are normative
================================================================================
```

## 1. Goal

A CLI an engineer can run on a real repository that shows the thesis: a **minimal, verified, reversible** change at **low token cost**. macOS and Linux only. Windows, the `debug` and `security` profiles, installer, and naming come after the MVP (TODO.md "After MVP").

## 2. Rules for implementers

1. One task at a time, in order (M1 to M9). Do not start the next until the current one passes section 4.
2. Smallest complete change. No speculative options or abstractions. No code for post-MVP items.
3. Standard library first. Add a dependency only when it clearly beats writing it (see section 3). Every dependency is justified in the task report; unlisted dependencies need justification and approval.
4. Tests never call a paid or networked LLM and never touch real credentials. Use `ScriptedModel` (M4). A manual smoke test against a real provider needs maintainer approval first.
5. Never read real `.env*` files. Never log API keys.
6. Do not edit `docs/prd.md` or `docs/devcode.md`. If the spec is wrong or silent, stop and report under "Spec questions".
7. No commits unless asked. Keep generated or large files out of the repo.
8. Done means every acceptance item in the task passes and the report (section 5) is written.

## 3. Layout and dependencies

One Cargo package `devcode` with a library and a binary; modules, not crates, until a split is justified.

```
Cargo.toml
src/main.rs            # thin: parse args, call lib
src/lib.rs
src/cli.rs             # args, profile selection, REPL loop
src/config.rs          # .devcode.json
src/guard/             # M2: paths, env strip, bounded process
src/manifest/          # M3: capture, blobs, rollback, guard-set snapshot
src/agent/             # M4: Model trait, loop, limits, ScriptedModel
src/tools/             # M6/M5: tool registry, file tools, glob/grep/read, run_command
src/profile.rs         # M5
src/gate/              # M7: classifier, verification log, audit chain
src/approval/          # M8: diff prompt, destructive patterns, shell lexical guard
tests/                 # integration tests, one file per task
.github/workflows/ci.yml
```

Recommended dependencies (starting list; edition 2021, stable Rust):

| Need | Use | Why not std |
| :--- | :--- | :--- |
| CLI args | `clap` (derive) | Subcommands, help, exit codes |
| JSON | `serde`, `serde_json` | No JSON in std |
| Hashing | `sha2` | No SHA-256 in std |
| Ignore rules and walking | `ignore` | Correct `.gitignore` semantics |
| Regex | `regex` | No regex in std |
| Unified diff | `similar` | Diff algorithm |
| Process group kill | `libc` (`killpg`) | Std has `process_group` but no group kill |
| HTTP (M4 adapter only) | `ureq` (blocking) | No HTTP client in std; blocking avoids an async runtime |
| Tests | `tempfile` (dev) | Temp dirs |

Not allowed without approval: async runtimes, `anyhow`/`thiserror` (use a plain error enum), logging frameworks (use `eprintln!`), tokenizers (use provider-reported usage).

## 4. Common acceptance (every task)

- `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`, `cargo test` pass on macOS and Linux (CI matrix from M1).
- New code has tests for each acceptance bullet; tests are deterministic and run in temp directories.
- No `unsafe` except calls into `libc` in `guard/`, each with a comment stating why it is sound.
- Public functions return `Result`; no `unwrap`/`expect` outside tests.

## 5. Task report (paste into the PR or TODO.md under the task)

```
Task: Mx
Changed: <files>
Dependencies added: <crate: reason>   (or "none")
Commands run: <fmt/clippy/test output summary>
Acceptance: <each bullet: pass/fail + test name>
Measured: <any numbers, with the command that produced them>
Not done / limits: <explicit>
Spec questions: <explicit>
Not checked: <explicit>
```

## 6. Tasks

### M0. Decisions (maintainer)
- **Decided:** the first provider is **NVIDIA NIM**, reached through one OpenAI-compatible chat-completions adapter (`kind = "openai_compat"`). The same adapter serves any endpoint with that API shape (NIM hosted or self-hosted, OpenAI, OpenRouter, vLLM, Ollama) by configuration only; no code change per provider. Native adapters for APIs with a different shape (for example Anthropic Messages, Gemini) are post-MVP and plug into the same `Model` trait.
- Still open: confirm MVP scope in TODO.md; pick the NIM model (see M4).

### M1. Scaffold
Spec: arch §7.4 (config), prd FR-1.3, FR-6.1.
Deliver:
- Package `devcode`, lib plus bin, CI workflow (macOS and Linux matrix: fmt, clippy, test).
- CLI: `devcode [--profile ask|plan|build] [PROMPT]`, `devcode rollback [--step N]`, `devcode --version`, `devcode --help`. Subcommand bodies are stubs returning "not implemented" with exit code 1 until their task lands.
- `.devcode.json` loader: `verification.test_command`, `verification.test_globs`, `sandbox.timeout_seconds`; unknown fields are an error; missing file gives defaults (`timeout_seconds = 30`).
Acceptance:
- `devcode --version` prints the package version, exit 0; unknown flag exits 2.
- Config tests: valid file, missing file (defaults), unknown field (error naming the field), wrong type (error).
Non-goals: REPL, tools, model calls.

### M2. Runtime guard (POSIX)
Spec: prd FR-4.1, FR-4.2, FR-4.3, NFR-8; arch §6, §6.4.
Deliver (`src/guard/`):
- `canonicalize(root, path) -> Result<PathBuf, GuardError>`: resolves symlinks and `..`; for a path that does not exist, canonicalizes the nearest existing parent then appends the checked remainder; error if the result is outside the canonical root.
- `.devcode/` deny: any canonical path inside `<root>/.devcode` is denied for agent writes (compare case-insensitively on every OS).
- `strip_env(vars) -> Vec<(String,String)>`: removes `AWS_*`, `GITHUB_TOKEN`, `*_API_KEY`, `*_TOKEN`, `SSH_*` (case-sensitive on POSIX).
- `run_bounded(argv, cwd, timeout, out_cap) -> RunResult`: spawns with `CommandExt::process_group(0)`, stripped env, captures stdout and stderr merged; on timeout sends SIGTERM to the group, then SIGKILL after a grace period; output beyond `out_cap` bytes is truncated keeping head and tail with a marker. Default cap 24,000 bytes (about NFR-4's 6,000 tokens) [guess; configurable].
Acceptance:
- Symlink pointing outside root: denied. `../` escape: denied. Write path under `.DevCode/x`: denied. New file in an existing subdirectory: allowed.
- `strip_env` removes each pattern and keeps `PATH`, `HOME`.
- Timeout test: `sh -c 'sleep 1000 & sleep 1000 & wait'` with 1 s timeout; after return no process with the test's unique marker argument remains (check with `ps`). Report the measured time from timeout to all-dead (NFR-8 target 100 ms; report, do not fail the build on it).
- Output cap: 1 MB output returns at most the cap, with head, tail, and a marker.
Non-goals: Windows Job Object, network isolation, command policy.

### M3. Step-manifest engine
Spec: prd FR-3.1 to FR-3.5, NFR-6, NFR-7, NFR-9, Gates 1, 4, 8; arch §4, §6.4.5; RES-008.
Deliver (`src/manifest/`):
- Storage: `.devcode/manifests/<session_id>/<step_id>.json`, blobs content-addressed under `.devcode/blobs/<sha256>`. Write blobs and the manifest, flush, and fsync the file and its directory before the mutation proceeds. If capture fails, the mutation is refused.
- Record per path: existence, type (file, symlink, dir), symlink target, mode bits, SHA-256, blob reference. Post-step state recorded after the mutation.
- `capture(path)`, `record_post(step)`, `rollback(session, step)`: reverse step order; **preflight** every touched path against recorded post-state and abort without changes on any mismatch or incomplete record; restore bytes, type, and mode; remove an agent-created path only if still unchanged; never read `HEAD` or the index and never run Git commands that mutate the index.
- Guard-set snapshot (arch §6.4.5): enumerate non-ignored files (use `ignore`; without Git use the built-in derived-directory list: `target`, `node_modules`, `dist`, `build`, `__pycache__`, `.cache`), excluding `.devcode/`; per-file cap 10 MB; fingerprint (type, size, mtime ns, mode); hash and store only changed paths; budget 5,000 files and blob bytes under 50 MB (NFR-7); over budget returns `BudgetExceeded`. `reconcile(before) -> {created, modified, deleted}` after a command.
- Retention helper for `devcode gc` is out of scope.
Acceptance (tests in temp directories, using the `git` binary where Git state matters):
- Gate 1: partially staged file, dirty unstaged file, untracked file, unrelated dirty file. Modify, delete, create, roll back. Afterwards bytes, type, and mode match pre-step; staged index bytes (`.git/index` hash) are unchanged; `git status --porcelain` matches pre-step. Repeat in a fresh process (persistence) and in a repository with no commit.
- Gate 4: external edit to a modified path before rollback gives a conflict error, no path changed, backups retained. Inject capture failure (read-only blob directory): the mutation is refused. Inject a restore failure: partial recovery is reported, backups retained.
- Gate 8 (manifest side): run `sed -i`-style in-place edit, a `>` redirect, a delete, and a create through `std::process` after a snapshot; `reconcile` lists exactly those paths; rollback restores them; a gitignored file is reported as unprotected and is not restored. Repeat in a non-Git directory. Over-budget fixture returns `BudgetExceeded`.
- Report the measured snapshot time on a generated fixture of 1,200 files (first run and incremental). NFR-9 targets (250 ms, 2 s) are reported, not asserted.
Non-goals: audit log (M7), `gc`, Windows.

### M4. Agent loop and provider trait
Spec: prd FR-1.1; arch §7.1; RES-001.
Deliver (`src/agent/`):
- `trait Model { fn complete(&self, req: &Request) -> Result<Response, ModelError>; }`. `Request` holds system prompt, messages, and tool schemas. `Response` holds text, tool calls, and `usage { input_tokens, output_tokens }`.
- `ScriptedModel`: replays a JSON script of responses for tests.
- Loop: query, parse tool calls, execute through the tool registry, append observations, repeat until the model returns no tool call or a limit trips. Limits (configurable, defaults are guesses): 50 steps, wall time 30 minutes, cost not enforced until pricing is known, 3 consecutive format errors. Each limit ends the run with a distinct exit reason.
- Token accounting: sum provider-reported usage; print one line at the end (steps, input and output tokens).
- **Extend the config loader** (M1 shipped without it) with the `provider` block:
  - Provider config (the same block may also be set in a user-level config file; project values win):
    ```json
    { "provider": { "kind": "openai_compat",
                    "base_url": "https://integrate.api.nvidia.com/v1",
                    "api_key_env": "NVIDIA_API_KEY",
                    "model": "<model id from the provider's catalog>",
                    "context_window": 128000 } }
    ```
    `kind` accepts only `openai_compat` in the MVP; any other value is an error that lists supported kinds. `--provider nim` is a preset filling `base_url` and `api_key_env`. `model` and `context_window` have no default (catalogs and limits change; tool-calling support is per model). `api_key_env` names an environment variable; the key is never stored in config. `api_key_env` may be empty for a local endpoint that needs no key.
  - Tests: valid block; unsupported `kind` error lists `openai_compat`; `--provider nim` fills `base_url` and `api_key_env`; a key value placed in config is rejected (only an env var name is accepted); unknown field inside `provider` is an error.
- **OpenAI-compatible adapter** (`src/agent/openai_compat.rs`, `ureq`):
  - Request: `POST {base_url}/chat/completions` with `Authorization: Bearer <key>` (omitted when `api_key_env` is empty), JSON body `{ model, messages, tools, tool_choice: "auto", stream: false }`; tools use the `{"type":"function","function":{name,description,parameters}}` shape.
  - Response: `choices[0].message.content` and `tool_calls[].function.{name, arguments}` (`arguments` is a JSON **string**; parse it); `usage.prompt_tokens` and `usage.completion_tokens` map to input and output tokens; read `finish_reason`.
  - Failures: 429 and 5xx retry with exponential backoff (honor `Retry-After`; at most 5 tries for 429, 2 for 5xx); other 4xx fail at once with the body's error message; connect timeout 10 s, read timeout 120 s. Invalid `arguments` JSON, an unknown tool name, or a response with neither text nor tool calls counts as a format error (3 in a row ends the run). A model that never returns tool calls produces a clear message that it may lack tool-calling support.
  - Key handling: read the env var named by `api_key_env` at startup; never read `.env` files; never log or print the key; redact it from error text. M2's `strip_env` keeps it out of child processes.
  - Rate limits: hosted NIM trial accounts are reported at about 40 requests per minute and about 1,000 credits (NVIDIA developer-forum posts; unconfirmed in official docs). An optional `provider.max_rpm` spaces requests; 429 handling above is the backstop.
  - No automated test hits the network. Tests run the adapter against a fake server built on `std::net::TcpListener` (no new dependency).
- A live smoke test against NIM needs maintainer approval first (it spends trial credits); run it once after M4 passes.
- Other providers are config-only. Examples, **untested; confirm each against the provider's docs**:

  | Provider | `base_url` | `api_key_env` |
  | :--- | :--- | :--- |
  | NVIDIA NIM hosted (docs list this server) | `https://integrate.api.nvidia.com/v1` | `NVIDIA_API_KEY` |
  | NVIDIA NIM self-hosted | `http://localhost:8000/v1` | empty |
  | OpenAI | `https://api.openai.com/v1` | `OPENAI_API_KEY` |
  | OpenRouter | `https://openrouter.ai/api/v1` | `OPENROUTER_API_KEY` |
  | Ollama | `http://localhost:11434/v1` | empty |
Acceptance (all with `ScriptedModel`):
- Adapter tests against the fake server: request body has the expected fields and tool schema; the `Authorization` header carries the key and the key appears in no log or error text; 200 with tool calls parses into `Response`; `arguments` that is not valid JSON becomes a format error; 429 with `Retry-After` retries then succeeds; 429 exceeding the retry cap fails with a clear error; 401 fails immediately; a connect timeout fails cleanly. `provider.kind` other than `openai_compat` is rejected at startup.
- Normal run ends on a no-tool response. Step limit, wall-time limit, and 3 format errors each end the run with the matching reason. Tool errors become observations, not crashes. Token totals equal the scripted usage.
Non-goals: streaming, native adapters for non-OpenAI-shaped APIs, cost enforcement.

### M5. Profiles
Spec: prd FR-1.2, FR-1.3; arch §3, §3.1; RES-004.
Deliver (`src/profile.rs`, `src/cli.rs`):
- `Profile::{Ask, Plan, Build}`. Each profile has a tool allowlist. Tools outside it are not sent in the schema and are rejected at execution if called anyway (defense in depth).
- `ask` and `plan`: read-only tools only (`read_file`, `glob`, `grep`); no write or `run_command`. `plan` prompt requires a structured result (Goal, Files, Steps, Risks, Proof). `ask` has no required artifact and names the profile to switch to if a change is requested; it never switches itself.
- `build`: all tools; gate and approvals apply (M7, M8).
- REPL: `/ask`, `/plan`, `/build` switch profiles; default `build`.
Acceptance:
- Tool list for `ask` and `plan` contains no mutating tool (assert on the request the scripted model receives). A forced call to `write_file` in `ask` returns an error observation and changes nothing on disk. Slash commands switch profiles.
Non-goals: `debug`, `security`.

### M6. JIT context tools
Spec: prd NFR-3, NFR-4; CBU §4.1.
Deliver (`src/tools/`):
- `glob(pattern)`, `grep(pattern, path?, max_results)` (`ignore` plus `regex`), `read_file(path, start_line?, end_line?)`. All paths go through `guard::canonicalize`.
- Per-call output cap 24,000 bytes (about 6,000 tokens [guess]) with a truncation marker; binary files are reported, not dumped.
Acceptance:
- `.gitignore` is respected; a path outside the root is denied; caps are enforced; line slicing is exact (1-based, inclusive).
- Report grep latency on the 1,200-file fixture, warm. The spec target is 150 ms (CBU §4.1); report, do not assert.
Non-goals: PageRank, vector search, repo map.

### M7. Gate v0, verification summary, audit chain
Spec: prd FR-2.1 to FR-2.3, Gate 2; arch §5; RES-010 (gate demand is a hypothesis; ship the summary first).
Deliver (`src/gate/`):
- Tool log: every `run_command` and file edit recorded with order, command, exit code, and timestamp.
- Classifier v0: **Class C** only when the change is whitespace-only (equal after stripping whitespace). Everything else is **Class A**. Class B is empty (prd FR-2.1). Unknown means Class A.
- In build mode, run the configured test command at least once before `write_file` or `edit_file`; a failing baseline is recorded as red.
- Class A rule v0: the configured test command must pass after the last edit, and either the diff touches a test glob (default `tests/**`, `**/*_test.*`, `**/*.spec.*`, `**/test_*.*`) or that command failed, then an edit occurred, then the command passed in this session. Passing results bind to a SHA-256 workspace fingerprint checked at finish. If a file returns to content seen twice earlier in the run, stop with no progress.
- Verification summary lists agent-edited test paths and net added/removed lines, with a review notice; when red-edit-green passes without agent test edits, identify the existing tests. Summary is printed from the tool log, never from model text: each check as fresh, stale (edits after it), or not run.
- `--skip-tests`: runs remaining checks, appends one JSON entry to `.devcode/audit.log` with timestamp, actor, files, diff SHA, reason, and `prev_entry_sha`. At start and before finalizing, verify the chain; a broken chain blocks finalization. (Truncation anchors are post-MVP, TODO A3.)
Acceptance (Gate 2 subset, scripted model and a temp repo with a trivial test command):
- Source edit with no test change: blocked with a reason. Source plus test edit and passing command after the last edit: allowed. Edit after the passing run: blocked (stale). Test command not run: blocked ("not run").
- Whitespace-only change is Class C and allowed with no test.
- `--skip-tests` appends a valid chained entry; editing or deleting an earlier line makes the next run block.
- Summary output marks stale and not-run correctly.
Non-goals: language-aware refactor detection, Class B, anchors, semgrep/gitleaks.

### M8. Approvals, diff UX, shell guard
Spec: prd FR-3.2; arch §6.4 (all sections); RES-003, RES-005.
Deliver (`src/approval/`):
- Before every edit-tool write: show a unified diff (`similar`), prompt once / always (this session) / reject. `--yes` auto-approves edits only. With no TTY and no `--yes`, edits are rejected.
- Destructive patterns (`rm`, `mv`, `sed -i`, `git clean`, `git checkout --`, `git restore`, `git reset --hard`, `git stash`) always ask, even with `--yes`; with no TTY they are denied.
- Shell lexical guard before every `run_command`: strip heredoc bodies (handle several per line), deny redirections (`>`, `>>`, `&>`, `>|`) and `tee`/`cp`/`mv` whose target resolves into `.devcode/` or a guard-set file, with a message telling the agent to use file tools. Document known gaps (for example `python -c "open(...).write"`): the post-command reconcile (M3) is the backstop.
- Every mutation-capable `run_command` is wrapped: snapshot (M3), run (M2), reconcile (M3), gates invalidated if tracked or guard-set source changed.
Acceptance:
- Table test of at least 25 command strings (allowed, denied, ask) including heredocs containing `>`, multiple heredocs, quoted `>`, and redirects into `.devcode/`.
- Scripted run: a `run_command` that edits a tracked file via `python -c` is not blocked lexically but is detected by reconcile, invalidates the gate, and is rolled back by `devcode rollback`.
- Approval flow with piped stdin: once, always, reject.
Non-goals: policy files, per-command allowlists beyond the lists above.

### M9. Discipline v0 and token frugality v0
Spec: new (TODO P2, P3); RES-011 (demand is real for token burn and false completion; overbuilding is weakly evidenced).
Deliver:
- Scope contract: in `build`, after the configured baseline command (if any) and before the first edit the model calls `declare_scope(files[], reason)`. Edits to undeclared paths prompt for approval with the text "outside declared scope".
- Dependency flag: a diff that adds a dependency to `Cargo.toml`, `package.json`, `pyproject.toml`, `requirements*.txt`, or `go.mod` requires approval and a one-line justification recorded in the tool log.
- End-of-task line: steps, tokens in and out, files edited, files outside scope, dependencies added.
- Compaction: when scripted usage shows more than 75% of the configured context window, replace old tool outputs with a deterministic harness-built summary (goals, files modified, last test result; head and tail of long outputs). No extra model call. Pin the active manifest, current diff, and config (arch §7.1).
Acceptance:
- Out-of-scope edit prompts; in-scope edit does not. Dependency add prompts and logs the justification. Compaction triggers at the threshold, keeps the pinned items, and preserves the last test result verbatim. End-of-task line numbers match the scripted run.
Non-goals: LLM-written summaries, tokens-per-task benchmark against a peer (post-MVP), dependency "better than std" judgment (human decides at the prompt).

## 7. What the verifier checks

1. Run the three commands in section 4 on macOS (and read the Linux CI result).
2. Re-run each task's tests; read the tests for the weak spots: assertions that cannot fail, mocks of the guard, tests that never exercise rollback in a separate process.
3. Manually: create a temp repo with staged, dirty, and untracked files; run a scripted `build` session; roll back; compare hashes.
4. Grep for `unwrap(`, `unsafe`, `println!` of secrets, and dependencies absent from section 3.
5. Confirm the report lists measured numbers with commands, and a non-empty "Not checked".

## 8. Open items (not blocking M1 to M3)

- NIM model choice: tool-calling support and context window are per model; pick one from the NIM catalog and record its `context_window` (M4 and M9 need it).
- Native (non-OpenAI-shaped) provider adapters, post-MVP.
- Whether the gate's test-file heuristic (M7) is acceptable for the MVP.
- Cost limits per provider (not enforced in the MVP).
