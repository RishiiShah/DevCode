# Research Note: Modes as Permission Lenses (Pass 4)

```
================================================================================
DOCUMENT ID    : DEVCODE-RES-004
REVISION       : 0.1.0
STATUS         : Working notes (reference study)
SOURCES        : references/roo-code (b867ec9), references/cline (34c104e)
RELATED        : docs/devcode.md §3, docs/prd.md FR-1
================================================================================
```

## Question

Are modes separate control-flow FSMs, or permission+prompt lenses on one loop?

## Verdict

**Both: one loop + mode as lens.** Roo swaps slug/prompt/tools in place; Cline rebuilds the session runtime on plan↔act so tools/prompt/guards stay aligned. Neither is a multi-state product ceremony.

## Findings

### roo-code

| Topic | Mechanism | Location |
| :--- | :--- | :--- |
| Schema | `modeConfigSchema`: slug, roleDefinition, `groups` (± `fileRegex`) | `packages/types/src/mode.ts:96–227` |
| Built-ins | architect, code, ask, debug, orchestrator | same |
| Switch | Webview / slash `mode:` / model `switch_mode` tool (user approve) | `ClineProvider.ts`, `SwitchModeTool.ts` |
| Enforce | `isToolAllowedForMode` / `validateToolUse` at execution | `validateToolUse.ts:32–238` |
| Read-only | `ask` = read+mcp; architect edit limited to `\.md$` | `mode.ts:177–201` |
| Auto-approve | Global by tool class, not per mode | `auto-approval/index.ts` |

### cline

| Topic | Mechanism | Location |
| :--- | :--- | :--- |
| Modes | VS Code `plan` \| `act`; SDK also `yolo` \| `zen` | `storage/types.ts`, `runtime-config.ts` |
| Tools | `ToolPresets`: plan disables editor; act enables | `presets.ts:25–57` |
| Guard | Plan `beforeTool` command-guard blocks mutating shell | `command-guard-extension.ts` |
| Switch | `performRebuildSessionForMode` replaces runtime | `sdk-mode-coordinator.ts:197–267` |
| Patches | Act: editor executor writes disk; VS Code diff coordinator | `executors/editor.ts`, `sdk-diff-edit-coordinator.ts` |

## Steal / Reject for DevCode

| Steal | Reject |
| :--- | :--- |
| Roo `groups` + `isToolAllowedForMode` at **execution** (not prompt-only) | Separate FSM per profile |
| Group `fileRegex` for partial write (e.g. plan docs-only) | Full Cline dual `planMode*`/`actMode*` API matrix unless needed |
| Cline plan **command-guard** when bash remains available | |
| Rebuild or hard-refresh tool list + prompt + guards on `/plan` `/build` … | |

## Mapping

- **`plan` / `security`:** read tool groups + mutating-shell guard; security produces diffs — **who applies** still must be specified in Arch/PRD (open audit #2).
- **`build` / `debug`:** unlock edit groups; gates fire after batches (orthogonal to mode).
- Aligns with Arch tenet: modes are policy lenses, not control flow.

## Stop condition

Enough to implement profile → tool allowlist + `beforeTool` guards without further IDE-agent reading.
