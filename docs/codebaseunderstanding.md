# [ATE-1002] Technical Specification: Codebase Semantic Graph & Understanding Engine

```
================================================================================
STANDARD       : ATE-1000 (Automated Technical Engineering Standard Series)
DOCUMENT ID    : ATE-1002-CBU
REVISION       : 0.3.0-draft
STATUS         : Working Draft (Engineering Specification)
AUTHOR         : DevCode Core Engineering Group
RELATED DOCS   : [docs/devcode.md](devcode.md), [docs/prd.md](prd.md)
================================================================================
```

---

## 1.0 Executive Summary & Objective

This specification formalizes the codebase understanding and semantic graph architecture for the **DevCode** autonomous developer harness. 

To maintain high developer velocity and sub-second startup times, DevCode rejects persistent vector databases and heavy ingestion daemons. Instead, it pairs **Agentic Just-in-Time (JIT) search** (via Rust-native `ripgrep`) with an **on-demand, threshold-gated Topological Graph Engine** (Tree-sitter + Personalized PageRank).

For the overarching harness architecture, refer to [docs/devcode.md](devcode.md). For functional requirements and benchmark recipes, see [docs/prd.md](prd.md).

---

## 2.0 State-of-the-Art Industry Archetypes

```
+---------------------------------------------------------------------------------------------------+
|                         CODEBASE UNDERSTANDING: ARCHITECTURAL ARCHETYPES                          |
+------------------------------------+-------------------------------+------------------------------+
| ARCHETYPE A: AGENTIC JIT           | ARCHETYPE B: REPO MAP         | ARCHETYPE C: HYBRID VECTOR   |
| (Claude Code, Google AGY)          | (Aider, Sourcegraph SCIP)     | (Cursor, Copilot Workspace)  |
+------------------------------------+-------------------------------+------------------------------+
| - Zero pre-indexing / no vector DB | - Tree-sitter AST symbol tags | - Merkle-tree change sync    |
| - Rust-native ripgrep live search  | - Directed call/ref graph     | - AST-aware chunk slicing    |
| - Ephemeral context accumulation   | - Personalized PageRank       | - Dense HNSW + BM25 sparse   |
| - Zero startup lag on branch hops  | - Token-budgeted prompt map   | - Cached content-hash embeds |
+------------------------------------+-------------------------------+------------------------------+
```

### 2.1 Archetype A: Agentic Just-in-Time (JIT) Exploration
- **Representative Systems**: Anthropic Claude Code, Google Antigravity (AGY).
- **Core Mechanism**: Leverages optimized OS utilities (`Glob` for directory topology, multi-threaded `ripgrep` for regex content matches) without pre-indexing. Context is assembled on-demand through targeted line-range reads.
- **Strength**: Instant zero-second startup; 100% fresh on git branch hops and rebases.

### 2.2 Archetype B: Topological AST Graph & Personalized PageRank
- **Representative Systems**: Aider, Sourcegraph SCIP / LSIF.
- **Core Mechanism**: Parses source files into concrete syntax trees using Tree-sitter, extracts definitions and call references, and constructs a directed graph. Runs Personalized PageRank (PPR) biased toward the active working set.
- **Strength**: Provides structural cross-file awareness inside a compressed token budget (1,000–2,500 tokens).

### 2.3 Archetype C: Cryptographic Merkle Sync & Hybrid Retrieval
- **Representative Systems**: Cursor, GitHub Copilot Workspace.
- **Core Mechanism**: Maintains a vector store with AST-chunked embeddings, synchronizing updates via cryptographic Merkle trees. Searches using dense embeddings combined with BM25 keyword matching via Reciprocal Rank Fusion (RRF).
- **Strength**: High semantic recall for natural-language conceptual queries.

---

## 3.0 Comparative Evaluation & Trade-off Matrix

| Engineering Criterion | Archetype A: Agentic JIT | Archetype B: Repo Map (PPR) | Archetype C: Hybrid Vector |
| :--- | :--- | :--- | :--- |
| **Startup / Indexing Latency** | **0 seconds (Instant)** | 2–8 seconds (Local AST) | 2–10+ minutes (Initial embed) |
| **Git Branch Switching Lag** | **Zero (Always fresh)** | Delta re-parse (<1 sec) | Merkle sync check required |
| **Disk & Memory Footprint** | Near zero (Ephemeral) | Minimal (In-memory / SQLite) | High (Vector store / indices) |
| **Cross-File Symbol Precision** | High (Targeted grep/LSP) | Deterministic (Syntax graph) | Probabilistic (Embedding rank) |
| **Token Budget Consumption** | Dynamic pay-as-you-explore | Fixed baseline (1k–2.5k tokens)| Search result chunk overhead |
| **Air-Gapped / Offline Feasibility** | 100% Native | 100% Native | Requires local embedder model |

---

## 4.0 DevCode Dual-Tier Hybrid Graph Specification

DevCode avoids running PageRank speculatively on every simple query. The initial release implements Tier 1 only. The **Dual-Tier Model** below specifies a deferred extension with explicit activation thresholds; those thresholds apply only after the delivery prerequisites in §4.4 pass.

