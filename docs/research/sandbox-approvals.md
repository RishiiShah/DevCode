# Research Note: Sandbox & Approvals (Pass 3)

```
================================================================================
DOCUMENT ID    : DEVCODE-RES-003
REVISION       : 0.1.0
STATUS         : Working notes (reference study)
SOURCES        : references/codex (bfb5f7d), references/gemini-cli (eecccfc)
RELATED        : docs/devcode.md §4 / §6, docs/prd.md FR-4
================================================================================
```

## Question

How do peer CLIs sandbox tool/shell execution and gate approvals — what fits DevCode’s honest macOS model (FS allowlist, env strip, PGID timeout, modes as profiles)?

## Findings

### codex

| Topic | Mechanism | Location |
| :--- | :--- | :--- |
| OS sandbox | macOS `sandbox-exec` + SBPL; Linux bwrap / Landlock | `sandboxing/src/seatbelt.rs`, `linux-sandbox/` |
| Paths | `canonicalize` on roots; FS policy entries | `seatbelt.rs:192–197`, `protocol/.../permissions.rs` |
| Network / secrets | Proxy env strip; credential broker virtualizes child env | `network-proxy/` |
| Timeout / PGID | Default exec timeout **10s**; kill process group on timeout/cancel | `core/src/exec.rs:67, 1004–1037` |
| Approvals | `ExecApprovalRequirement` + session cache; `AskForApproval` modes | `tools/orchestrator.rs`, `sandboxing.rs` |
| Command policy | Layered `ExecPolicyManager` → Forbidden / Prompt / Allow | `exec_policy.rs` |

### gemini-cli

| Topic | Mechanism | Location |
| :--- | :--- | :--- |
| OS sandbox | Seatbelt / bwrap managers; **`NoopSandboxManager`** = sanitize only | `MacOsSandboxManager.ts`, `sandboxManager.ts:289–307` |
| Paths | `resolveSandboxPaths` / realpath; deny `.env` secrets | `sandboxManager.ts:208–401` |
| Env | `sanitizeEnvironment` allow/block + credential-helper strip | `environmentSanitization.ts`, `shellExecutionService.ts` |
| Timeout / PGID | Detached spawn; `killProcessGroup` SIGTERM→SIGKILL (AbortSignal-driven) | `process-utils.ts`, `shellExecutionService.ts` |
| Approvals | `ALLOW` / `DENY` / `ASK_USER`; modes DEFAULT / AUTO_EDIT / YOLO / PLAN | `policy/`, `confirmation-bus/` |
| Command policy | TOML tool rules; dangerous cmds force ASK unless YOLO | `policy-engine.ts`, `commandSafety.ts` |

## Steal / Reject

| Steal | Reject |
| :--- | :--- |
| Pre-tool approval pipeline + session “approved for session” cache (codex) | Mandatory Seatbelt/`sandbox-exec` on every macOS shell |
| realpath allowlist + secret-path denylist (gemini) | YOLO as default; docker-outer sandbox as baseline |
| PGID teardown with SIGTERM→SIGKILL (both) | Full network proxy + credential broker virtualization (v1) |
| Policy decisions as ALLOW/DENY/ASK — map to DevCode profiles | Treating command denylist classifiers as hard security |

## Mapping to DevCode open issues

- **Audit #3 (subprocess writes):** Neither peer solves macOS “capture all shell writes.” They either OS-sandbox FS or accept risk + approvals. DevCode should: (a) prefer structured edit tools over free shell for writes; (b) `plan`/`security` deny mutating shell; (c) `build` shell writes require approval **or** known-path allowlist — drop “capture or block everything” as written.
- **Audit #5 (blacklist):** Align docs with peers — policy engine + approvals, not regex denylist as a control.
- **Audit #4 (`.devcode/`):** Put harness state on the secret/deny path list (same idea as gemini `.env` denylist).

## Stop condition

Enough to specify v1 runtime: realpath allowlist, env strip, PGID watchdog, approval bus, optional Linux bwrap later — without implementing Seatbelt as a hard dependency.
