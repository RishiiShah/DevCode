# Research Note: Demand for Engineering Discipline and Token Frugality (Pass 11)

```
================================================================================
DOCUMENT ID    : DEVCODE-RES-011
REVISION       : 0.1.0
STATUS         : Working notes (market evidence)
SOURCES        : GitHub issue trackers (REST API, 2026-11-24)
RELATED        : DEVCODE-RES-010; TODO.md P1–P4
================================================================================
```

## Question

Product thesis: agents do extra or sloppy work and burn tokens; DevCode behaves like a senior engineer (minimal change, std lib first, compact agent-to-agent context, plain user-facing replies). What evidence exists?

Labels: **production data** (issue read or listed) | **guess**.

## Method and limits

- 6 keyword searches (claude-code, codex, gemini-cli), sorted by reactions; 3 issue bodies read.
- Keyword search is noisy; titles only for most hits.

## Findings

| Claim | Evidence | Strength |
| :--- | :--- | :--- |
| Models are sloppy and claim completion | claude-code#42796 (closed): "ignores instructions, claims 'simplest fixes' that are incorrect, does the opposite of requested, claims completion against instructions"; 583 comments, 3286 reactions | Strong demand, no concrete cases in body |
| Models overbuild | claude-code#95494 (open): first delivery 3,562 lines and a 38-test suite never requested; scope rewritten four times; 1 comment | One report only |
| Token and quota burn | claude-code#16157 (open, 1499 comments, 728 reactions); #38335 (878 / 545); codex#14593 (closed, 630 / 307) "Burning tokens very fast"; codex#28879 (211 / 557) rate-limit cost jump; codex#41220 usage accounting | Strong demand |
| Context loss from compaction | codex#5957 (open, 33 comments): auto compaction makes the model forget it is mid-task | One report |
| Std-lib-first / dependency discipline | No matching issue found | None |

## Assessment

- Token burn and "sloppy, false completion" have large, real signals (production data).
- Token complaints mix vendor rate-limit changes and plan pricing with client behavior. A harness can cut tokens per task; it cannot fix a provider's limits. Savings per task are unmeasured. [guess]
- "Overbuilding" and "prefer std lib" are weakly evidenced: one report and none.
- Compaction is double-edged: codex#5957 shows lossy compaction harms tasks. Compact formats must keep precision.

## Implications

1. Lead the pitch with verified completion and measured tokens per task, not with minimalism alone.
2. Make every discipline rule measurable: diff size versus request, files outside the declared scope, new dependencies, duplicated code. Rules that only live in prompts are the same model self-grading.
3. Publish tokens-per-task against a peer on a fixed task set before claiming savings.

## Not checked

- Closed issues, forums, Reddit, Hacker News.
- Whether peers already enforce scope or dependency rules.
- Real token savings from any compaction design.