```
                              [ DevCode Engine ]
                                      |
            +-------------------------+-------------------------+
            |                                                   |
            v                                                   v
   +-----------------------+                           +-----------------------+
   |   TIER 1: JIT LAYER   |                           |  TIER 2: GRAPH ENGINE  |
   | (Default / Zero Cost) |                           | (Threshold-Gated / PPR)|
   +-----------------------+                           +-----------------------+
   | - Embedded ripgrep    |                           | - Tree-sitter AST Map |
   | - Fast Glob Matcher   |                           | - Dependency DiGraph  |
   | - Surgical File Read  |                           | - Personalized PageRank|
   | - Fresh on branch hop |                           | - Token cap: 2,500 tok|
   +-----------------------+                           +-----------------------+
```

### 4.1 Tier 1: Zero-Latency JIT Layer (Default Path)
- **Engine**: Embedded Rust `ripgrep` runner coupled with filename globbers.
- **Latency**: $\le 150\text{ ms}$ on warm disk cache.
- **System Prompt Token Overhead**: **0 baseline tokens** added to the system prompt; tokens are consumed only for matched file lines during tool results.

### 4.2 Tier 2: Topological Graph Engine (Measured Activation Thresholds)
Tier 2 is **not** activated by guesswork, nor by uncomputable metrics like symbol counts that require parsing to evaluate. It is engaged strictly when one of the following concrete conditions is met:

1. **Repository Scale Threshold**:
   - The workspace contains $> 300$ source files (determined via fast filesystem directory traversal without opening file contents). This is a provisional tuning threshold, not a measured recall guarantee.
2. **Measured Tier 1 Miss Rate / Ambiguity**:
   - When a JIT symbol search yields $> 20$ candidate files across disparate directories.
   - When a query contains no unique lexical identifier (pure natural language architectural query).
3. **Explicit Cross-Module Intent**:
   - When the user explicitly requests an architectural plan, dependency audit, or widespread refactoring via the `plan` profile.

### 4.3 PageRank Computational & Token Ceilings
- **Execution Budget**: Graph traversal and PageRank calculation must complete within $\le 2.0\text{ seconds}$ on a 500-file reference repo.
- **Token Ceiling**: The serialized repository map injected into the prompt must **never exceed 2,500 tokens**. Low-ranked nodes are pruned down to file paths without function signatures to preserve token budget.

### 4.4 Delivery Prerequisites & Evidence
1. **Initial Release**: Ship JIT search with the unified loop, persisted rollback, and trusted classification gates. Pass PRD Gates 1, 2, 4, and 5 before adding the graph engine.
2. **Tier 2 Evaluation**: Record JIT failures on a versioned task set with expected relevant files. Compare JIT and JIT-plus-graph recall, elapsed time, and context tokens on identical tasks and repository revisions. Include cross-module queries and branch-change freshness checks.
3. **Enablement Decision**: Keep Tier 2 deferred until recorded results demonstrate retrieval gains within §4.3 budgets. Record the task set, hardware, results, and selected thresholds before enabling it; repository size alone is not evidence of benefit.
4. **Fallback**: Missing grammars, graph timeout, or stale graph state falls back to fresh JIT results and reports the limitation. Graph results never relax verification gates.

---

## 5.0 Security Analysis Strategy: Tool Orchestration

Rather than attempting to build a custom AST taint-tracking engine (an intractable research project), DevCode adopts a pragmatic **Tool Orchestration Architecture**:

```
+----------------------------------------------------------------------------------+
|                          SECURITY AUDITING ORCHESTRATOR                          |
+----------------------------------------------------------------------------------+
|                                                                                  |
|   +-----------------------+  +-----------------------+  +---------------------+  |
|   |       GITLEAKS        |  |        SEMGREP        |  |     OSV-SCANNER     |  |
|   |   (Secret Scanner)    |  |    (SAST Analysis)    |  |   (Dependency CVE)  |  |
|   +-----------+-----------+  +-----------+-----------+  +----------+----------+  |
|               |                          |                         |             |
|               +--------------------------+-------------------------+             |
|                                          |                                       |
|                                          v                                       |
|                       +--------------------------------------+                   |
|                       |  DevCode Security Normalizer Layer   |                   |
|                       |  - Deduplicates findings             |                   |
|                       |  - Maps issues to file lines         |                   |
|                       |  - Emits actionable patch plan       |                   |
|                       +--------------------------------------+                   |
|                                                                                  |
+----------------------------------------------------------------------------------+
```

1. **Secret Detection**: Runs `gitleaks` against current uncommitted diffs to catch exposed API keys, tokens, and private certificates.
2. **Static Analysis (SAST)**: Executes `semgrep` with standardized OWASP Top 10 rulesets to detect injection patterns, unsafe deserialization, and path traversals.
3. **Dependency CVE Auditing**: Executes `osv-scanner` against lockfiles (`package-lock.json`, `poetry.lock`, `Cargo.lock`, `go.sum`).
4. **Offline-First Vulnerability Database**:
   - Scans run **strictly offline** using locally cached vulnerability definitions.
   - Database updates are decoupled from runs and triggered explicitly by the user via `devcode security --update-db`.

---

## 6.0 Normative References & Traceability

- **System Architecture**: [docs/devcode.md](devcode.md) (ATE-1000-ARCH)
- **Product Requirements**: [docs/prd.md](prd.md) (ATE-1001-PRD)
- **External Specifications**:
  - Tree-sitter CST Grammars (`https://tree-sitter.github.io`)
  - Semgrep Rule Registry (`https://semgrep.dev/rules`)
  - Open Source Vulnerabilities Schema (`https://ossf.github.io/osv-schema/`)
