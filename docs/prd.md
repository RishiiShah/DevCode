# [ATE-1001] Product Requirements Document: DevCode Harness

```
================================================================================
STANDARD       : ATE-1000 (Automated Technical Engineering Standard Series)
DOCUMENT ID    : ATE-1001-PRD
REVISION       : 0.4.1-draft
STATUS         : Working Draft (Engineering Specification)
AUTHOR         : DevCode Product Architecture Group
OWNS           : Requirements (what): FR-1..6, NFR-1..9, Gates 1..8.
                 Mechanisms (how) live in ATE-1000-ARCH; cited, not repeated.
SECTIONS       : 3 FR | 4 NFR | 5 Gates (acceptance tests)
RELATED DOCS   : [docs/devcode.md](devcode.md), [docs/codebaseunderstanding.md](codebaseunderstanding.md)
================================================================================
```

---

## 1.0 Product Vision & Purpose

**DevCode** is a developer-centric agentic CLI harness: one responsive agent loop, dynamic permission profiles (`ask`, `plan`, `build`, `debug`, `security`), and deterministic verification gates. The differentiator is the uncompromising enforcement of quality gates before finalization, not conversational output. Edits are provisional on disk while checks run; external Git operations are outside enforcement.

Design: ATE-1000-ARCH. Codebase indexing and security tooling: ATE-1002-CBU.

---

## 2.0 User Personas & Core Workflows

| Persona | Core Pain Point | DevCode Solution |
| :--- | :--- | :--- |
| **Full-Stack Engineer** | AI tools churn out untested code that breaks builds or silently alters behavior. | Class-based test gates, atomic diff reviews, compiler pass verification. |
| **Systems / Platform Engineer** | Index overwrites and destructive edits destroy local work. | Non-destructive Step Manifest rollbacks and strict filesystem allowlists. |
| **Security Engineer** | AI models introduce subtle flaws and hardcoded credentials. | Orchestration of proven SAST and secret scanners (`semgrep`, `gitleaks`). |

---

## 3.0 Functional Requirements (FR)

### FR-1: Unified CLI Loop with Dynamic Policy Profiles
- **FR-1.1**: Execute a single continuous agent loop, accepting user instructions without forced state transitions.
- **FR-1.2**: Mode profiles are permission and prompt lenses. Definitions: ARCH §3.1.
  - `ask`: read-only; write tools and mutating shell disabled at the tool schema level; no artifact, no gate. A change request is answered with the suggested profile switch, never executed.
  - `plan`: read-only; write tools disabled; structured specification output.
  - `build`: scoped workspace writes; verification gates enabled.
  - `debug`: hypothesis-driven prompting; reproducer assertion required.
  - `security`: read-only plus scanner execution; the harness (not the agent) applies user-approved remediation diffs (FR-5.4).
- **FR-1.3**: The developer may change the profile at any time via slash commands (`/ask`, `/plan`, `/build`, `/debug`, `/security`) or CLI flags.

### FR-2: Class-Based Test Enforcement Policy
- **FR-2.1 (Rule-Based Classification)**: Apply ARCH §5.1 using trusted pre-step policy. Class A (behavior/API/configuration) is the default; Class B is empty in v1; Class C is strictly inert content. Unknown formats, parse failures, and ambiguous impact are Class A; mixed batches use the strictest class. No extension or directory grants blanket exemption.
- **FR-2.2 (Enforcement Rule)**:
  - Class A: block finalization unless a new or updated behavior assertion is discovered and executed successfully, alongside applicable tests, builds, and validators. Configuration changes may use fixture assertions or dry-run checks; syntax/schema validation alone is insufficient.
  - Class B (empty in v1): a future AST-proven refactor requires all applicable existing tests green. Class C: formatting/lint checks suffice.
  - Only an explicit user flag or instruction identifying the bypass authorizes an audited exception. Run all remaining gates; a warning or model instruction is not authorization.
  - Bind results to the checked diff hash and invalidate affected checks after later edits.
- **FR-2.3 (Audit Ledger & Truncation Anchor)**: Record each bypass in append-only `.devcode/audit.log` (timestamp, user authorization, skipped checks, reason, diff SHA, `prev_entry_sha`). Anchor the tail outside the workspace (user state dir and, with Git, `refs/devcode/audit-tail`). Validate chain, count, and anchor at session start and before finalization; deletion, truncation, or mismatch blocks finalization. Mechanism and error codes: ARCH §5.2.
- **FR-2.4 (Policy Integrity)**: Proposed changes to gate commands, test discovery, or classification rules are checked against user-approved pre-step policy. They cannot exempt themselves or take effect before explicit user acceptance.

