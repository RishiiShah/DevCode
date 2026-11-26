# RES-015 Harness bake-off protocol

## Question

Do DevCode's verification gate and rollback improve completed-task quality or reduce recovery work at comparable token cost? RES-010 covers demand evidence. This protocol measures workflow outcomes.

## Pilot

Compare DevCode, Codex, and OpenCode in terminal headless build workflows. Record exact harness revisions, model ID, effort, settings, and platform. Use a common model and endpoint where all three support them. If model or endpoint differs, report the result as a separate end-to-end comparison.

Each task starts from the parent of the listed commit. Give each harness the same issue prompt, baseline test command, time budget, and operator policy. Keep the listed solution commit, evaluator tests, and Git history containing the fix out of the agent's workspace. Use a fresh disposable workspace per run. Run baseline tests before evaluating each task. Reject a fixture if its evaluator fails against the known-good commit or passes unchanged against the parent.

| Known-good commit | Task prompt seed | Evaluator starting point |
| --- | --- | --- |
| `c3b9f2a` | Use stored credentials for a known provider endpoint without an explicit connector | `tests/m16_infer_connector.rs` |
| `1dcb532` | Stop repeated identical read/search rounds while allowing progress after a different result | `tests/m10g_gate.rs` |
| `5274129` | Send valid Anthropic tool-choice payloads and preserve thinking during tool turns | `tests/m20_anthropic.rs`, `tests/m24_anthropic_stream.rs` |
| `484e6c5` | Save a project conversation and allow restarting the CLI to continue it | `tests/m28_resume.rs` |
| `6278555` | Resolve context windows by model and show where the value came from | `src/config.rs`, `tests/m15_switching.rs` |
| `22e2381` | Hide non-chat models from interactive suggestions while accepting manually typed IDs | `src/config.rs`, `tests/m15_switching.rs` |
| `4a92fae` | Expire old completed checkpoints and retain blobs needed by recovery | `tests/m3_manifest.rs` |
| `4650348` | Restore tool results, edit diffs, and token counters when resuming | `tests/m28_resume.rs` |
| `d038440` | Treat equivalent workspace paths as the same declared edit scope | `src/discipline/scope.rs` unit tests |
| `9971423` | Restore declared file scope on session resume | `tests/m28_resume.rs` |

These historical DevCode tasks check that the protocol works. Confirm any ranking claim on a separate set of real tasks from other repositories selected before runs begin.

## Measurements

- Per run, record hidden-test result, baseline regressions, wall time, provider-reported input/output/cache tokens, and operator interventions. Record unavailable counters as missing.
- Count a gate intervention only if it rejects an otherwise attempted completion and the next accepted patch resolves the evaluator failure. Record unnecessary blocks and their cost as well.
- For rollback, repeat a fixed rejected-edit scenario with dirty tracked and untracked fixtures. Compare bytes and executable bits before and after, and repeat with a concurrent user edit that must survive. Count manual recovery steps.
- Report pass rate over all runs, tokens for all runs, median tokens among passes, and paired task outcomes. Keep aborted runs in the denominator and identify why they stopped.

## Status

Task candidates and evaluator locations identified. Fixture isolation, hidden evaluator extraction, run budgets, and model selection remain open. No comparative model runs recorded.
