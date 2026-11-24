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
| **`debug`** | Scientific root-cause isolation and regression-free resolution | Scoped Workspace Read/Write | Minimal Reproducer Test (Red $\rightarrow$ Green) |
| **`security`** | Vulnerability auditing, secret detection, dependency CVE scan | Read-Only (Local DBs) | Gitleaks (Secrets) + Semgrep (SAST) |

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
