# [ATE-1000] System Architecture Specification: DevCode Harness

```
================================================================================
STANDARD       : ATE-1000 (Automated Technical Engineering Standard Series)
DOCUMENT ID    : ATE-1000-ARCH
REVISION       : 0.4.4-draft
STATUS         : Working Draft (Engineering Specification)
AUTHOR         : DevCode Architecture Group
OWNS           : Design mechanisms (how). Requirements, NFRs, gates: ATE-1001-PRD
SECTIONS       : 3 profiles | 4 rollback | 5 gates + audit | 6 runtime, shell,
                 install | 7 loop + config | 8 non-goals
RELATED DOCS   : [docs/prd.md](prd.md), [docs/codebaseunderstanding.md](codebaseunderstanding.md)
================================================================================
```

---

## 1.0 System Overview & Engineering Philosophy

**DevCode** is a developer-centric agentic CLI harness: a **single agent loop** with **dynamic policy profiles**, **deterministic verification gates**, and **transparent security boundaries**. Product vision and personas: PRD §1.0–2.0.

Components: CLI/TUI (diff inspector, profile switch, rollback) over one agent core (context assembler per ATE-1002, compaction), bounded by the runtime guard (§6) and the gatekeeper (§5).

### Core Architecture Tenets
1. **Modes as Policy & Permission Profiles, Not Control Flow**: Modes (`ask`, `plan`, `build`, `debug`, `security`) are lenses that alter prompting emphasis and tool permissions. The execution engine remains a single fluid loop.
2. **The Differentiator is the Gate**: Anyone can generate code; DevCode blocks harness finalization until required checks pass or the user explicitly authorizes an audited override. Edits remain provisional on disk during verification; external Git commands are outside this gate.
3. **No Self-Grading Gates**: The model cannot classify its own edits as "exempt" to bypass testing. File classification is evaluated by deterministic path rules and AST heuristics.
4. **Honest Engineering Boundaries**: No claims of magical "air-tight process isolation" on platforms that do not support it. Boundaries are defined as strict path allowlists, credential stripping, process timeouts, and opt-in Linux namespaces.
5. **Zero Mutation of User State**: Checkpoints rely on **Step Manifests and selective working-tree restores**, preserving pre-step content even in touched files and never modifying the developer's staged index. Concurrent edits cause rollback conflicts rather than silent overwrites.

---

## 2.0 Subsystem Decomposition

| Subsystem | Architectural Responsibility |
| :--- | :--- |
| 1. Profile Controller | Injects mode-specific system prompts, tool allowlists, and gate rules (§3). |
| 2. Context Engine | Dual-Tier Graph: JIT ripgrep plus threshold-gated AST PageRank (ATE-1002). |
| 3. Step Manifest Engine | Tracks file mutations (created, modified, deleted) for safe rollback (§4). |
| 4. Runtime Guard | Enforces FS allowlists, credential env stripping, and tree timeouts (§6). |
| 5. Gatekeeper & Auditing | Evaluates file classifications, runs linters/tests, logs overrides (§5). |
| 6. Configuration Loader | Parses `.devcode.json` for build/test/lint commands (§7.4). |

---

## 3.0 Modes as UX & Permission Profiles

Modes are **behavioral profiles** governing tool permissions and verification rigor, not a forced state graph.

| Mode Profile | FS Permissions | Network Access | Enforced Verification Gate |
| :--- | :--- | :--- | :--- |
| `ask` | Strictly Read-Only | Unrestricted (env stripped) | None (answers only) |
| `plan` | Strictly Read-Only | Unrestricted (env stripped) | Structured task specification output |
| `build` (default) | Scoped Read/Write | Unrestricted (env stripped)* | Typecheck + Linter + Test Gate (TDD) |
| `debug` | Scoped Read/Write | Unrestricted (env stripped)* | Minimal Reproducer Test (Red -> Green) |
| `security` | Read-Only (agent)* | Unrestricted (local DBs cached) | Secret scan (Gitleaks) + SAST (Semgrep) |

*On Linux, network can be unshared via `bwrap --unshare-net` if enabled in `.devcode.json`. On macOS and Windows, socket creation is unrestrained at the OS level; credentials and tokens are stripped from the process environment. In `security`, remediation patches are applied by the harness on explicit user approval, never by the agent.*