### FR-3: Safe Step-Manifest Checkpointing & Rollback
Mechanism: ARCH §4 (rollback), §6.4 (shell).
- **FR-3.1**: Before any path mutation, persist pre-step existence, type, exact bytes or symlink target, permissions, and hash. A hash alone is insufficient. `HEAD` is optional diagnostic metadata. Block the write if capture fails.
- **FR-3.2 (Mutation Manifests & Shell Write Policy)**:
  - Publish mutation records under `.devcode/manifests/<session_id>/<step_id>.json` before any file-tool write; record post-step state for conflict detection.
  - Dedicated file tools (`write_file`, `edit_file`, `patch`) are the sole authorized path for agent source and configuration edits.
  - Shell subprocesses (`run_command`) may write derived build artifacts, compiler outputs, and temp files in the workspace; blocking them would freeze `cargo build`, `npm test`, `pytest`, `make`, `dotnet`. Rationale: ARCH §6.4.1.
  - Lexical guarding with heredoc stripping denies shell commands and redirections targeting `.devcode/` or tracked source files before execution (ARCH §6.4.3).
  - Before any mutation-capable shell command, snapshot the guard set (non-ignored workspace files except `.devcode/`, 10 MB per-file cap) incrementally and publish the pre-step record first; afterwards record created, modified, and deleted paths. Rollback restores shell-changed guard-set paths under the FR-3.4 preflight (ARCH §6.4.5).
  - Destructive patterns (`rm`, `mv`, `sed -i`, `git clean`, `git checkout --`, `git restore`, `git reset --hard`, `git stash`) always require user approval.
  - If the guard-set budget is exceeded or a file cannot be read, block the command. Only explicit user approval may run it without rollback protection; record `OVERRIDE_NO_SNAPSHOT` with the unprotected paths in the audit log.
  - A subprocess write to source or configuration invalidates prior gate results and forces Class A re-evaluation. Gitignored files, oversize files, and paths outside the workspace are not rollback-protected; clean ignored build artifacts via build-tool commands, never `git clean -fd`.
- **FR-3.3**: Never mutate the staged index or unrelated paths. Preserve pre-existing unstaged and untracked content on touched paths.
- **FR-3.4**: Roll back steps in reverse order from persisted pre-step content, never `HEAD` or the index. Before any restore, preflight every touched path against recorded post-step state; abort on concurrent edits or incomplete records. Remove only unchanged agent-created files; restore modified/deleted files with original type and permissions. Revalidate workspace boundaries during recovery.
- **FR-3.5**: Keep active and unresolved recovery checkpoints out of retention pruning. On storage exhaustion block new writes; on recovery I/O failure retain backups and report partial recovery. Verify restored state before reporting success.

### FR-4: Practical Runtime Boundaries & Honest Security Claims
Mechanism: ARCH §6.
- **FR-4.1 (Filesystem)**: Check all writes against a canonicalized (`realpath`) allowlist limited to the repository root. `.devcode/` is excluded in every profile: only the harness writes `audit.log` and `manifests/`. Shell tampering is detected, not prevented, on macOS and Windows (ARCH §5.2).
- **FR-4.2 (Network)**: Strip sensitive credentials and tokens (`AWS_*`, `GITHUB_TOKEN`, `*_API_KEY`) from child environments. On Linux, `bwrap --unshare-net` is configurable. On macOS and Windows, children have standard socket access.
- **FR-4.3 (Process Lifecycle)**: Run tool processes in an isolated process tree (POSIX process group; Windows Job Object) under a configurable watchdog timeout (default 30 s); abort terminates all children (`SIGKILL` on POSIX, job termination on Windows).

