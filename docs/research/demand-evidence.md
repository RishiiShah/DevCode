# Research Note: Demand Evidence for Harness-Enforced Test Gates (Pass 10)

```
================================================================================
DOCUMENT ID    : DEVCODE-RES-010
REVISION       : 0.2.0 (rewrite; supersedes 0.1.0)
STATUS         : Working notes (market and engineering evidence)
SOURCES        : GitHub issue trackers (REST API, 2026-11-23); local clones
RELATED        : docs/prd.md FR-2, Gate 2; docs/devcode.md §5.0–5.2; TODO.md T12
================================================================================
```

## Question

Do developers ask for a harness that blocks finalization until tests exist and pass, with no model self-grading?

Labels: **code** (local clone) | **production data** (GitHub issue or PR, read) | **vendor claim** | **guess**.

## Correction to 0.1.0

0.1.0 cited sources that do not say what it claimed. Removed after reading the issue bodies:

- swe-agent #1526, mini-swe-agent #953: prompt-injection hardening RFCs, not test tampering.
- aider #3362: feature ideas from Claude Code, no assertion-deletion reports.
- gemini-cli #29393 / #29396: to-do persistence and AST search PRs, not verification state.
- pi #7547 / #7730: Windows usage thread and macOS CPU bug, not TDD complaints.
- codex #51601 / #51524: Windows sandbox and Computer Use bugs, not process trees.
- "swe-agent/eval" harness: no such directory in the clone.
- Claude 3.5/3.7 "specification gaming" reports: vendor claim, not checked.
- Conclusion "real and grounded in production data and code": not supported.

## Method and limits

- 8 GitHub keyword searches (codex, claude-code, gemini-cli, cline, aider, opencode), 6 results read per query by title; 5 issue bodies read in full.
- Keyword search is noisy and misses paraphrase. Absence of hits is weak evidence.
- Counts of comments and reactions are the only demand-size signal available.

## Findings

### Failure mode exists: model claims verification it did not do (production data)

| Source | Date | State | Comments / reactions | What it says |
| :--- | :--- | :--- | :--- | :--- |
| anthropics/claude-code#97155 | 2026-09-25 | open | 5 / 0 | Model reported a CI fix as done without running the failing tests; the fix was incomplete |
| anthropics/claude-code#99083 | 2026-10-02 | open | 0 / – | Model skipped the test runs that mattered, gave verdicts anyway |
| anthropics/claude-code#95494 | 2026-09-19 | open | 1 / – | Agent presented hardcoded fake test results as real (single user's report) |

### Nearest explicit request (production data)

- **anthropics/claude-code#101247** (open, 3 comments, 0 reactions): asks for a verification summary built from the harness tool log, not model text, showing which tests ran, exit codes, and whether they ran after the last edit. This is the non-self-grading principle (arch tenet 3), but it asks for **display**, not a blocking gate.

### Demand for rollback and approvals, not gates (production data)

- **openai/codex#9203** (open): "Please make /undo back". Author: Codex deleted untracked files and edited uncommitted work. Supports FR-3 (rollback), not FR-2.
- **openai/codex#59060** (closed, 440 comments; title only, body not read): "Told to reboot a server, it destroyed systemd, destroyed containers…". Large thread on destructive actions; relates to approvals and runtime guard.
- **kilocode PR #14228** (open): config-edit approval setting, default on. Supports FR-2.4 and `.devcode/` protection.

### Not found

- No issue in the sampled trackers reporting an agent **deleting or weakening assertions** to pass tests. Searches may have missed it.
- No issue asking for a gate that **blocks** completion until tests exist.
- aider #5898 (open): benchmark runner leaves skipped tests unexecuted. Harness-side test-runner bug, not agent behavior.

## Assessment

- The failure mode (unverified "tests pass" claims) is real: 3 reports plus 1 feature request, all in one tracker, all from Sep–Oct 2026.
- Demand size is small: 0 reactions, 0–5 comments each. No proof users want a blocking gate.
- Rollback and destructive-action protection have stronger signals than test gates.
- **FR-2 (Class A/C gate) is a product hypothesis, not a demonstrated need.** [guess]

## Implications

1. Keep FR-2; label it hypothesis in planning.
2. Cheapest first validation: build the I7 gate and measure how often agents on a fixed task set finish without running tests (internal dogfood), before investing in AST work.
3. Consider shipping the display-only verification summary (#101247) first; it is a subset of FR-2.2 "bind results to the checked diff hash".
4. Order roadmap by evidence: rollback (FR-3) and runtime guard (FR-4) before gate polish.

## Not checked

- Closed issues in most trackers, discussions, Reddit, Hacker News, Discord.
- Full bodies of 3 of the 8 queries' hits.
- Whether peers already ship similar gates (only swe-agent read in RES-006).
