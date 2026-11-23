# DevCode

**DevCode** is an autonomous, developer-centric agentic software engineering harness (in the class of Claude Code, Google Antigravity/AGY, and OpenCode) engineered with a unified execution loop, dynamic policy profiles, and deterministic verification gates to enforce disciplined software development practices.

---

## Technical Documentation Suite (ATE-1000 Series)

All engineering specifications are maintained under [`docs/`](docs/) following the **ATE-1000 Technical Documentation Series**:

| Document | Identifier | Revision | Scope |
| :--- | :--- | :--- | :--- |
| **[System Architecture](docs/devcode.md)** | `ATE-1000-ARCH` | `0.3.0-draft` | Unified agent loop, dynamic policy profiles, pragmatic sandbox runtime, persisted pre-step rollback, and trusted behavior-based verification gates. |
| **[Product Requirements](docs/prd.md)** | `ATE-1001-PRD` | `0.3.0-draft` | Product vision, functional requirements (FR-1 to FR-5), testable NFR benchmark recipes, and deterministic quality gates. |
| **[Codebase Understanding](docs/codebaseunderstanding.md)** | `ATE-1002-CBU` | `0.3.0-draft` | Comparative analysis of indexing archetypes (JIT vs. PageRank vs. Vector), deferred, evidence-gated Tier 2 PageRank engine, and security tool orchestration. |

---

## Dynamic Policy Profiles

| Profile | Purpose | Tool Permissions | Enforced Gate |
| :--- | :--- | :--- | :--- |
| **`plan`** | Architectural discovery, task breakdown, dependency mapping | Strictly Read-Only | Structured Technical Specification |
| **`build`** | Test-driven implementation, surgical code edits, verification | Scoped Workspace Read/Write | Class-Based Test Gate + Linters + Compilers |
| **`debug`** | Scientific root-cause isolation and regression-free resolution | Scoped Workspace Read/Write | Minimal Reproducer Test (Red $\rightarrow$ Green) |
| **`security`** | Vulnerability auditing, secret detection, dependency CVE scan | Read-Only (Local DBs) | Gitleaks (Secrets) + Semgrep (SAST) |

## Delivery Priorities

1. **Initial Release**: Unified loop, JIT search, persisted pre-step rollback with conflict detection, and trusted behavior-based gates covering configuration changes.
2. **Release Evidence**: Pass rollback and gate-integrity acceptance tests in `ATE-1001-PRD` before extending retrieval.
3. **Deferred Extension**: Enable Tier 2 PageRank only after comparative retrieval results satisfy `ATE-1002-CBU` §4.4.
