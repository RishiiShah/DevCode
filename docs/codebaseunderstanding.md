# [ATE-1002] Technical Specification: Codebase Semantic Graph & Understanding Engine

```
================================================================================
STANDARD       : ATE-1000 (Automated Technical Engineering Standard Series)
DOCUMENT ID    : ATE-1002-CBU
REVISION       : 0.3.2-draft
STATUS         : Working Draft (Engineering Specification)
AUTHOR         : DevCode Core Engineering Group
OWNS           : Codebase retrieval design (JIT, Tier 2 PageRank) and security
                 tool orchestration. Budgets: ATE-1001-PRD NFR-3/4/5.
SECTIONS       : 2-3 archetypes + trade-offs | 4 dual-tier spec + Tier 2
                 gate | 5 security tools
RELATED DOCS   : [docs/devcode.md](devcode.md), [docs/prd.md](prd.md)
================================================================================
```

---

## 1.0 Executive Summary & Objective

DevCode rejects persistent vector databases and heavy ingestion daemons to keep sub-second startup. It pairs **agentic Just-in-Time (JIT) search** (Rust-native `ripgrep`) with an **on-demand, threshold-gated topological graph engine** (Tree-sitter + Personalized PageRank). Harness architecture: ATE-1000-ARCH. Requirements and benchmark recipes: ATE-1001-PRD.

---

## 2.0 State-of-the-Art Industry Archetypes

### 2.1 Archetype A: Agentic Just-in-Time (JIT) Exploration
- **Systems**: Anthropic Claude Code, Google Antigravity (AGY).
- **Mechanism**: OS utilities (`Glob` for topology, multi-threaded `ripgrep` for content) with no pre-indexing. Context is assembled on demand through targeted line-range reads. Ephemeral; no vector DB.
- **Strength**: zero startup; 100% fresh on branch hops and rebases.

### 2.2 Archetype B: Topological AST Graph & Personalized PageRank
- **Systems**: Aider, Sourcegraph SCIP / LSIF.
- **Mechanism**: Tree-sitter parses source into concrete syntax trees, extracts definitions and call references into a directed graph, and runs Personalized PageRank (PPR) biased toward the active working set. The map is token-budgeted for the prompt.
- **Strength**: structural cross-file awareness inside 1,000–2,500 tokens.

### 2.3 Archetype C: Cryptographic Merkle Sync & Hybrid Retrieval
- **Systems**: Cursor, GitHub Copilot Workspace.
- **Mechanism**: A vector store of AST-chunked embeddings, synchronized by Merkle trees, searched with dense embeddings plus BM25 via Reciprocal Rank Fusion (RRF). Cached content-hash embeddings.
- **Strength**: high semantic recall for natural-language conceptual queries.

---

## 3.0 Comparative Evaluation & Trade-off Matrix

| Criterion | A: Agentic JIT | B: Repo Map (PPR) | C: Hybrid Vector |
| :--- | :--- | :--- | :--- |
| **Startup / indexing latency** | **0 s** | 2–8 s (local AST) | 2–10+ min (initial embed) |
| **Branch switching lag** | **None (always fresh)** | Delta re-parse (<1 s) | Merkle sync check |
| **Disk & memory** | Near zero | Minimal (memory / SQLite) | High (vector store) |
| **Cross-file symbol precision** | High (targeted grep/LSP) | Deterministic (syntax graph) | Probabilistic (embedding rank) |
| **Token budget** | Pay-as-you-explore | Fixed baseline (1k–2.5k) | Search-chunk overhead |
| **Air-gapped feasibility** | 100% native | 100% native | Needs local embedder |

---

## 4.0 DevCode Dual-Tier Hybrid Graph Specification

DevCode does not run PageRank speculatively on every query. The initial release implements Tier 1 only. Tier 2 below is a deferred extension with explicit activation thresholds that apply only after the §4.4 prerequisites pass.

