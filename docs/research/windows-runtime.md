# Research Note: Windows Runtime Pitfalls & Architecture (Pass 9)

```
================================================================================
DOCUMENT ID    : DEVCODE-RES-009
REVISION       : 0.1.0
STATUS         : Working notes (reference study)
SOURCES        : references/codex/codex-rs/windows-sandbox-rs,
                 references/pi/packages/coding-agent (issue #7547, paths.ts),
                 references/kimi-code (packages/agent-core-v2/src/_base/text/encoding.ts)
RELATED        : docs/devcode.md §6.2, §6.4, docs/prd.md FR-6.1, TODO.md T6
================================================================================
```

## Question

What are the concrete technical pitfalls and requirements for supporting Windows 11 native (x86_64) as a Tier 1 target without requiring administrator elevation or compromising DevCode's honest security claims? What can DevCode steal or reject from Codex's `windows-sandbox-rs`, Pi's shell path normalization (issue #7547), and Kimi Code's text/BOM/EOL handling?

## Method and limits

- Evidence labels: **code** (direct inspection of local peer codebases), **prod data** (documented GitHub issues/changelogs).
- Examined 3 sources:
  1. `codex-windows-sandbox` (`references/codex/codex-rs/windows-sandbox-rs`): AppContainer, Win32 Job Objects, ACLs, elevated setup manifest.
  2. `pi` (`references/pi/packages/coding-agent/src/utils/paths.ts` and changelog #7547): Windows shell drive path translation.
  3. `kimi-code` (`references/kimi-code/packages/agent-core-v2/src/_base/text/encoding.ts`, `fileEditService.ts`): UTF-8/UTF-16 BOM detection, transcoding, and CRLF preservation.

---

## Peer Findings

### 1. Codex `windows-sandbox-rs`

- **Architecture**:
  - Implements a complex Windows sandboxing architecture using Windows AppContainer tokens, dedicated Capability SIDs, Windows Desktop station isolation (`desktop.rs`), and ACL manipulation (`acl.rs`, `deny_read_acl.rs`).
  - Requires a dedicated setup binary with an administrator manifest (`codex-windows-sandbox-setup.manifest` specifying `requireAdministrator`) and runs alongside a Windows background service (`windows-sandbox-service`).
  - Implements process tree cleanup using Win32 Job Objects (`job_object.rs`) with `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE`.
- **Verdict for DevCode**:
  - **Reject**: Elevated setup and AppContainer requirement. DevCode is fundamentally an unprivileged developer tool (`FR-6.2`, `arch §6.3`: "never call sudo / run without elevation"). Requiring UAC elevation, service registration, or driver/AppContainer setup breaks the frictionless curl/PowerShell one-line install and contradicts the honest engineering boundaries tenet.
  - **Steal**: Unprivileged Win32 Job Object lifecycle management. Configuring a Job Object with `SetInformationJobObject` (`JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE` and `JOB_OBJECT_LIMIT_SILENT_BREAKAWAY_OK`) guarantees that all child and grandchild processes launched by `run_command` are killed instantly when the job handle closes (e.g. on watchdog timeout or session abort), fulfilling NFR-8 on Windows without administrator rights.

### 2. Pi Issue #7547 (`paths.ts`)

- **The Problem**:
  - On Windows, developers frequently use Git Bash (MSYS2), MinGW, Cygwin, or WSL as their terminal shell.
  - Large language models frequently emit POSIX-style drive paths, such as `/c/Users/project/src/index.ts`, `/mnt/c/...` (WSL format), or `/cygdrive/c/...` (Cygwin format).
  - When such a path is passed to native Win32 APIs (e.g., `CreateFileW`, `GetFileAttributesW`, `std::fs::metadata`), Windows parses `/c` as a relative path from the root of the current drive (`C:\c\...`). This causes false `ENOENT` errors or, worse, allows unintended directory traversal.
- **Pi's Solution (`normalizeWindowsShellPath`)**:
  - Matches paths conforming to `/^/(?:mnt/|cygdrive/)?([a-z])(?:/(.*))?$/i`.
  - Replaces leading shell prefixes with native Windows drive letters: `C:\path\...`.
- **Verdict for DevCode**:
  - **Steal**: Native shell path normalization prior to path canonicalization. DevCode's runtime guard on Windows must normalize POSIX shell drive paths (`/c/`, `/mnt/c/`, `/cygdrive/c/`) to native drive letters (`C:\...`) before running `realpath` and workspace allowlist checks.

### 3. Kimi Code (BOM, EOL & Encoding)

- **The Problem**:
  - Windows development environments (Visual Studio, PowerShell, MSBuild) frequently write files with a Byte Order Mark: UTF-8 BOM (`0xEF, 0xBB, 0xBF`), UTF-16 LE BOM (`0xFF, 0xFE`), or UTF-16 BE BOM (`0xFE, 0xFF`).
  - Naive tools either:
    1. Misclassify UTF-16 text files as binary because of NUL bytes, blocking agent reads and diffs.
    2. Strip the UTF-8 BOM on edit, causing compiler errors or Git dirty diffs in legacy Windows codebases.
    3. Overwrite CRLF (`\r\n`) with LF (`\n`), causing massive git churn where every line appears modified.
- **Kimi Code Solution**:
  - Detects BOM in `encoding.ts`: explicitly tests leading bytes against `UTF8_BOM`, `UTF16LE_BOM`, and `UTF16BE_BOM`.
  - Transcodes UTF-16 to UTF-8 for model consumption while tagging the file record with `preserveBOM: true` and encoding type.
  - On write, restores the BOM if it was originally present.
  - Preserves native line endings (`\r\n` vs `\n`) on a per-file basis.
- **Verdict for DevCode**:
  - **Steal**: Full BOM and line-ending preservation. Step manifests (FR-3.1) must persist raw bytes without newline or encoding conversion. When harness file tools apply text diffs, they must preserve existing BOM and CRLF conventions.

---

## Windows Path Pitfalls Summary

| Pitfall | Threat / Defect | DevCode Resolution |
| :--- | :--- | :--- |
| **8.3 Short Names** (`PROGRA~1`, `DEVCOD~1`) | Bypass string prefix filters (e.g. `.devcode/` denylist) | Resolve all paths using `GetFinalPathNameByHandle` to obtain canonical long paths before allowlist/denylist checks. |
| **Shell Drive Paths** (`/c/repo`) | Parsed by Windows as `C:\c\repo` -> false ENOENT or path escape | Normalize via `normalizeWindowsShellPath` (`/c/` -> `C:\`) prior to canonicalization. |
| **Reserved Names** (`CON`, `PRN`, `AUX`, `NUL`, `COM1..9`, `LPT1..9`) | Win32 legacy device hangs or system errors | Reject any path segment matching Win32 reserved device names. |
| **Case Insensitivity** (`.DEVCODE/`, `src/App.ts`) | Denylist or allowlist mismatch on case variant | Compare platform-normalized lowercase/canonical paths for policy enforcement. |
| **Mandatory File Locks** (`ERROR_SHARING_VIOLATION`) | Subprocess holds file lock; rollback or replace crashes | Exponential backoff retry (up to 1.5s); report partial rollback per FR-3.5 rather than corrupting disk. |
| **Process Tree Orphans** | Subprocess spawns background daemons | Win32 Job Object with `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE` ensures atomic tree termination. |

---

## Architectural Feed into `devcode.md` §6.2

The cross-platform matrix in architecture §6.2 must be updated to formally mandate:
1. **Unprivileged Win32 Job Objects**: Process tree watchdog using Job Objects without administrative elevation.
2. **Path Normalization Pipeline**: Shell drive path translation (`/c/` -> `C:\`) followed by `GetFinalPathNameByHandle` long-name resolution.
3. **BOM & EOL Invariance**: Step manifests store raw bytes; file tools preserve UTF-8/UTF-16 BOMs and CRLF endings.
4. **Reserved Device Rejection**: Rejecting Win32 special devices (`CON`, `NUL`, etc.) and device-namespace paths. The `\\?\` prefix is stripped during canonicalization, not rejected (arch §6.4.4).
