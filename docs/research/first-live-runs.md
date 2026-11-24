# Research Note: First Live Runs (Pass 12)

```
================================================================================
DOCUMENT ID    : DEVCODE-RES-012
REVISION       : 0.1.0
STATUS         : Working notes (measured, small sample)
SOURCES        : Terminal output and .devcode/trace.jsonl from manual runs, 2026-11-24
RELATED        : DEVCODE-RES-011; docs/mvp.md M10a to M10h
================================================================================
```

## Question

Does the MVP loop deliver the thesis (minimal, verified, reversible, low token use) with a real model?

Labels: **production data** (measured in these runs) | **guess**.

## Setup

- Provider: NVIDIA NIM hosted, model `nvidia/nemotron-3.5-super-vl-120b-a12b`, through the OpenAI-compatible adapter. Tool calling worked in the smoke test.
- Platform: macOS arm64. Linux CI passes the test suite (GitHub Actions); no live run on Linux.
- Fixtures: a one-line bug in a 2-file Python project; a 2-file feature in a small Python package.
- Each row is one run (n = 1). The model is not deterministic; do not read these as averages.

## Results (production data)

| Run | Gate rule in force | Steps | Tokens in | Tokens out | Edit approvals | Outcome |
| :--- | :--- | ---: | ---: | ---: | ---: | :--- |
| One-line fix, first | tests-touched required | 17 | 28,318 | 2,045 | 2 | Passed; model added unrequested assertions because the gate blocked finish three times |
| One-line fix, loop | red before first edit only | 13+ | not recorded | not recorded | 5+ | Not finished: model reverted its own fix to create a failing run and looped |
| One-line fix, recovery rule | red, edit, green at any point | 10 | 14,914 | 877 | 3 | Passed; revert-and-reapply dance |
| One-line fix, baseline-first | baseline test run required before first edit | 6 | 8,115 | 414 | 1 | Passed; one edit |
| 2-file feature (accents plus max_length) | baseline-first | 9 | 18,515 | 1,857 | 3 | Passed; rollback restored both files |

- Rollback restored the pre-run bytes in every finished run; `git status` was clean afterward in the runs where I checked it.
- Per-call model latency was 11 to 20 s with about 40 to 130 output tokens per call, so it is not driven by output length. Cause not established; hosted-tier queueing is a guess.
- Whole-run summaries come from the harness's own tool log, not from model text.

## What the runs show

1. Two gate rules I wrote caused bad agent behavior (forced test edits; a loop). Both were found only by running a real model. Scripted tests did not catch them.
2. Making the baseline test run a precondition of the first edit cut steps from 17 to 6 on the same task. One run each, so the size of the gain is unproven.
3. The agent used the standard library (`unicodedata`) for the accent task and touched only the two expected files.

## Weaknesses found

- **The gate does not judge test quality.** In the feature run the model wrote tests, then removed an `else` branch to make its own test pass. The resulting behavior (a single word longer than max_length is cut mid-word) arguably conflicts with the request ("truncates at a dash boundary"). Two of its three new tests overlap. The gate only reports that tests passed.
- **The model's prose summary can misdescribe its own change** (it described one test as checking truncation inside a word when it checks a word boundary). The harness-built verification summary was accurate.
- Token and latency numbers are for one provider and model.

## Not checked

- Any comparison with another agent on the same task (the thesis claim about tokens per task is untested).
- A second provider; `provider.extra_body` live; Linux live runs; tasks larger than two files; repeated runs for variance.
- Whether the baseline-first rule helps or annoys on repositories with slow test suites.
