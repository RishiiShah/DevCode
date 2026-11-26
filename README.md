# DevCode

**DevCode** is an autonomous, developer-centric agentic software engineering harness (in the class of Claude Code, Google Antigravity/AGY, and OpenCode) engineered with a unified execution loop, dynamic policy profiles, and deterministic verification gates to enforce disciplined software development practices.

## Goal

DevCode ships no model. You connect your own (Anthropic, OpenAI-compatible endpoints, Gemini, NVIDIA NIM). The harness enforces development practice in code, not in the prompt: tests before finalizing, declared edit scope, approval of new dependencies, and rollback of every file change. The aim is fewer tokens per completed task than other harnesses, with every token reported.

| Pillar | Built | Planned (PRD FR-7) |
| :--- | :--- | :--- |
| Practice | Test gate, scope contract, dependency approval, rollback, process kill | `debug` and `security` profiles |
| Fewer tokens | Compaction at 75% of context, tool-output truncation | Prompt caching, enforced NFR-3/NFR-4 budgets, optional task cap |
| Transparency | Input/output tokens and context left in the status line, end-of-task line | Per-call ledger with cache-read, cache-write and reasoning tokens, per-category breakdown, cost |
| Any model | Per-model context windows, provider and model switching | Per-model capability record; runtime downgrade (for example to text tool calls) when a model fails |

Token savings against other harnesses are not yet measured. The comparison protocol is [`docs/research/harness-bakeoff.md`](docs/research/harness-bakeoff.md).

---

## Quickstart

On macOS or Linux, install from this checkout with `cargo install --path .`.

1. Open your project directory and run `devcode`.
2. Run `/connect` and paste an API key into the masked prompt. New installs default to NVIDIA NIM. To use another provider, run `/provider`, choose one, then run `/connect`. Existing keys from supported environment variables or saved accounts work automatically.
3. Type a prompt and press Enter. Try `/ask` followed by `Where is the retry logic?` for a read-only first run.

Use `/help` for shortcuts, `/model` to pick a model, and Shift+Tab to cycle profiles. Use `/init` to scaffold optional project settings. Build runs without a configured test command finish marked UNVERIFIED.

## Technical Documentation Suite (ATE-1000 Series)

Specifications live under [`docs/`](docs/) in the **ATE-1000 series**. Each fact has one owning document and the others cite it by section. Agents: start at [`AGENTS.md`](AGENTS.md), which maps tasks to the sections worth reading.

| Document | Identifier | Revision | Owns |
| :--- | :--- | :--- | :--- |
| [`docs/prd.md`](docs/prd.md) | `ATE-1001-PRD` | 0.4.4-draft | Requirements FR-1..7, NFRs, acceptance gates |
| [`docs/devcode.md`](docs/devcode.md) | `ATE-1000-ARCH` | 0.4.3-draft | Design mechanisms |
| [`docs/codebaseunderstanding.md`](docs/codebaseunderstanding.md) | `ATE-1002-CBU` | 0.3.2-draft | Retrieval and security tooling |
| [`docs/mvp.md`](docs/mvp.md) | `ATE-1005-MVP` | 0.2.0 | Per-task deliverables and acceptance |
| [`TODO.md`](TODO.md) | `ATE-1004-TODO` | 0.3.2 | Status and open work |

## Dynamic Policy Profiles

| Profile | Purpose | Tool Permissions | Enforced Gate |
| :--- | :--- | :--- | :--- |
| **`ask`** | Questions about the codebase: explain, locate, compare | Strictly Read-Only | None (answers only; no edits, no artifact) |
| **`plan`** | Architectural discovery, task breakdown, dependency mapping | Strictly Read-Only | Structured Technical Specification |
| **`build`** | Test-driven implementation, surgical code edits, verification | Scoped Workspace Read/Write | Class-Based Test Gate + Linters + Compilers |
| **`debug`** (not built) | Scientific root-cause isolation and regression-free resolution | Scoped Workspace Read/Write | Minimal Reproducer Test (Red $\rightarrow$ Green) |
| **`security`** (not built) | Vulnerability auditing, secret detection, dependency CVE scan | Read-Only (Local DBs) | Gitleaks (Secrets) + Semgrep (SAST) |

