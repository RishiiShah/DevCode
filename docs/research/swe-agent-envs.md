# Research Note: SWE-agent Execution Environments (Pass 6)

```
================================================================================
DOCUMENT ID    : DEVCODE-RES-006
REVISION       : 0.1.0
STATUS         : Working notes (reference study)
SOURCES        : references/swe-agent (HEAD 3ea751c)
RELATED        : docs/devcode.md §4–6, docs/prd.md FR-2 / FR-4
NOTE           : mini-swe-agent covered in DEVCODE-RES-001; not re-derived here
================================================================================
```

## Question

How does full SWE-agent isolate execution and attach verification — anything DevCode should steal for runtime boundaries and class-based gates?

## Findings

| Area | Location | Mechanism |
| :--- | :--- | :--- |
| Env façade | `sweagent/environment/swe_env.py` | `DeploymentConfig` → docker / modal / local / dummy via SWE-ReX; `communicate` / read / write / execute |
| Repo in env | `environment/repo.py` | Upload local tree or `PreExistingRepoConfig` (bind mount / image `testbed`) |
| Protocol | `types.py` `StepOutput`; `agents.py` `forward` → `handle_action` | Model → parse → blocklist → `env.communicate` |
| Hooks | `environment/hooks`, `agent/hooks`, `run/hooks` | Lifecycle hooks around deployment, actions, instance runs |
| “Verification” | `agents.py` submit / optional `review_on_submit_m`; SWE-bench external | **No post-edit test gate** — agent may run pytest via bash; framework does not require it |
| Rollback | `repo.py` git reset recipe; `swe_env.hard_reset` | Attempt-scoped container restart + git clean — not step-manifest |
| Safety | `tools/tools.py`, `models.py` | Command blocklist; per-cmd 30s + total 1800s; cost/call caps; obs truncate |

## Steal / Reject

| Steal | Reject |
| :--- | :--- |
| Single env façade over docker/local so gates run identically | Bash-as-only tool protocol |
| Env / agent / run hook chains to attach gate results to `StepOutput.state` | Patch-submit or LLM reviewer as sole verification |
| Layered budgets (per-cmd, total exec, cost, calls) | Git `hard_reset` as DevCode’s primary rollback |
| Attempt-level reset for multi-try debug loops | Blocklist as a security control (UX only) |

## Mapping to DevCode

- **Class A/B/C gates** must be harness-owned after file batches — SWE-agent proves peers often leave tests to the model; that is the gap DevCode claims.
- **Runtime:** keep pragmatic local PGID + allowlist; optional docker later behind the same façade pattern if needed for CI.
- **Rollback:** step-manifest (FR-3), not SWE-agent git reset inside a container.

## Stop condition

Enough to reject “agent ran tests in chat” as a gate, and to copy hook/façade shape if/when DevCode adds a deployment abstraction.
