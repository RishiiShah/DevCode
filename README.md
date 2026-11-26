# DevCode

**DevCode** is an autonomous, developer-centric agentic software engineering harness (in the class of Claude Code, Google Antigravity/AGY, and OpenCode) engineered with a unified execution loop, dynamic policy profiles, and deterministic verification gates to enforce disciplined software development practices.

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
| :---

## Dynamic Policy Profiles

| Profile | Purpose | Tool Permissions | Enforced Gate |
| :--- | :--- | :--- | :--- |
| **`ask`** | Questions about the codebase: explain, locate, compare | Strictly Read-Only | None (answers only; no edits, no artifact) |
| **`plan`** | Architectural discovery, task breakdown, dependency mapping | Strictly Read-Only | Structured Technical Specification |
| **`build`** | Test-driven implementation, surgical code edits, verification | Scoped Workspace Read/Write | Class-Based Test Gate + Linters + Compilers |
| **`debug`** (not built) | Scientific root-cause isolation and regression-free resolution | Scoped Workspace Read/Write | Minimal Reproducer Test (Red $\rightarrow$ Green) |
| **`security`** (not built) | Vulnerability auditing, secret detection, dependency CVE scan | Read-Only (Local DBs) | Gitleaks (Secrets) + Semgrep (SAST) |

## CLI and diagnostics

Pass a prompt directly, for example `devcode --profile ask "where is the retry logic?"`. Use `devcode --plain` for the line-based REPL or `devcode rollback` to undo the latest run.

For a custom OpenAI-compatible endpoint, set `base_url`, `api_key_env`, and a tool-capable `model` under `provider` in `.devcode.json`.

`--trace` writes per-step timing and token counts (no file contents) to `.devcode/trace.jsonl`. Set `DEVCODE_STREAM=0` to disable TUI streaming. Live results and limits: [`docs/research/first-live-runs.md`](docs/research/first-live-runs.md).

Shell commands stop after 120 seconds. Long-running servers use the `start_process` / `stop_process` tools, which snapshot first, run in their own process group, and reconcile file changes on stop. If a command cannot be reconciled, its error names a retained `.devcode/recovery/<session>/<step>.json` snapshot. Run `devcode recover <session> <step>` to extract original files into `.devcode/recovered/<session>/<step>` without overwriting your working tree. Compare before restoring; the snapshot does not distinguish command changes from concurrent user changes or capture newly created paths.

## Accounts and keys

```sh
devcode auth login nim            # hidden prompt; or --with-stdin
devcode auth login nim:work       # named slot (one colon, alias [A-Za-z0-9_-]+)
devcode auth switch nim:work      # select the stored key; beats the env var for that connector
devcode auth login gemini --oauth # Google browser/loopback OAuth (or NO_BROWSER=1 paste-code) -> oauth-gemini.json (Code Assist)
devcode auth list | logout NAME
```

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