### 3.1 Profile Definitions
- **`ask`**: Question-answering over the codebase with the same read-only tools as `plan` (`Glob`, `Grep`, `FileRead`, `RepoMap`). Write and mutating shell tools are disabled at the tool schema level. Produces no specification and runs no gate. If the user asks for a change, the agent answers and names the profile to switch to (`/build`, `/debug`, `/plan`); it never switches itself.
- **`plan`**: Maximizes read-only exploration (`Glob`, `Grep`, `FileRead`, `RepoMap`). Write and shell mutation tools are disabled at the tool schema level.
- **`build`**: Unlocks surgical file edits. Triggers the Verification Gate on completion of any file batch.
- **`debug`**: Focuses prompt attention on hypothesis testing and minimal reproduction scripts before modifying target code.
- **`security`**: Orchestrates local scanners (Gitleaks, Semgrep, osv-scanner; ATE-1002 §5.0) with read-only filesystem access. File-modification and mutating shell tools are disabled at the tool schema level. Findings produce remediation diffs as structured unified patches. The harness (not the agent) shows the diff, and on explicit acceptance applies it under step-manifest capture and runs the security gate. Complex changes or test additions need `/build`.

---

## 4.0 Step Manifest Checkpointing & Safe Rollback

DevCode preserves staged, unstaged, and untracked work independently of Git history with a **manifest-driven snapshot engine**. Lifecycle of one step:
1. Before the first mutation, persist pre-step state and publish the manifest. `HEAD` is recorded as diagnostic metadata only. Index and working tree stay untouched.
2. The agent edits; the harness records post-step state in `<session_id>/<step_id>.json` (`created`, `modified`, `deleted` path lists).
3. Checks pass and the user accepts: changes stay. Checks fail or the user rejects: run the rollback in §4.1.

### 4.1 Rollback Algorithm
Before the first mutation of each path in a batch, persist its existence, file type, exact bytes (or symlink target), permission bits, and content hash under the session checkpoint. A hash alone is not a recoverable backup. Publish the manifest before allowing the mutation; if capture fails, block the write.

When a rollback is triggered (via `devcode rollback` or gate failure rejection):
1. **Conflict Preflight**: Compare every affected path with its recorded post-step state, including existence, type, content, and permissions. If any path differs, stop before restoring any paths, retain the checkpoint, and report the conflict. Do not overwrite or delete concurrent user edits. A batch with unknown writes or incomplete post-step records requires recovery review rather than automatic rollback.
2. **Created Files**: Remove a path only if it was absent before the step and still matches the recorded agent-created state. Existing untracked files are modified files, not created files.
3. **Modified and Deleted Files**: Restore exact pre-step bytes, file type, symlink target, and permissions from persisted records. Never restore from `HEAD` or the staged index. Treat a rename as a captured deletion plus creation.
4. **Guarantee**: Successful rollback restores touched paths to pre-step state, preserves unrelated paths, and leaves the staged index untouched. Restore steps in reverse order; older steps cannot be restored over later changes.
5. **Recovery Failure**: Revalidate paths against the workspace boundary before restoration, do not follow symlinks outside it, and retain checkpoint data after any I/O failure. Report partial recovery explicitly; never report success until restored state is verified.

Harness file tools (`write_file`, `edit_file`, `patch`) capture every mutation before write. Shell writes follow §6.4: every mutation-capable command runs after a guard-set snapshot (§6.4.5), so shell changes to non-ignored workspace files are restorable. Gitignored files, oversize files, and paths outside the workspace are not; do not advertise rollback for them.

### 4.2 Checkpoint Retention & Garbage Collection
- Manifests live under `<state>/manifests/<session_id>/` and blobs under `<state>/blobs/` (§4.3).
- **Retention**: last 20 completed steps per session plus active or unresolved recovery steps.
- **Expiration**: completed, inactive manifests older than 7 days are pruned on session initialization.
- **Manual cleanup**: `devcode gc` purges eligible stale manifests and unreferenced blobs. Never evict a required backup to meet the disk budget; block new mutations if a checkpoint cannot be stored safely.

---

### 4.3 Harness State Location
- `<state>` = `$DEVCODE_STATE_DIR`, else `$XDG_DATA_HOME/devcode`, else `~/.local/share/devcode`, then `projects/<folder>-<sha256(canonical root)[..12]>/`. No home directory: `<root>/.devcode/`.
- Holds `blobs/`, `manifests/`, `recovery/`, `recovered/`, `managed/`, `audit.log`, `trace.jsonl`, `conversations/` (saved conversations for `--continue`, `--resume`, `/resume`).
- Outside the workspace so a shell command (`rm -rf .*`, `find -delete`) cannot delete the checkpoints rollback needs. Peers: gemini-cli `~/.gemini/history/<project>`, Codex `~/.codex/sessions`.
- The `.devcode/` deny rules (§5.2, §6.4) stay for the fallback location and older workspaces. A missing blob loses that path's diff and rollback only; the run continues.

