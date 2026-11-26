# AGENTS.md: Read Map

Read what the task needs and nothing else. Every fact has one owner document; other documents cite it by section instead of copying it. To open one section, list headings first (`rg -n '^#{2,4} ' docs/devcode.md`), then read only that range.

## Which file for which task

| Task | Read | Skip |
| :--- | :--- | :--- |
| What is open or next | `TODO.md` §2.0 | `TODO.md` §5.0 (history) |
| Implement or review milestone `Mx` | `docs/mvp.md` §2, §4, §5 and task `Mx`; then the spec sections that task cites | other tasks |
| Behavior is required, testable, or has a number (FR, NFR, Gate) | `docs/prd.md` §3.0–5.0, the one FR or Gate you need | `docs/devcode.md` unless the FR cites it |
| Profiles and tool permissions | `docs/devcode.md` §3 | |
| Rollback, manifests, checkpoints | `docs/devcode.md` §4 | |
| Gate classes (A/B/C), audit log | `docs/devcode.md` §5 | |
| Sandbox, env stripping, shell policy, Windows paths | `docs/devcode.md` §6.1, §6.2, §6.4 | §6.3 |
| Installer, signing, hosting | `docs/devcode.md` §6.3; `install.sh`; `packaging/README.md` | |
| Compaction, config schema, missing-tool policy | `docs/devcode.md` §7 | |
| Search, repo map, Tier 2 PageRank | `docs/codebaseunderstanding.md` §4 | §2–3 (background) |
| Security scanners | `docs/codebaseunderstanding.md` §5 | |
| Why a design choice was made, or peer evidence | the one `docs/research/` note below | the rest |
| Peer clone locations and licenses | `references/README.md` §2.0 | clone contents |
| User-facing commands and install | `README.md` | |

Never read: `references/<peer>/` clone contents (search them only for a named question), `target/`, `.env*`.

## Documents

| ID | File | Owns |
| :--- | :--- | :--- |
| `ATE-1000-ARCH` | `docs/devcode.md` | Design mechanisms |
| `ATE-1001-PRD` | `docs/prd.md` | Requirements, NFRs, gates |
| `ATE-1002-CBU` | `docs/codebaseunderstanding.md` | Retrieval, security tools |
| `ATE-1003-REF` | `references/README.md` | Peer source catalog |
| `ATE-1004-TODO` | `TODO.md` | Status and open work |
| `ATE-1005-MVP` | `docs/mvp.md` | Per-task deliverables and acceptance |

Research notes (informative, never normative), all under `docs/research/`:

| ID | File | Topic |
| :--- | :--- | :--- |
| RES-001 | `agent-loop.md` | Agent loop (mini-swe-agent, mistral-vibe) |
| RES-002 | `repo-map-git.md` | Repo map, Git and edit safety (aider) |
| RES-003 | `sandbox-approvals.md` | Sandbox and approvals (codex, gemini-cli) |
| RES-004 | `modes-permissions.md` | Modes as permission lenses (roo-code, cline) |
| RES-005 | `sessions-tui.md` | Sessions and TUI UX (opencode, pi) |
| RES-006 | `swe-agent-envs.md` | SWE-agent execution environments |
| RES-007 | `reference-selection.md` | Which peers to study |
| RES-008 | `rollback-peers.md` | Rollback peers vs FR-3 |
| RES-009 | `windows-runtime.md` | Windows runtime pitfalls |
| RES-010 | `demand-evidence.md` | Demand for test gates |
| RES-011 | `token-and-discipline-demand.md` | Demand for token frugality and discipline |
| RES-012 | `first-live-runs.md` | First live NIM runs |
| RES-013 | `tui-dogfood-and-harder-task.md` | TUI dogfood and harder real-repo task |
| RES-014 | `account-connect.md` | Account connect vs peers |
| RES-016 | `capability-and-token-peers.md` | Peer capability records, token ledger, caching, fallback, OAuth terms |

## Editing rules

- Cite, do not copy. Write `ARCH §6.4.5` or `PRD FR-3.4` instead of restating a rule. If a rule changes, change it in the owning document only.
- Keep section numbers stable. Code, tests, and other documents cite them. Add new material as a new subsection; do not renumber.
- Requirement changes land in `ATE-100x` documents, never only in a research note. Bump the `REVISION` line when normative text changes.
- Record status in `TODO.md` in one line with pointers (spec section, test file, RES note). Put long evidence in a research note.
- Never read or print real credentials or `.env*` files.