### FR-5: Orchestrated Security Auditing
Tool details: ATE-1002-CBU §5.0.
- **FR-5.1**: Orchestrate industry-standard tools, not custom taint analysis: secrets via `gitleaks`; static rules via `semgrep` (OWASP Top 10 rulesets); dependency CVEs via `osv-scanner` or `trivy`.
- **FR-5.2 (Offline-First DB)**: Scans use locally cached vulnerability databases. Syncs happen out-of-band via `devcode security --update-db`.
- **FR-5.3 (Missing Tool Policy)**: ARCH §7.3. Interactive mode blocks pending install, abort, or audited bypass; `--strict` blocks the gate.
- **FR-5.4 (Remediation Diff Application)**: The `security` profile emits remediation patches as structured unified diffs without modifying the workspace. On acceptance the harness applies the patch under FR-3.1 capture, then runs the security gate. The model never applies diffs in this profile.

### FR-6: Cross-Platform Support & One-Line Installation
Mechanism and hosting decisions: ARCH §6.2–6.3.
- **FR-6.1 (Platforms)**: Tier 1: macOS (arm64, x86_64), Linux (x86_64, aarch64; glibc and musl), Windows 11 native (x86_64). WSL2 counts as Linux. FR-1 to FR-5 hold on every Tier 1 platform; any OS-specific limit is listed in ARCH §6.2, never silently relaxed.
- **FR-6.2 (Primary Install)**: `curl -fsSL https://<install-host>/install.sh | sh` (macOS/Linux/WSL2) and `irm https://<install-host>/install.ps1 | iex` (Windows). During beta `<install-host>` is a free Cloudflare Pages subdomain with assets in GitHub Releases on a personal account; a custom domain is documented only once owned. The installer follows ARCH §6.3: POSIX `sh`, platform detection, checksum verification before extraction, no `sudo`, idempotent re-run, version pinning, `--no-modify-path`.
- **FR-6.3 (Implementation Language & Single Binary)**: Implemented in **Rust**, one self-contained native binary per Tier 1 target (fully static on Linux musl), no external runtime. Rust does not by itself meet NFR-1 or NFR-2; those are verified by benchmark and remain unmeasured.
- **FR-6.4 (Release Integrity & Signing)**: Every release publishes `SHA256SUMS` signed via **minisign** (`SHA256SUMS.minisig`) with a pinned Ed25519 public key embedded in the installers; CI adds Sigstore/cosign provenance attestations and claims no SLSA level until audited. Validate checksums and signatures before extraction.

---

## 4.0 Non-Functional Requirements (NFR) & Benchmark Recipes

Every NFR has a reproducible recipe. NFR-5 applies only after Tier 2 is enabled; rollback and gate acceptance tests are release prerequisites. Active recovery data takes precedence over NFR-7: block new mutations rather than evict needed backups.

| ID | Metric | Target | Recipe |
| :--- | :--- | :--- | :--- |
| NFR-1 | Startup, cold | <= 600 ms | Time-to-first-prompt on the 50k LOC reference repo, cold OS disk cache. |
| NFR-2 | Startup, warm | <= 150 ms | Time-to-first-prompt on the same repo, warm filesystem cache. |
| NFR-3 | System prompt baseline | <= 2,500 tokens (Tier 1 = 0, Tier 2 = 2.5k) | Token count of system prompt + repo map injected before the user query. |
| NFR-4 | Tool context per turn | <= 6,000 tokens | Hard ceiling on file slices and ripgrep results ingested into a model turn. |
| NFR-5 | Tier 2 PageRank compute | <= 2.0 s | Wall-clock to parse 500 files and compute Personalized PageRank. |
| NFR-6 | Rollback fidelity | 100% bit-for-bit parity on captured paths | SHA-256 comparison of uncommitted touched files before vs after rollback. Covers bytes, type, and POSIX mode or Windows attributes; ACLs out of scope. |
| NFR-7 | Checkpoint disk ceiling | <= 50 MB (auto-pruned at 7 days) | Measure `.devcode/manifests` usage across 50 consecutive interactive runs. |
| NFR-8 | Sandbox kill watchdog | <= 100 ms to terminate runaway children | Launch a runaway child; verify `kill(-pgid)` on POSIX or Job Object close on Windows leaves 0 orphans. |
| NFR-9 | Shell snapshot overhead | Incremental <= 250 ms; first <= 2 s (targets, unmeasured) | Run a no-op mutation-capable command on the reference repo; time snapshot and reconcile, warm and cold. |

**Reference environment**: 50,000 LOC mixed TypeScript/Python repository (1,200 files); 8-core CPU, 16 GB RAM, Apple Silicon or modern x86 Linux.

---

## 5.0 Acceptance Criteria & Verification Protocol

