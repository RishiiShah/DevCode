# Research Note: Sessions & TUI UX (Pass 5)

```
================================================================================
DOCUMENT ID    : DEVCODE-RES-005
REVISION       : 0.1.0
STATUS         : Working notes (reference study)
SOURCES        : references/opencode (2002026), references/pi (8c47efc)
RELATED        : docs/devcode.md §1 (CLI/TUI), docs/prd.md FR-1 / FR-3
================================================================================
```

## Question

How are sessions persisted/resumed, and what terminal UX should DevCode steal for diffs, slash commands, and checkpoints?

## Findings

### opencode

| Area | Mechanism | Location |
| :--- | :--- | :--- |
| Storage | SQLite under XDG data (`opencode.db`) | `packages/cli/src/database-path.ts`, `util/global.ts` |
| Resume | Durable event log + `resumeSuspendedSessions` | `specs/v2/session.md`, `session/execution/restart.ts` |
| Compaction | Rolling summary; full transcript durable; `/compact` | `specs/v2/session.md`, TUI session route |
| Slash | `/agents` `/models` `/variants` `/diff` `/undo` `/redo` `/fork` … | `packages/tui/src/app.tsx`, session `index.tsx` |
| Diff approve | Edit permission UI: diff + once / always / reject | `permission.tsx:169–237` |
| Routing | Central `Model` + provider map | `packages/core/src/model.ts`, `provider.ts` |

### pi

| Area | Mechanism | Location |
| :--- | :--- | :--- |
| Storage | `~/.pi/agent/sessions/--{cwd}--/{ts}_{id}.jsonl` | `session-manager.ts` |
| Resume | `/resume` → load JSONL / switch session | `interactive-mode.ts` |
| Compaction | `CompactionEntry` boundary; auto on threshold/overflow | `session-manager.ts`, `agent-session.ts` |
| Slash | `/model` `/thinking` `/compact` `/fork` `/clone` `/tree` `/trust` | `slash-commands.ts` |
| Diff | In-transcript render; p2 has scrollable consent preview | `tools/renderers/edit.ts`, `tui-trusted-ui.ts` |
| Routing | `ModelRegistry` / `ModelRuntime` | `model-registry.ts`, `model-runtime.ts` |

## Steal / Reject for DevCode

| Steal | Reject |
| :--- | :--- |
| Pre-apply edit permission with **diff + once/always/reject** (opencode) | Full opencode daemon + SQLite server requirement (unless DevCode runs one) |
| Slash profile/model switches (`/plan` `/build` … mirroring `/agents`) | Heavy p2 session-worker / Chord split |
| `/compact` + durable transcript with sliding window | Treating in-transcript diff render as a gate (pi coding-agent) |
| Session fork/tree for experiments; wire undo to **step-manifest**, not git | |

## Mapping

- Diff inspector in Arch diagram ≈ opencode permission reply flow **before** write finalization.
- Checkpoint cmds ≈ manifest step undo/redo, not aider-style git undo.
- Compaction belongs in Context Assembler (already named in Arch §1).

## Stop condition

Enough to sketch CLI slash surface and approval UX without more TUI spelunking.