## 5.0 Deterministic Gatekeeper & Classification Protocol

To prevent the **"Gate Grades Itself" vulnerability** (the generating model labels its own breaking code "Class C: Docs/Exempt"), classification is enforced by a **rule-based path and AST classifier**. Flow: classify the batch (A, B, or C per §5.1) → Class A needs an added or updated behavior assertion plus required tests, builds, and validators → missing assertion blocks finalization unless the user explicitly overrides, which is audited (§5.2) while remaining gates still run. The acceptance flow is PRD §5.0.

### 5.1 Classification Rules
1. **Class A (Behavior / API / Configuration)**: Any change affecting executable behavior, interfaces, dependencies, compiler settings, build scripts, CI workflows, permissions, deployment, or verification policy requires a new or updated assertion of the changed behavior plus applicable tests, build checks, and validators. Configuration assertions may be fixture checks or dry-run validation; a parser/schema check alone does not establish behavior.
2. **Class B (Invariance/Refactor, declared empty in v1)**: No transformation qualifies automatically. Without deterministic AST equivalence provers, treating agent-claimed "refactors" as exempt creates a self-grading loophole, so in v1 all code changes default to Class A unless they meet Class C. Class B is reserved for releases backed by formal AST verification (Tree-sitter verified type-annotation additions with zero runtime emit diff, LSP-verified local renames, deterministic import sorting). Developers performing behavior-preserving refactors in v1 must touch or update an existing assertion, or authorize an audited bypass via `--skip-tests`.
3. **Class C (Inert Content)**: Only verified documentation, plain text, or comment/whitespace-only changes with no executable or policy effect qualify for format/lint-only checks. File extensions and `docs/**` placement never establish exemption by themselves. Executable examples, ignore rules, and configuration embedded in documentation require impact classification.
4. **Conservative Default**: Unknown formats, unsupported parsers, parse failures, mixed changes, and ambiguous impact default to Class A. Apply the strictest applicable class to the batch. Missing validators block finalization unless explicitly bypassed by the user and audited.
5. **Trusted Policy Baseline**: Load gate commands and classification policy from the user-approved pre-step configuration. Changes to `.devcode.json`, test discovery, CI policy, or gate scripts cannot weaken their own checks. Validate proposed policy changes against the baseline; activate them only after explicit user acceptance. Model text and repository content cannot authorize an override.
6. **Bound Verification Results**: Record the checked diff hash, commands, and outcomes. Any later source, configuration, or test change invalidates affected results. A new test file alone is insufficient: the configured runner must discover and execute the behavior assertion successfully.

### 5.2 Append-Only, Tamper-Evident Audit Log
Every bypass via `--skip-tests` or an explicit user instruction identifying the bypass appends a non-interactive record to `<state>/audit.log` (§4.3):
```json
{
  "timestamp": "2026-11-23T15:25:00Z",
  "action": "OVERRIDE_SKIP_TESTS",
  "actor": "user_cli_flag",
  "modified_files": ["src/payment/processor.ts"],
  "diff_sha": "a1b2c3d4...",
  "prev_entry_sha": "9f8e7d6c...",
  "reason": "Hotfix emergency bypass"
}
```
**Harness State Protection & Truncation Anchor**: `.devcode/` (`audit.log`, `manifests/`) is written only by the harness process. The runtime guard (§6) denies every agent file-tool write, edit, delete, and rename under `.devcode/` in all profiles, including via symlinks resolved by `realpath`. Each entry carries `prev_entry_sha`, the SHA-256 of the preceding entry.

To stop a shell subprocess or adversary from truncating or deleting the log to erase past bypasses, the tail hash is anchored in two places outside the agent-writable tree:
1. **User Local State Anchor**: `~/.local/state/devcode/anchors/<repo_uuid>.json` on Linux/macOS, `%LOCALAPPDATA%\devcode\anchors\<repo_uuid>.json` on Windows. Records `{"repo_uuid": "...", "entry_count": N, "last_entry_sha": "...", "updated_at": "..."}`.
2. **Private Git Ref Anchor**: In a Git workspace, the harness writes the latest tail SHA and count to `refs/devcode/audit-tail`, outside the working tree and index and never touched by agent file tools.

