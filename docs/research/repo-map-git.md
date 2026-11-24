# Research Note: Repo Map & Git/Edit Safety (Pass 2)

```
================================================================================
DOCUMENT ID    : DEVCODE-RES-002
REVISION       : 0.1.0
STATUS         : Working notes (reference study)
SOURCES        : references/aider
RELATED        : docs/codebaseunderstanding.md (Dual-Tier), docs/devcode.md §4–5,
                 docs/prd.md FR-2 / FR-3
================================================================================
```

## Question

How does aider build a token-budgeted repo map (Tree-sitter + PageRank), and how does it protect edits / roll back — relative to DevCode’s JIT-default + threshold-gated PageRank + step-manifest rollback?

## Findings (aider)

### Repo map

| Area | Location | Mechanism |
| :--- | :--- | :--- |
| Tags | `repomap.py:279–301` | Tree-sitter queries (`.scm`) → def/ref `Tag`s; Pygments backfill |
| Graph | `repomap.py:470–525` | `nx.MultiDiGraph`; weighted referencer→definer edges |
| Rank | `RepoMap.get_ranked_tags` | **No custom PPR class** — `networkx.pagerank(..., personalization=, dangling=)` biased to chat/mentioned files |
| Budget | `repomap.py:676–686`, `models.py:782` | Binary-search tag prefix → `to_tree` under `max_map_tokens` (default ~1024, capped) |
| Cache | `repomap.py:43, 233, 586–606` | Per-file mtime tag cache; map cache keyed by file lists + token budget; `refresh=auto|files|always|manual` |
| Scope | `repo.py:433` | File universe = **git HEAD tracked files** |

Map is **always-on / eager** in normal coding flows (scaled when no files in chat; `ContextCoder` forces larger always-refresh). Not a separate “Tier 2” gate.

### Edit + Git safety

| Area | Location | Mechanism |
| :--- | :--- | :--- |
| Apply | `editblock_coder.py`, `udiff_coder.py`, `wholefile_coder.py`, `search_replace.py` | Search-replace / udiff / whole-file; fuzzy multi-strategy replace |
| Pre-edit | `base_coder.py:2175–2411` | `dirty_commit` snapshots dirty paths into git before agent edits |
| Post-edit | `base_coder.py:2375` | `auto_commit` after LLM edits |
| Undo | `commands.py:553–628` | `/undo` only session aider commits; dirty paths require **stash**; soft reset + checkout |

**No step-manifest.** Rollback = git history surgery. User uncommitted work is “protected” by committing it first, which **mutates** the user’s commit timeline — opposite of DevCode FR-3.

## Steal / Reject for DevCode

| Steal | Reject |
| :--- | :--- |
| Tag graph + personalized PageRank weights | Always-on full-repo map |
| Important-file prepend + binary-search token fit | mtime/`refresh=files` weak invalidation as sole freshness |
| Fuzzy multi-strategy search-replace | `dirty_commit` / `auto_commit` as checkpoint model |
| | Stash-gated `/undo` |
| | Git-tree-only universe (no JIT ripgrep tier) |

## Mapping to DevCode Dual-Tier (ATE-1002)

1. **Tier 1 default:** keep JIT ripgrep/glob (aider does not provide this; mini/vibe/codex-style search tools do).
2. **Tier 2:** when ATE-1002 thresholds fire (>20 ambiguous files, >300 sources per CBU §4.2, explicit `plan` architecture ask), run **aider-shaped** tree-sitter tags → NetworkX PPR → ≤2500-token tree — with **≤2s** compute budget from the CBU spec (benchmarked on 500 files per §4.3).
3. **Rollback:** step-manifest (FR-3), never aider’s dirty/auto commit. Pre-step capture must include existence, type, bytes/symlink target, mode, hash — hash alone insufficient.
4. **Open doc gaps that affect this pass:**
   - **Class B undefined** (PRD FR-2.1): “deterministic recognized refactors” has no list → everything defaults Class A until a concrete AST transform allowlist exists.
   - **Subprocess writes** (FR-3.2 / Arch): without macOS FS isolation, either restrict shell to an allowlisted read-only set in `plan`/`security`, or only permit write-capable shell under explicit user approval with known-path capture — “block if uncaptured” otherwise freezes `build`.
   - **`.devcode/` must be deny-listed** for agent writes or manifests/audit are mutable under `build`.

## Stop condition for this pass

Enough to specify Tier-2 map algorithm (steal aider’s PPR + token binary search) and to reject git-commit checkpoints in favor of step-manifest — without reading further map-centric repos yet (`continue` deferred).