## CLI and diagnostics

Pass a prompt directly, for example `devcode --profile ask "where is the retry logic?"`. Use `devcode --plain` for the line-based REPL or `devcode rollback` to undo the latest run. Each conversation is saved per folder: `devcode -c` continues the latest, `devcode --resume` (or `/resume` in the TUI) picks one.

For a custom OpenAI-compatible endpoint, set `base_url`, `api_key_env`, and a tool-capable `model` under `provider` in `.devcode.json`.

`--trace` writes per-step timing and token counts (no file contents) to `trace.jsonl` in the project's state dir (`~/.local/share/devcode/projects/<folder>-<hash>/`, ARCH §4.3). Set `DEVCODE_STREAM=0` to disable TUI streaming. Live results and limits: [`docs/research/first-live-runs.md`](docs/research/first-live-runs.md).

Shell commands stop after 120 seconds. Long-running servers use the `start_process` / `stop_process` tools, which snapshot first, run in their own process group, and reconcile file changes on stop. If a command cannot be reconciled, its error names a retained `recovery/<session>/<step>.json` snapshot in the state dir. Run `devcode recover <session> <step>` to extract original files into `recovered/<session>/<step>` there without overwriting your working tree. Compare before restoring; the snapshot does not distinguish command changes from concurrent user changes or capture newly created paths.

## Accounts and keys

```sh
devcode auth login nim            # hidden prompt; or --with-stdin
devcode auth login nim:work       # named slot (one colon, alias [A-Za-z0-9_-]+)
devcode auth switch nim:work      # select the stored key; beats the env var for that connector
devcode auth login gemini --oauth # Google browser/loopback OAuth (or NO_BROWSER=1 paste-code) -> oauth-gemini.json (Code Assist)
devcode auth list | logout NAME
```

Subscription logins: Anthropic and Google forbid third-party tools from using consumer-plan sign-ins, and Google users report suspensions for Gemini CLI OAuth in third-party tools (vendor claim; [`docs/research/capability-and-token-peers.md`](docs/research/capability-and-token-peers.md)). Prefer API keys. `auth login gemini --oauth` is at your own risk.

Key order: an explicit `auth switch` / `/connect NAME:alias` wins, then the env var named by `api_key_env`, then the stored default key. Keys live in `credentials.json` (0600, outside the workspace). `DEVCODE_CREDENTIALS_STORE=auto` tries the OS keyring first and falls back to the file; the default is the file only.

## Install

Works today (needs a Rust toolchain; no release has been published yet):

```sh
cargo install --git https://github.com/RishiiShah/DevCode
```

Prepared but not yet published, see [`packaging/README.md`](packaging/README.md): `install.sh` for release binaries, a Homebrew formula, and `pip`/`pipx` wheels built with maturin.

## Platforms & Install (planned)

Tier 1: macOS, Linux (glibc/musl), Windows 11; one self-contained Rust binary per target. The one-line installers (`install.sh`, `install.ps1`) are prepared but not published, and `<install-host>` is still a placeholder because the product name is undecided. Hosting, signing, and installer requirements: `ATE-1000-ARCH` §6.3 and `ATE-1001-PRD` FR-6.

## Delivery Priorities

1. **Initial Release**: Unified loop, JIT search, persisted pre-step rollback with conflict detection, and trusted behavior-based gates covering configuration changes.
2. **Release Evidence**: Pass rollback and gate-integrity acceptance tests in `ATE-1001-PRD` before extending retrieval.
3. **Deferred Extension**: Enable Tier 2 PageRank only after comparative retrieval results satisfy `ATE-1002-CBU` §4.4.
