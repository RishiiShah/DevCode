# Research Note: Rollback Peers (Pass 8)

```
================================================================================
DOCUMENT ID    : DEVCODE-RES-008
REVISION       : 0.1.0
STATUS         : Working notes (reference study)
SOURCES        : references/cline, references/gemini-cli, references/kimi-code,
                 references/qwen-code, references/codex, references/pi
RELATED        : docs/prd.md FR-3, docs/devcode.md §5.1, TODO.md T1/T2
================================================================================
```

## Question

How do leading agent harnesses implement checkpointing and rollback? What can DevCode steal or reject relative to `ATE-1001-PRD` FR-3 (step-manifest rollback: pre-step byte persistence, per-step manifests, preserving staged index & untracked files, preflight conflict detection)? How do these peer findings feed the shell write policy (T2)?

## Method and limits

- Evidence labels: **code** (direct inspection of local clone), **prod data** (documented design / PR / issue), **guess** (inferred or unverified).
- Examined 6 peers:
  1. `cline` (`references/cline`)
  2. `gemini-cli` (`references/gemini-cli`)
  3. `kimi-code` (`references/kimi-code`)
  4. `qwen-code` (`references/qwen-code`)
  5. `codex` `git-utils` (`references/codex/codex-rs/git-utils`)
  6. `pi` `git-checkpoint.ts` (`references/pi/packages/coding-agent/examples/extensions/git-checkpoint.ts`)
- Not checked: performance benchmarks of large git trees under shadow repo `git add .`, behavior under submodules/symlink loops in each tool.

---

## Peer Findings

### 1. Cline

- **Locations**:
  - `sdk/packages/core/src/session/checkpoint-restore.ts`
  - `sdk/packages/core/src/session/checkpoint-diff.ts`
  - `apps/vscode/src/core/controller/checkpoints/checkpointRestore.ts`
  - `docs/core-workflows/checkpoints.mdx`
- **Mechanism**:
  - **VSCode**: Maintains a shadow Git repository in global storage per task. After each tool use (file edits, commands), Cline stages and commits file snapshots into this shadow repo.
  - **SDK**: Implements `beginWorktreeRestoreTransaction(cwd)` before running destructive restore:
    1. Uses `git stash push --include-untracked -m "cline restore transaction <uuid>"`.
    2. Points a private ref (`refs/cline/restore-transactions/<uuid>`) to `refs/stash`.
    3. Drops `stash@{0}` so the user's stash list is not visibly altered.
    4. On transaction commit: deletes the private ref.
    5. On transaction rollback: executes `git reset --hard HEAD`, `git clean -fd`, and `git stash apply --index <privateRef>`.
- **Verdict vs FR-3**:
  - **Violates FR-3.3 & FR-3.4**: Restoration runs `git clean -fd` and `git reset --hard`. If any untracked or uncommitted user file was not part of the initial stash or if the user added files outside the agent during execution, `git clean -fd` permanently deletes them.
  - **Steal**: The *restore transaction guard pattern* (`beginWorktreeRestoreTransaction`). Storing a safety ref before mutating the workspace ensures that if the restore operation itself fails midway, the pre-restore state can still be salvaged.
  - **Reject**: Relying on `git clean -fd` and full working tree resets.

### 2. Gemini CLI

- **Locations**:
  - `packages/core/src/services/gitService.ts`
  - `packages/core/src/utils/checkpointUtils.ts`
  - `docs/cli/checkpointing.md`
- **Mechanism**:
  - Operates a shadow Git repository under `~/.gemini/history/<project_hash>`.
  - Points `GIT_DIR` to the shadow repo while setting `GIT_WORK_TREE` to the active project root.
  - Generates dedicated, isolated `.gitconfig` and empty `.gitconfig_system_empty` files, setting `GIT_CONFIG_GLOBAL` and `GIT_CONFIG_SYSTEM` to strip user git hooks, aliases, and GPG signing requirements (`gpgsign = false`).
  - **Snapshot (`createFileSnapshot`)**: Runs `repo.add('.')`, checks `repo.status()`, and if dirty runs `repo.commit(message, {'--no-verify': null})`.
  - **Restore (`restoreProjectFromSnapshot`)**: Executes `repo.raw(['restore', '--source', commitHash, '.'])` followed by `repo.clean('f', ['-d'])`.
