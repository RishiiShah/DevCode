# Research Note: Account Connect vs Peers (Pass 14)

```
================================================================================
DOCUMENT ID    : DEVCODE-RES-014
REVISION       : 0.2.0
STATUS         : Working notes
SOURCES        : references/codex, opencode, pi, gemini-cli, cline; DevCode credentials/auth
RELATED        : docs/mvp.md M12–M13; RES-013
DATE           : 2026-11-25
================================================================================
```

## Question

Where does DevCode lag peers on connecting provider accounts (Codex / Claude / Gemini style), and what can we cover without OAuth or a native Anthropic adapter?

## Peer patterns (code)

| Peer | Mechanism |
| :--- | :--- |
| Codex | API key + device/browser OAuth; file or keyring store |
| OpenCode | `auth login/list/logout/switch`; rows show stored vs environment |
| Pi | `auth.json`; **stored beats env**; OAuth per provider |
| gemini-cli | OAuth personal + API key + keychain hybrid |
| Cline | `providers.json` multi-provider; OAuth handlers |

## DevCode before this pass

- API key only in `credentials.json`; `auth login/list/logout`; TUI `/connect`
- Presets: `nim`, `gemini` only
- `/connections` / `auth list`: `"name stored"` only
- `/connect` did not write `settings.json` (unlike `/provider`)
- `auth login` defaulted CLI name only for `--provider nim`
- Native Anthropic / OAuth / keyring / multi-profile: mvp.md non-goals

## Narrowed (still PARTIAL vs peers — not done)

1. Account list rows: `stored` / `env` / `stored+env` / `missing` + active `*` (OpenCode-shaped **list**, not full auth product).
2. `/connect` best-effort `persist_provider`.
3. `auth login` defaults from any `--provider` preset.
4. Presets `openai` + `openrouter` (proxy path retained).
5. **P10a landed partial:** native Messages adapter `src/agent/anthropic.rs` + preset `anthropic`. Fake-server green (`m20_anthropic`). Still behind OpenCode/pi on OAuth, SSE-only stream, and beta feature headers.
6. **P10b landed partial:** `auth login gemini --oauth` paste-code Google OAuth (`src/auth.rs`) writes `oauth-gemini.json` (access + refresh + expiry) and leaves the API key in `credentials.json` alone. Not wired into `openai_compat` / Code Assist yet. Fake-server tests `m21_oauth`.
7. **P10c landed partial:** opt-in `DEVCODE_CREDENTIALS_STORE=auto` tries the OS keyring then `credentials.json`; unset or `file` stays file-only. Tests use an in-memory blob.
8. **P10d landed partial:** named credential slots (`nim`, `nim:work`) and `auth switch` via `UserSettings.active_slot` (`tests/m22_auth_switch.rs`). `/connect nim:work` stores that key and selects it. One keyring blob.

**Do not mark P10 / P10a / P10b / P10c / P10d done** until peer bar in TODO is met.

## Open to reach peer level

| Gap | Peer bar |
| :--- | :--- |
| Native Anthropic Messages adapter | OpenCode / Cline Anthropic |
| OAuth / device code | Codex / gemini-cli |
| OS keyring (optional file) | Codex / gemini-cli |
| Multi-key per connector + `auth switch` | OpenCode `auth switch` |
| Stored-vs-env policy | Decided (P10e): explicit switch beats env; otherwise env-first. pi/OpenCode are stored-first; intentional difference. |

## Stop condition (peer-done)

Native or first-class Claude path **and** either OAuth or keyring-backed multi-account switch **and** list/switch UX at OpenCode depth. Until then: **partial**.