Flow: the developer submits a task; the loop applies the profile, records pre-run blob state in a step manifest, and applies edits to allowlisted files. The classifier (ARCH §5) then decides. Class A without a new assertion refuses completion unless `--skip-tests` is given, which writes `.devcode/audit.log` and proceeds. Failing tests route to the auto-debug loop. All checks passing presents a unified diff: reject triggers rollback (zero index mutation); accept commits the manifest to session history.

1. **Gate 1 (Checkpoint Invariance)**:
   - Setup: a partially staged file, a dirty unstaged file, an existing untracked file, and an unrelated dirty file; record content hashes, file types, permissions, and staged index state.
   - Action: modify the partially staged, unstaged, and untracked files, delete a captured file, create a new file, then roll back. Repeat after restarting the harness to verify persisted recovery data.
   - Verification: all captured paths match pre-step bytes, type, and permissions; only agent-created paths disappear. Staged index and unrelated files remain identical. Excluding harness metadata, `git status --porcelain` matches pre-step state. Repeat without an existing `HEAD`.
2. **Gate 2 (Test Requirement Enforcement)**:
   - Editing a Class A source file without adding or modifying tests triggers an automated refusal unless `--skip-tests` is given. Every override appends a verifiable JSON entry in `.devcode/audit.log`. An agent write to `.devcode/` through a file tool is denied; a tampered or truncated log blocks finalization.
3. **Gate 3 (Process Tree Cleanup)**:
   - Commands spawning background sub-processes that exceed timeout have their entire process tree eradicated without orphaned daemons, on every Tier 1 platform.
4. **Gate 4 (Concurrent Edit and Recovery Failure)**:
   - Change an agent-modified or agent-created path externally before rollback. Preflight reports a conflict without altering any path and retains backups. Inject capture failure and verify no write occurs; inject restore failure and verify partial recovery is reported with backups retained.
5. **Gate 5 (Configuration Gate Integrity)**:
   - Change a dependency manifest, compiler setting, CI workflow, and `.devcode.json` test command. Verify Class A applies and pre-step gate commands remain active. Attempt to disable test discovery, label behavior changes as refactoring, and place executable content under `docs/`; none receives automatic exemption.
   - Inert documentation receives Class C, unknown formats default to Class A, a non-discovered assertion blocks completion, and later edits invalidate prior results. Explicit bypasses identify skipped checks in the audit ledger.
6. **Gate 6 (Installer Acceptance)**:
   - Run the installer in a clean environment per Tier 1 target (including an Alpine/musl container and, where available, a Rosetta shell) under `dash`, `bash`, and `zsh`.
   - Verify: correct binary selected; `devcode --version` runs; re-run is idempotent and does not duplicate PATH entries; `--version` pins; a tampered asset aborts before extraction leaving no binary or temp files; a truncated script download executes nothing; no `sudo`; `devcode uninstall` removes the binary and PATH edit only.
7. **Gate 7 (Platform Parity)**:
   - CI runs Gates 1 to 5 and 8 on macOS, Linux, and Windows, including case-variant paths (`.DEVCODE/`, mixed-case workspace paths) to confirm allowlist and `.devcode/` deny rules hold on case-insensitive filesystems. A platform failing any gate is not Tier 1.
8. **Gate 8 (Shell Mutation Recovery)**:
   - Setup: a dirty tracked file, a partially staged file, an untracked non-ignored file, and a gitignored file. Repeat in a non-Git directory.
   - Action: run shell commands that edit in place (`sed -i`, a script writing via `open().write`, `>` redirect), delete, and create files, then roll back.
   - Verification: changed non-ignored files match pre-step bytes, type, and mode; shell-created files are removed; the staged index is untouched; the gitignored file is reported unprotected, not restored. A command over budget is blocked unless approved, and approval appends `OVERRIDE_NO_SNAPSHOT`. A user edit to the same path during the command yields a conflict, not an overwrite.

---

## 6.0 Normative References & Traceability

- **System Architecture**: [docs/devcode.md](devcode.md) (ATE-1000-ARCH)
- **Codebase Understanding**: [docs/codebaseunderstanding.md](codebaseunderstanding.md) (ATE-1002-CBU)
- **Reference Catalog**: [references/README.md](../references/README.md) (ATE-1003-REF)
- **Empirical Research Series**: [docs/research/](research/) (DEVCODE-RES-001 through DEVCODE-RES-014; informative)