- **Verdict vs FR-3**:
  - **Violates FR-3.1 & FR-3.3**: Monolithic `git add .` snapshots the entire repository into git objects, ignoring file boundaries and capturing unrelated user edits. On restore, `git restore .` reverts every file to the snapshot, and `git clean -fd` obliterates newly added user untracked files.
  - **Steal**: Environment sanitization for git subshells (`GIT_CONFIG_GLOBAL`, `GIT_CONFIG_SYSTEM`, unsetting `GIT_DIR`/`GIT_WORK_TREE` leaks).
  - **Reject**: Shadow git repo whole-tree commits (`add .`), blanket `git clean -fd`.

### 3. Kimi Code

- **Locations**:
  - `packages/agent-core-v2/src/agent/undo/undoService.ts`
  - `packages/agent-core-v2/src/agent/undo/turnUndo.ts`
  - `packages/agent-core-v2/src/agent/contextMemory/conversationUndoParticipants.ts`
- **Mechanism**:
  - Kimi's undo system is strictly an in-memory conversation context and wire-record undo mechanism (`cutTurnUndo`, `isUndoAnchorOrigin`).
  - Tracks user prompt anchors and turn records, allowing rewinding of conversation history and token compaction windows.
  - Provides a participant registry (`IAgentConversationUndoParticipantRegistry`) where subservices implement `reconcileAfterUndo()`.
  - **Workspace State**: Explicitly *does not* snapshot or roll back filesystem modifications.
- **Verdict vs FR-3**:
  - **Violates FR-3.1–3.4**: Does not provide filesystem rollback. Rewinding conversation without rolling back disk changes creates an immediate "split-brain" state where the model forgets what it wrote, but the mutated code remains on disk.
  - **Steal**: Clean participant lifecycle design (`reconcileAfterUndo`) for updating internal harness state after a rollback occurs.
  - **Reject**: Context-only undo without workspace synchronization.

### 4. Qwen Code

- **Locations**:
  - `packages/core/src/managed-runtime/managed-harness-checkpoint.ts`
  - `packages/core/src/managed-runtime/original-receipt-checkpoint.ts`
  - `packages/cli/src/serve/daemon-git-worktree-guard.ts`
- **Mechanism**:
  - **Checkpointing**: Implements a formal state-machine journal checkpoint (`HarnessCheckpointV1`) with monotonic sequence numbers, action receipts, tool publication bindings, and SHA-256 event digests. Primarily aimed at distributed durable execution and idempotency.
  - **Worktree Guard & Shell Safety**:
    - Implements lexical shell parsing in `daemon-git-worktree-guard.ts`.
    - `stripHeredocBodies`: Strips `<<[-]WORD ... WORD` bodies from commands before splitting into sub-commands. Prevents embedded scripts inside heredocs from masquerading as top-level commands or evading directory escape checks.
    - Tracks directory changes (`cd`) across compound shell pipelines to maintain an accurate view of working directory and prevent worktree escapes.
- **Verdict vs FR-3**:
  - **Harness Checkpointing**: Focuses on event stream receipts rather than filesystem diff rollback.
  - **Steal**: The lexical parser technique in `stripHeredocBodies` and separator parsing. Essential for T2 shell write policy to prevent models from bypassing shell guards via heredocs or multiline quoting.
  - **Reject**: Protocol-only checkpointing that omits physical file rollback.

### 5. Codex `git-utils`

- **Locations**:
  - `codex-rs/git-utils/src/baseline.rs`
  - `codex-rs/git-utils/src/apply.rs`
  - `codex-rs/git-utils/src/operations.rs`
- **Mechanism**:
  - **Baseline (`baseline.rs`)**: Uses pure-Rust `gix` (gitoxide) to build an in-memory git baseline to compute structured diffs (`GitBaselineDiff`, `GitBaselineChangeStatus`) without modifying the user's primary git repository.
  - **Patch Application (`apply.rs`)**:
    - Executes `git apply --3way`.
    - Supports `preflight` dry-run mode via `git apply --check`, returning affected paths, skipped paths, and conflicts before any disk modification occurs.
    - In revert mode (`req.revert`), if not preflight, it runs `stage_paths(&git_root, &req.diff)` before calling `git apply --3way -R`.
- **Verdict vs FR-3**:
  - **Violates FR-3.3**: Calling `stage_paths` on revert explicitly mutates the user's git index/staging area to avoid 3-way merge conflicts. FR-3.3 strictly forbids modifying the staged index.
  - **Steal**: Dry-run preflight check (`git apply --check` concept translated to step-manifest preflight in FR-3.4); using library-level git parsing (`gix`) rather than raw CLI invocations for repository inspection.
  - **Reject**: Staging workspace paths into the user's index during revert.