**Tamper Detection** at session startup and before finalizing any gate:
- Log absent or empty while the anchor records `entry_count > 0`: block finalization (`AUDIT_LOG_DELETED`).
- Entries in `audit.log` fewer than `entry_count`: block (`AUDIT_LOG_TRUNCATED`).
- Any `prev_entry_sha` mismatching the previous record's SHA-256, or the final record mismatching `last_entry_sha`: block (`AUDIT_CHAIN_BROKEN`).

On macOS and Windows there is no OS-level filesystem isolation (§6.1), so a shell subprocess could still modify `.devcode/`. The guarantee is tamper-evident, not tamper-proof; tampering or deletion prevents further automated completions. Shell write policy: §6.4.

---

## 6.0 Pragmatic Runtime Boundaries & Threat Model

Every tool invocation (`run_command`, `write_file`, ...) passes three stages in order:
1. **Path canonicalization (`realpath`)**: all writes stay inside the workspace root; symlink escapes and paths such as `~/.ssh` are blocked.
2. **Policy Engine & Approval Bus**: authoritative control is profile policy plus user approvals. A command blacklist (`rm -rf /`, `dd`, `shutdown`, `mkfs`) is an advisory UX rail only (§6.1).
3. **Subprocess execution**: environment stripped (`AWS_*`, `GITHUB_TOKEN`, `*_API_KEY`, `SSH_*`), dedicated process tree, wall-clock watchdog (default 30 s), abort kills the whole tree (SIGKILL or Job Object).

### 6.1 Honest Security Claims
- **Network**: DevCode does **not** claim kernel-level network isolation on macOS or Windows. Child processes can open sockets. Protection is **environment stripping** of API tokens and cloud credentials. On Linux, `bwrap --unshare-net` can be enabled if `bubblewrap` is installed.
- **Command Blacklist as UX Safety Rail, Not Hard Security**: Pattern matching is trivially bypassed via string concatenation, base64, script evaluation, subshells, or aliases. The true controls are profile permissions (mutating tools disabled in `ask`/`plan`/`security`), `realpath` canonicalization, environment stripping, and the Policy Engine & Approval Bus.
- **Threat Model**:
  - *Addressed*: accidental destructive commands (`rm -rf`), runaway CPU processes, directory traversal outside the workspace, accidental leakage of local environment tokens into child processes.
  - *Not addressed*: zero-day kernel exploits, hostile root users, hardware side channels. DevCode is a developer tool, not an untrusted multi-tenant container.

### 6.2 Cross-Platform Runtime Matrix
Tier 1 targets: macOS (arm64, x86_64), Linux (x86_64, aarch64; glibc and musl), Windows 11 native (x86_64). Windows arm64 runs the x86_64 build under emulation until a native build is justified. WSL2 is treated as Linux. Every OS-specific behavior sits behind one platform abstraction; the agent loop, gates, and classifier contain no OS branches.