- **Tier 1: JIT layer** (default, zero cost): embedded ripgrep, fast glob matcher, surgical file reads.
- **Tier 2: graph engine** (threshold-gated): Tree-sitter AST map, dependency DiGraph, Personalized PageRank, 2,500-token cap.

### 4.1 Tier 1: Zero-Latency JIT Layer (Default Path)
- **Engine**: embedded Rust `ripgrep` runner plus filename globbers.
- **Latency**: <= 150 ms on warm disk cache.
- **System prompt overhead**: **0 baseline tokens**; tokens are spent only on matched lines in tool results.

### 4.2 Tier 2: Topological Graph Engine (Measured Activation Thresholds)
Tier 2 is **not** activated by guesswork or by metrics that need parsing to evaluate. It engages only when one of these holds:
1. **Repository scale**: the workspace has > 300 source files (counted by directory traversal without opening files). Provisional tuning threshold, not a measured recall guarantee.
2. **Measured Tier 1 miss rate or ambiguity**: a JIT symbol search yields > 20 candidate files across disparate directories, or the query has no unique lexical identifier (pure natural-language architectural query).
3. **Explicit cross-module intent**: the user requests an architectural plan, dependency audit, or widespread refactor via the `plan` profile.

### 4.3 PageRank Computational & Token Ceilings
- **Execution budget**: traversal and PageRank complete in <= 2.0 s on a 500-file reference repo (PRD NFR-5).
- **Token ceiling**: the serialized repository map never exceeds 2,500 tokens. Low-ranked nodes are pruned down to file paths without function signatures.

### 4.4 Delivery Prerequisites & Evidence
1. **Initial release**: ship JIT search with the unified loop, persisted rollback, and trusted classification gates. Pass PRD Gates 1, 2, 4, and 5 before adding the graph engine.
2. **Tier 2 evaluation**: record JIT failures on a versioned task set with expected relevant files. Compare JIT and JIT-plus-graph recall, elapsed time, and context tokens on identical tasks and repository revisions, including cross-module queries and branch-change freshness checks.
3. **Enablement decision**: keep Tier 2 deferred until results show retrieval gains within §4.3 budgets. Record the task set, hardware, results, and selected thresholds first; repository size alone is not evidence of benefit.
4. **Fallback**: missing grammars, graph timeout, or stale graph state falls back to fresh JIT results and reports the limitation. Graph results never relax verification gates.

---

## 5.0 Security Analysis Strategy: Tool Orchestration

DevCode orchestrates proven tools instead of building a custom AST taint engine. Three scanners (`gitleaks`, `semgrep`, `osv-scanner`) feed a normalizer layer that deduplicates findings, maps them to file lines, and emits an actionable patch plan (consumed by the `security` profile, ATE-1000-ARCH §3.1; requirements FR-5).

1. **Secret detection**: `gitleaks` against current uncommitted diffs for exposed API keys, tokens, and private certificates.
2. **Static analysis (SAST)**: `semgrep` with standardized OWASP Top 10 rulesets for injection patterns, unsafe deserialization, and path traversal.
3. **Dependency CVE auditing**: `osv-scanner` against lockfiles (`package-lock.json`, `poetry.lock`, `Cargo.lock`, `go.sum`).
4. **Offline-first vulnerability database**: scans run strictly offline against locally cached definitions. Updates are decoupled and triggered by `devcode security --update-db`.

---

## 6.0 Normative References & Traceability

- **System Architecture**: [docs/devcode.md](devcode.md) (ATE-1000-ARCH)
- **Product Requirements**: [docs/prd.md](prd.md) (ATE-1001-PRD)
- **Reference Catalog**: [references/README.md](../references/README.md) (ATE-1003-REF)
- **Empirical Research Series**: [docs/research/](research/) (DEVCODE-RES-001 through DEVCODE-RES-014; informative)
- **External**: Tree-sitter grammars (`https://tree-sitter.github.io`), Semgrep Rule Registry (`https://semgrep.dev/rules`), OSV schema (`https://ossf.github.io/osv-schema/`)
