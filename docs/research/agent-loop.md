# Research Note: Agent Loop (Pass 1)

```
================================================================================
DOCUMENT ID    : DEVCODE-RES-001
REVISION       : 0.1.0
STATUS         : Working notes (reference study)
SOURCES        : references/mini-swe-agent, references/mistral-vibe
RELATED        : docs/devcode.md §1–3, docs/prd.md FR-1 / FR-4
================================================================================
```

## Question

How should DevCode structure the single agent loop (message → model → tools → continue/stop), tool permissions, and stop limits — without becoming a multi-state control-flow engine?

## Findings

### mini-swe-agent (steal skeleton)

| Area | Location | Mechanism |
| :--- | :--- | :--- |
| Loop | `agents/default.py:88–157` | `run()` → `while True: step()` = `execute_actions(query())` |
| Tools | `models/utils/actions_toolcall.py`, `litellm_model.py` | Single `bash` tool via LiteLLM `tools=[]` |
| Stop | `default.py:26–32, 132–140`; `exceptions.py` | `step_limit`, `cost_limit`, `wall_time_limit`; `role=="exit"` / `Submitted` |
| Prompt | Jinja templates in `config/mini.yaml` | System + instance templates; observation head/tail elision |
| Entry | `run/mini.py` | Typer → `get_agent` → `agent.run(task)` |

**Mechanism:** Render system+task into `messages`, loop model query → parse tool calls → `env.execute` → append observations, until an `exit` message (limits, submit, repeated format errors).

### mistral-vibe (steal gates, reject shape)

| Area | Location | Mechanism |
| :--- | :--- | :--- |
| Loop | `harness/core/.../turn.rs` | Rust turn SM: completion → tool batch → continue/finish |
| Tools | `vibe/core/tools/manager.py`, `_provided_tools.py` | Discovered builtins + schema; dispatch by name |
| Permissions | `_unified_permissions.py:497–514` | `_decide` before tool execution |
| Stop | `turn.rs:813–823`; CLI `TurnCounter` / `StopRequests` | Per-turn `max_iterations` + identical-fail; separate run-level caps |
| Prompt | `_runtime.py:372–452` | System instructions + project context + AGENTS.md |

**Mechanism:** CLI starts a turn over JSON-RPC; harness packs messages/tools, runs LLM, executes tool batch under a permission gate, then continues or completes. Limits are split: turn iterations vs session/run caps.

## Steal / Reject for DevCode

| Steal | Reject |
| :--- | :--- |
| Single `while` loop (mini) | Split Rust/Python harness (vibe) |
| Step / cost / wall-time limits | One-tool-is-bash-only model (mini) |
| Pre-tool permission `_decide` (vibe) | Forced multi-state mode machine |
| Dual limits: per-turn iterations + run caps | |
| Format-error recovery with capped retries | |

## Mapping to DevCode

- **Profiles plug in at the permission + `tools=[]` boundary**, not as separate loops (`plan` = read-only tools; `build` unlocks writes + gates).
- **Verification gates run after a file batch**, outside the model’s self-assessment (aligns with Arch “no self-grading”).
- Open doc gaps that block a correct loop implementation (from local audit; not fixed here):
  1. **security profile**: who applies remediation diffs? (PRD “patch generation only” vs write path)
  2. **Subprocess write capture** (FR-3.2): on macOS without FS sandbox, “capture or block” collapses to block-almost-everything unless scoped differently
  3. **`.devcode/` allowlist exclusion**: audit log / manifests must be outside agent write scope or they are not “immutable”
  4. **Command blacklist**: treat as UX hint for accidents only; do not claim it as a control

## Stop condition for this pass

Enough to implement a prototype loop + profile-gated tool allowlist without opening further references for loop structure.
