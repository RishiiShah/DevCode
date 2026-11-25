# Research Note: Account Connect vs Peers (Pass 14)

```
================================================================================
DOCUMENT ID    : DEVCODE-RES-014
REVISION       : 0.1.0
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

## Gaps closed this pass

1. **Richer account list** — every preset row shows `stored` / `env` / `stored+env` / `missing`, active marked `*` (OpenCode list shape).
2. **`/connect` persists provider** — calls `persist_provider` after storing the key.
3. **Login default** — `auth login` accepts any `--provider` preset name, not only nim.
4. **More OpenAI-compat accounts** — presets `openai` and `openrouter` (Claude models via OpenRouter until a native Anthropic adapter).

## Still deferred (intentional)

| Gap | Why deferred |
| :--- | :--- |
| Native Anthropic Messages adapter | mvp.md post-MVP; different wire shape |
| OAuth / device code | mvp.md M12 non-goal |
| OS keyring | mvp.md non-goal |
| Multi-key per connector + `auth switch` | needs credential schema change |
| Pi-style stored-over-env | DevCode env-first is specified in M12 |

## Stop condition

Account list + connect persist + openai/openrouter presets shipped with tests; deferred gaps recorded here.