| Concern | macOS / Linux | Windows | Spec rule |
| :--- | :--- | :--- | :--- |
| Process tree kill | `setpgid` + `killpg` (SIGTERM, then SIGKILL) | Unprivileged Win32 Job Object with `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE` | FR-4.3 / NFR-8 hold on every Tier 1 OS without administrator elevation |
| Path canonicalization | `realpath`; APFS is case-insensitive by default and normalizes Unicode | Normalize shell drive paths (`/c/` -> `C:\`), resolve to the final long path (8.3 names, junctions, symlinks) per §6.4.4 and reject reserved device names (`CON`, `NUL`) | Compare platform-normalized canonical paths. The `.devcode/` deny rule and the workspace allowlist use the same normalization |
| Symlinks | Native | Creation may need privilege; junctions differ | Manifest records link type and target. Restore failure is reported per FR-3.5, never skipped silently |
| Permission capture | POSIX mode bits | Read-only and hidden attributes; ACLs not captured | NFR-6 parity covers bytes, type, and POSIX mode or Windows attributes. ACL fidelity is out of scope and documented |
| Line endings & BOM | Bytes preserved; preserve file EOL | Bytes preserved; preserve UTF-8/UTF-16 BOM and CRLF | Manifests store raw bytes; file tools preserve existing BOM and CRLF |
| File replace | Atomic `rename` | Replace retries with backoff on locked files (`ERROR_SHARING_VIOLATION`) | Locked-file failure blocks the write or reports partial rollback per FR-3.5; never overwritten blind |
| Env stripping | Case-sensitive names | Case-insensitive names | Match credential patterns case-insensitively on Windows |
| Command execution | `sh -c` or argv | PowerShell or `cmd`; argv preferred | Prefer argv spawn over shell strings on every OS |
| Network isolation | Linux: opt-in `bwrap`; macOS: none | None | Unchanged (§6.1) |
| Config and cache | XDG dirs; `~/Library` on macOS | `%APPDATA%` / `%LOCALAPPDATA%` | Per-OS user dirs; harness state in the per-project state dir (§4.3) |

Open items: native Windows availability of `semgrep`, `gitleaks`, `osv-scanner` is unverified (§7.3 missing-tool policy applies until it is). Windows runtime detail: `DEVCODE-RES-009`.

### 6.3 Distribution & Installation
Primary channel is a one-line installer; Homebrew, winget, npm, and source build are deferred.

```sh
curl -fsSL https://<install-host>/install.sh | sh        # macOS, Linux, WSL2
irm https://<install-host>/install.ps1 | iex             # Windows (PowerShell)
```

Hosting and release integrity:
- **Beta (current decision)**: `<install-host>` is `<project>.pages.dev` on **Cloudflare Pages**; no purchased domain and no GitHub organization. Cloudflare is chosen over Vercel because Vercel Hobby is restricted to non-commercial personal use, while Cloudflare Pages documents unlimited static requests and bandwidth on the free plan with a 20,000-file limit [vendor claim, checked 2026-11-24]. The site serves only `install.sh`, `install.ps1`, and the public key fingerprint. Release archives, `SHA256SUMS`, and `SHA256SUMS.minisig` stay in GitHub Releases of a repository on the maintainer's personal account (`https://github.com/<github-user>/<repo>/releases/download/v<version>/...`). Binaries are never hosted on the site [guess: keeps it within free-tier limits].
- **Subdomain names are first come**: as of 2026-11-23, `devcode.pages.dev`, `devcode.vercel.app`, `assay.pages.dev`, and `assay.vercel.app` were in use. A name is claimed only when a project is created; reserve the subdomain before documenting it.
- **After beta**: a custom domain may point at the same site. Do not document a live `curl | sh` URL for a host DevCode does not control. As of 2026-11-23, `devcode.tools` had no DNS answer and the GitHub organizations `devcode` and `devcode-ai` belong to unrelated accounts.
- **Account risk**: whoever controls the site controls the installer script and any key embedded in it, so the pinned minisign key protects against a tampered release mirror, not against site or account compromise. Beta mitigations: hardware-key 2FA on both accounts, signed tags, minisign secret key kept off CI runners, script served from a repository-built deploy only (no dashboard edits), key fingerprint published in the README and in a second place.
- **Signing Standard**: Releases publish `SHA256SUMS` signed via **minisign** (`SHA256SUMS.minisig`) with a pinned Ed25519 public key embedded in `install.sh` and `install.ps1`. CI also produces Sigstore/cosign provenance attestations; no SLSA level is claimed until the pipeline is audited against it.

Requirements:
1. **Rust, self-contained binary.** One native binary per target; no Node, Python, or other runtime on the user machine. Rust gives direct access to Win32 Job Objects and POSIX process groups. Startup (NFR-1/2) is a benchmark outcome, not a property of the language. Library choices (for example `gix`, `tree-sitter`) are undecided; `tree-sitter` is a C library with Rust bindings.
2. **POSIX `sh` only.** Runs under `dash`, `bash`, and `zsh` when piped to `sh`; no bashisms. Wrap the body in a function invoked on the last line so a truncated download cannot execute a partial script.
3. **Detection.** OS, CPU architecture, and libc (glibc vs musl, including Alpine and Termux). Handle Rosetta 2 and Windows-on-ARM shells that report x86_64.
4. **Integrity & verification.** Download the asset, `SHA256SUMS`, and `SHA256SUMS.minisig`; verify the checksum and the signature with the embedded key before extracting. Abort and delete all artifacts on mismatch.
5. **No privilege.** Install to a user-writable directory (default `~/.local/bin`; `%LOCALAPPDATA%\Programs\devcode` on Windows); never call `sudo`.
6. **Idempotent and reversible.** Re-running upgrades in place. `--version <v>` pins a release. `--no-modify-path` skips shell-profile edits; otherwise append to the profile once and only when the directory is absent from `PATH`. `devcode uninstall` removes the binary and PATH edit and never touches `.devcode/` in any repository.
7. **Hygiene.** Temp files live in a `mktemp` directory removed on exit and failure. The installer executes no downloaded code other than the verified binary's `--version` smoke test, and sends no telemetry.
8. **Upgrade path.** `devcode upgrade` reuses the same download and verification code.

Reference evidence: codex ships `install.sh`/`install.ps1` with a `SHA256SUMS` check; goose ships download scripts with no checksum and bash-only syntax; opencode's installer handles Rosetta, musl, and AVX2 baseline builds.

### 6.4 Shell Write Policy & Subprocess Mutation Model

#### 1. The Core Tension
Step-manifest rollback (PRD FR-3) needs pre-step bytes, permissions, and hashes before a mutation. For harness file tools (`write_file`, `edit_file`, `replace`) this capture is exact and synchronous. For shell subprocesses (`run_command`) the written paths are unknown ahead of time, and per-path capture is impossible without kernel filesystem redirection (overlayfs, fanotify). §6.4.5 closes the gap by snapshotting the whole guard set before the command. On macOS and Windows, blocking all uncapturable shell writes would **freeze build and test tools** (`cargo build`, `npm test`, `pytest`, `go test`, `dotnet build`, `make`), which routinely emit artifacts, caches, and temp files.

#### 2. Separation of Tools
1. **Agent file tools (sanctioned mutation path)**: Agent changes to source, tests, configs (`.devcode.json`, CI scripts, lockfiles), and documentation MUST use harness file tools. The guard canonicalizes paths (`realpath`), checks workspace boundaries, excludes `.devcode/`, and persists pre-step bytes in `.devcode/manifests/<session_id>/<step_id>.json` before the edit.
2. **Shell subprocesses (`run_command`)**: Authorized for reading, compiling, building, and running test suites. May write derived build directories (`target/`, `build/`, `dist/`, `.cache/`, `node_modules/`, `__pycache__/`) and system temp locations (`/tmp`, `%TEMP%`). Monitored so the shell cannot bypass source tracking or harness state protection.

#### 3. Lexical Guarding against Shell Evasion
1. **Heredoc stripping**: Following Qwen Code PR #9417 / `daemon-git-worktree-guard.ts`, heredocs (`<<[-]WORD ... WORD`) are stripped before lexical analysis so script bodies or delimiter text cannot mask redirection operators or command chains.
2. **Redirection blocking**: POSIX operators (`>`, `>>`, `&>`, `>|`) targeting `.devcode/` or tracked source files are blocked before spawn. Direct writers (`cat <<EOF >`, `tee`, `cp`, `mv`) targeting protected paths or source files are denied; the agent is told to use file tools.
3. **Canonicalization**: Redirection targets are canonicalized against the workspace root to defeat symlink traversal and `../` tricks.

#### 4. Windows-Specific Shell Write Policy
On Windows 11 native (x86_64):
1. **Shell variant coverage**: PowerShell (`pwsh.exe`, `powershell.exe`) and `cmd.exe` shapes are parsed.
   - PowerShell redirection (`>`, `>>`, `2>&1`, `*>&1`) and cmdlets (`Out-File`, `Set-Content`, `Add-Content`, `Tee-Object`, `Export-Clixml`) targeting `.devcode/` or tracked sources are blocked.
   - PowerShell heredocs/herestrings (`@" ... "@`, `@' ... '@`) are stripped before parsing cmdlet pipelines.
2. **Case insensitivity & short names**:
   - `.devcode/` checks match case-insensitively (`.DEVCODE`, `.DevCode`).
   - **Canonical form**: resolve with `GetFinalPathNameByHandle` (flag `VOLUME_NAME_DOS`), which expands 8.3 short names (`DEVCOD~1`), junctions, and symlinks. Strip the `\\?\` prefix from `\\?\C:\...`, and rewrite `\\?\UNC\server\share\...` to `\\server\share\...`. Canonical forms are compared only after this normalization, so the prefix is never a reason to reject a path.
   - **Targets that do not exist yet**: canonicalize the nearest existing parent the same way, check the rule, then append the validated final component.
   - **Containment**: canonicalize the workspace root with the same function. A path is allowed only if the canonical root is a case-insensitive path-component prefix of the canonical target. The `.devcode/` deny rule uses the same comparison.
   - **Rejected**: Win32 device names (`CON`, `PRN`, `AUX`, `NUL`, `COM1..9`, `LPT1..9`), device-namespace paths (`\\.\`, `\\?\GLOBALROOT`, `\\?\Volume{...}` not resolving to a drive letter), and any network UNC path unless the canonical workspace root is itself on that share.
3. **Locked files**: Windows enforces mandatory locking (`ERROR_SHARING_VIOLATION` / `ERROR_LOCK_VIOLATION`). Rollback retries with exponential backoff (up to 1.5 s) and reports partial recovery per FR-3.5 if the file stays locked.
4. **Process tree eradication**: Win32 Job Objects with `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE`. When the 30 s watchdog trips or a command aborts, closing the handle kills all descendants without POSIX signals.

#### 5. Pre-Command Workspace Snapshot & Post-Command Reconciliation
Pre-step capture of an unknown write set is solved by capturing the whole **guard set** before any command that can write. This replaces an earlier `git status` check, which fails in non-git repositories, cannot tell whether a command changed an already-dirty file, and restores nothing.

**Command classes** (argv-parsed, then lexical analysis for shell strings):
1. **Read-only allowlist** (`ls`, `cat`, `rg`, `git status`, `git diff`, `git log`): no snapshot. Any redirection, unknown flag, or pipe into a non-allowlisted command moves it to class 2.
2. **Mutation-capable** (everything else, including build and test): snapshot before, reconcile after.
3. **Destructive patterns** (`rm`, `mv`, `sed -i`, `git clean`, `git checkout --`, `git restore`, `git reset --hard`, `git stash`): always routed to the Policy Engine as ASK, whatever the profile or prior approvals. The prompt states whether the target set lies inside the guard set.

**Guard set**: every non-ignored file in the workspace root, tracked or untracked, excluding `.devcode/`. Ignore rules come from `.gitignore` (and nested ignore files); without Git, from the derived-directory list in §6.4.2. Per-file cap 10 MB; larger files are outside the guard set and reported as unprotected.

**Snapshot procedure**:
1. Fingerprint each guard-set path: type, size, mtime (ns), mode or Windows attributes.
2. Hash and store bytes only for paths whose fingerprint differs from the session's last snapshot. The first snapshot reads every file; later ones are incremental. Blobs are content-addressed under `<state>/blobs/` (§4.3) and shared with step manifests, so disk use stays under the NFR-7 ceiling.
3. Publish the pre-step record in the step manifest before spawning the command (FR-3.1 applies unchanged).

**Budget**: at most 5,000 guard-set files and unique blob bytes within the NFR-7 ceiling (shared with manifests). Both numbers are untested starting values [guess]; NFR-9 sets the time limit. If the budget is exceeded or a file cannot be read (for example a Windows sharing violation), the command is blocked (FR-3.5). The user may approve running it **without rollback protection**; the harness appends `OVERRIDE_NO_SNAPSHOT` to the audit log (§5.2) listing the unprotected paths, and the step manifest marks the step `rollback: unavailable`.

**Post-command reconciliation**:
1. Re-fingerprint the guard set; hash only changed paths. Record created, modified, and deleted paths as the step's post-state.
2. If any tracked or guard-set source or configuration file changed: invalidate all prior verification results (PRD Gates 1–5), assign the batch to Class A, and re-run gates.
3. Rollback restores shell-changed paths from the stored blobs under the FR-3.4 preflight (post-state comparison, reverse order, never `HEAD` or the index). Shell-created files are removed only if still unchanged since the command.
4. Derived artifacts in ignored paths are not restored; clean them with project commands (`cargo clean`, `npm run clean`), never `git clean -fd`.

**Not protected, stated plainly**:
- Gitignored files (including `.env`, local databases, `node_modules`, build outputs), files over the size cap, and any path outside the workspace root.
- External effects: network calls, remote Git operations, databases, other processes.
- Guard-set writes the harness cannot attribute because the user edited the same path during the command: reported as a conflict, never overwritten.

On macOS and Windows the harness cannot prevent a shell command from writing outside the workspace; Policy Engine approval and environment stripping are the only controls (§6.1).

---

## 7.0 Agent Loop Internals & Configuration Schema

### 7.1 Context Window Compaction & Overflow Management
When conversation history exceeds 75% of the model's active context window:
1. **Compaction**: previous turn pairs (user instruction + raw tool output) become a structured summary: goals attempted, files modified, key test/compiler output retained, verbose logs dropped.
2. **Working-set priority**: the active Step Manifest, current file diffs, and project configuration are pinned and never evicted.

### 7.2 Diff Rejection Workflow
When the user rejects a proposed diff:
1. **Option A (Rollback)**: revert the working tree via the Step Manifest to pre-step state.
2. **Option B (Refine)**: keep the current file state, ingest the user's corrective feedback, and re-enter the loop.

### 7.3 Missing Tool Policy
When a gate tool (`semgrep`, `gitleaks`, `pytest`, `eslint`) is missing from `$PATH`:
- **Interactive mode**: offer installation, abort, or an explicit audited bypass naming the missing check. A warning alone never counts as a passed gate.
- **Strict CI mode (`--strict`)**: BLOCK the gate with exit code 1.

### 7.4 Configuration Schema (`.devcode.json`)
Project-root file defining build, test, and verification commands:
```json
{
  "$schema": "<schema-url>",
  "project_name": "my-service",
  "verification": {
    "test_command": "npm test -- --bail",
    "lint_command": "npm run lint",
    "format_command": "npm run format:check",
    "typecheck_command": "npx tsc --noEmit"
  },
  "security": {
    "sast_tool": "semgrep",
    "secret_tool": "gitleaks",
    "dependency_tool": "osv-scanner",
    "strict_mode": false
  },
  "sandbox": {
    "timeout_seconds": 30,
    "linux_unshare_network": false
  }
}
```
The provider block and implemented fields are specified in `docs/mvp.md` M4 and M12.

### 7.5 Model Capabilities, Token Ledger & Caching (PRD FR-7)
Capability records and streaming-usage downgrade are implemented. Ledger, budgets, caching, text-tool and schema downgrade, and egress are planned. Peer evidence: RES-016.
- **Capability record**: one record per (provider, model), not per adapter (NIM and OpenRouter share an adapter but differ per model). Fields: native and parallel tool calls, streaming, streaming usage, reasoning control, cache mechanism (explicit, automatic, none), context window, max output, vision, edit format, prices (input, output, cache-read, cache-write). Source precedence: user config, provider `/models`, bundled table, conservative default. The source is reported as with `context_window_source`.
- **Configuration**: `model_capabilities` maps the connector name or custom endpoint URL to exact model IDs and per-field overrides. Example: `{"model_capabilities":{"openai":{"gpt-5":{"streaming_usage":false}}}}`. Catalog metadata is refreshed when `/model` fetches models. Missing fields fall through individually. Price fields use USD millionths per million tokens.
- **Runtime downgrade**: the loop counts failures per record. N malformed tool calls switch to a text tool protocol; a rejected `stream_options` disables streaming usage; a schema rejection simplifies the schema. Each downgrade is logged and shown in `/status`. Peers choose native or emulated tool calls by flag or template (goose, mini-swe-agent) and none downgrades at runtime, so this mechanism needs its own tests.
- **Ledger**: one row per model call, normalized: input = uncached + cache-read + cache-write; reasoning counted inside output; each counter `Option` where `None` means missing, never 0; each figure tagged `reported` or `estimated` (bytes/4). Persisted fields use `#[serde(default)]` so saved conversations still load (`tests/m28_resume.rs`).
- **Attribution**: input split into system prompt, tool schemas, history, file reads, command output; shown in `/status`, the end-of-task line, and the tool log.
- **Budgets**: NFR-3 and NFR-4 enforced before send; per-provider estimate calibration against reported usage; optional task cap with a named stop reason.
- **Caching**: stable prefix; Anthropic gets explicit markers on system prompt, last tool, and recent messages (where the record allows tool-level markers); OpenAI and Gemini rely on automatic caching. Report hit rate from cache-read over input.
- **Egress**: mark tool output as untrusted in prompts; deny private paths; preview what leaves the machine (FR-7.6).
- **Subscription OAuth**: Anthropic and Google forbid third-party use of consumer logins (RES-016, vendor claim). API keys are the default path; do not add Anthropic OAuth.

---

## 8.0 Non-Goals

1. **Not a general-purpose chatbot**: every interaction maps to engineering tasks and actionable diffs.
2. **Not a multi-tenant cloud sandbox**: it runs locally with developer privileges and does not replace Docker or microVMs for untrusted code.
3. **Not an IDE replacement**: it works in the terminal and integrates with existing editors via file interfaces.

---

## 9.0 Normative References & Traceability

- **Product Requirements**: [docs/prd.md](prd.md) (ATE-1001-PRD)
- **Codebase Understanding**: [docs/codebaseunderstanding.md](codebaseunderstanding.md) (ATE-1002-CBU)
- **Reference Catalog**: [references/README.md](../references/README.md) (ATE-1003-REF)
- **Empirical Research Series**: [docs/research/](research/) (DEVCODE-RES-001 through DEVCODE-RES-014; informative)