### 6. Pi `git-checkpoint.ts`

- **Locations**:
  - `packages/coding-agent/examples/extensions/git-checkpoint.ts`
- **Mechanism**:
  - Minimal coding-agent extension attached to `turn_start` and `session_before_fork`.
  - On `turn_start`: runs `git stash create` and records the resulting dangling commit hash against the current session turn ID.
  - On fork/restore: runs `git stash apply <ref>`.
  - On `agent_settled`: clears in-memory checkpoint map.
- **Verdict vs FR-3**:
  - **Violates FR-3.1–3.4**: `git stash create` without `-u` / `--include-untracked` completely ignores untracked files. `git stash apply` fails and leaves merge conflicts if the working tree has concurrent modifications. Relies entirely on git existence.
  - **Steal**: Clean, non-intrusive hook triggers (`turn_start`, `agent_settled`) for initiating snapshot and teardown cycles.
  - **Reject**: Naive `git stash create` / `git stash apply` as a rollback mechanism.

---

## Steal / Reject Summary vs FR-3

| Peer | Mechanism | Steal | Reject |
| :--- | :--- | :--- | :--- |
| **cline** | Shadow git repo + stash transaction guard | Transaction guard before destructive restore (private ref safety net) | `git clean -fd` and `reset --hard` (destroys untracked & dirty user files) |
| **gemini-cli** | Shadow repo with isolated gitconfig; `add .` | Isolated `.gitconfig` and empty system config (`GIT_CONFIG_*`) | Monolithic `git add .`, whole-repo `git restore .` + `git clean -fd` |
| **kimi-code** | Wire-protocol / memory turn undo | Participant lifecycle hook registry (`reconcileAfterUndo`) | Memory-only undo without workspace/file rollback (split-brain) |
| **qwen-code** | Journal receipt sequences + worktree guard | Lexical `stripHeredocBodies` and cwd tracking for shell commands | Protocol-only receipts without file byte capture |
| **codex** | `gix` baseline + `git apply --3way` | Dry-run preflight validation prior to applying changes; `gix` usage | `stage_paths` (mutating staged index during revert) |
| **pi** | `git stash create` turn extension | Hook event lifecycle (`turn_start` pre-step capture trigger) | `git stash create` (omits untracked files; conflicts on apply) |

---

## Architecture Input Feeding T2 (Shell Write Policy)

DevCode's PRD FR-3.2 currently states:
> "Capture subprocess writes too, or block commands whose mutations cannot be captured."

### The Problem
On Linux, tools like `ptrace`, `fanotify`, or `overlayfs`/`bwrap` can intercept or sandbox filesystem writes. However, on **macOS and Windows** (which are Tier 1 platforms under FR-6.1):
1. There is no unprivileged kernel filesystem redirection or namespace sandboxing (arch §6.1–6.2).
2. Any attempt to strictly "capture or block" arbitrary subprocess writes means DevCode would have to **block common build, compile, and test commands** (e.g. `cargo build`, `npm test`, `pytest`, `go build`, `tsc`), because these commands routinely write to `.build`, `target/`, `node_modules/`, `__pycache__/`, or temp directories without going through harness file tools.
3. If DevCode freezes/blocks these commands, the agent cannot build or run tests—rendering the harness unusable on macOS and Windows.

### Lessons from Peers for T2
1. **Differentiate Tool Edits vs. Command Subprocesses**:
   - Agent file tools (`write_file`, `edit_file`, `replace_lines`) MUST strictly capture pre-step bytes, permissions, and hashes in the step manifest before writing (FR-3.1).
   - Shell commands (`bash`, `sh`, `cmd`, `powershell`) cannot be captured via pre-step byte snapshotting because the modified paths are unknown ahead of execution.
2. **Lexical Guarding & Shell Restrictions (from Qwen Code PR #9417 & worktree guard)**:
   - Use lexical command analysis (`stripHeredocBodies`, separator parsing) to prevent shell redirection (`>`, `>>`) from modifying protected harness areas (e.g., `.devcode/`).
3. **Workspace Allowlisting & Ephemeral Exclusions**:
   - Shell build commands must be allowed to write to designated build artifacts and temp paths, but monitored or restricted via working directory boundaries.
   - Post-command git status / change detection can record touched paths without blocking builds.
