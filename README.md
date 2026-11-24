# DevCode

**DevCode** is an autonomous, developer-centric agentic software engineering harness (in the class of Claude Code, Google Antigravity/AGY, and OpenCode) engineered with a unified execution loop, dynamic policy profiles, and deterministic verification gates to enforce disciplined software development practices.

---

## Technical Documentation Suite (ATE-1000 Series)

All engineering specifications are maintained under [`docs/`](docs/) following the **ATE-1000 Technical Documentation Series**:

| Document | Identifier | Revision | Scope |
| :--- | :--- | :--- | :--- |
| **[System Architecture](docs/devcode.md)** | `ATE-1000-ARCH` | `0.4.0-draft` | Unified agent loop, dynamic policy profiles, pragmatic sandbox runtime, persisted pre-step rollback, and trusted behavior-based verification gates. |
| **[Product Requirements](docs/prd.md)** | `ATE-1001-PRD` | `0.4.0-draft` | Product vision, functional requirements (FR-1 to FR-6), testable NFR benchmark recipes, and deterministic quality gates. |
| **[Codebase Understanding](docs/codebaseunderstanding.md)** | `ATE-1002-CBU` | `0.3.1-draft` | Comparative analysis of indexing archetypes (JIT vs. PageRank vs. Vector), deferred, evidence-gated Tier 2 PageRank engine, and security tool orchestration. |
| **[Reference Catalog](references/README.md)** | `ATE-1003-REF` | `0.2.0-draft` | Reference source inventory, upstream license compliance, clone acquisition tracking, and peer study scope decisions. |

Empirical research notes supporting these normative standards are maintained under [`docs/research/`](docs/research/) under the **`DEVCODE-RES-00x`** series (informative; the `ATE-100x` documents above are normative, and any requirement change must land in them).

---

## Dynamic Policy Profiles

| Profile | Purpose | Tool Permissions | Enforced Gate |
| :--- | :--- | :--- | :--- |
| **`ask`** | Questions about the codebase: explain, locate, compare | Strictly Read-Only | None (answers only; no edits, no artifact) |
| **`plan`** | Architectural discovery, task breakdown, dependency mapping | Strictly Read-Only | Structured Technical Specification |
| **`build`** | Test-driven implementation, surgical code edits, verification | Scoped Workspace Read/Write | Class-Based Test Gate + Linters + Compilers |
| **`debug`** (not built) | Scientific root-cause isolation and regression-free resolution | Scoped Workspace Read/Write | Minimal Reproducer Test (Red $\rightarrow$ Green) |
| **`security`** (not built) | Vulnerability auditing, secret detection, dependency CVE scan | Read-Only (Local DBs) | Gitleaks (Secrets) + Semgrep (SAST) |

## Quickstart (MVP, macOS and Linux)

Status: working MVP, tested live only with NVIDIA NIM on macOS. `debug` and `security` profiles are not built; Windows is not supported yet.

```sh
cargo build --release          # binary: target/release/devcode
export NVIDIA_API_KEY=...      # or the env var named in api_key_env; the key is never stored in config
```

Create `.devcode.json` in the repository you want to work on:

```json
{ "provider": { "kind": "openai_compat",
                "base_url": "https://integrate.api.nvidia.com/v1",
                "api_key_env": "NVIDIA_API_KEY",
                "model": "<a model with tool calling>",
                "context_window": 32000 },
  "verification": { "test_command": "python3 -m unittest discover -s tests" } }
```

Any OpenAI-compatible endpoint works by changing `base_url`, `api_key_env` and `model` (`--provider nim` is a preset for NIM). Run:

```sh
devcode --profile ask   "where is the retry logic?"      # read-only
devcode --profile plan  "add rate limiting to the API"   # read-only, structured plan
devcode --profile build "fix the bug in calc.py"         # edits with approval, test gate
devcode rollback                                         # undo the whole latest run
```

Run `devcode` with no prompt in a terminal to open the full-screen interface: live transcript, diff approvals in a modal, masked key entry, and slash commands (`/help`, `/init nim|gemini`, `/connect`, `/ask`, `/plan`, `/build`, `/rollback`). `--plain` keeps the line-based REPL. With no `verification.test_command` configured, build runs finish marked UNVERIFIED instead of looping.

`--trace` writes per-step timing and token counts (no file contents) to `.devcode/trace.jsonl`. Do not paste input while `devcode` runs: approval prompts read stdin. Live results and limits: [`docs/research/first-live-runs.md`](docs/research/first-live-runs.md).

## Install

Works today (needs a Rust toolchain; no release has been published yet):

```sh
cargo install --git https://github.com/RishiiShah/DevCode
```

Prepared but not yet published, see [`packaging/README.md`](packaging/README.md): `install.sh` for release binaries, a Homebrew formula, and `pip`/`pipx` wheels built with maturin.

## Platforms & Install (planned)

Beta: a static site on Cloudflare Pages (`<project>.pages.dev`) serves the two installer scripts; release binaries live in GitHub Releases on a personal account. No organization and no custom domain. `<install-host>` is that subdomain, still a placeholder because the product name is undecided; nothing below works today.

Tier 1: macOS, Linux (glibc/musl), Windows 11. Single self-contained native Rust binary per target; one-line installer is the primary channel (`ATE-1001-PRD` FR-6, `ATE-1000-ARCH` §6.2–6.3). Releases are signed via minisign (Ed25519); Sigstore provenance attestations are planned.

```sh
curl -fsSL https://<install-host>/install.sh | sh   # macOS, Linux, WSL2
irm https://<install-host>/install.ps1 | iex        # Windows PowerShell
```

## Delivery Priorities

1. **Initial Release**: Unified loop, JIT search, persisted pre-step rollback with conflict detection, and trusted behavior-based gates covering configuration changes.
2. **Release Evidence**: Pass rollback and gate-integrity acceptance tests in `ATE-1001-PRD` before extending retrieval.
3. **Deferred Extension**: Enable Tier 2 PageRank only after comparative retrieval results satisfy `ATE-1002-CBU` §4.4.
