# Research Note: TUI Dogfood + Harder Real-Repo Task (Pass 13)

```
================================================================================
DOCUMENT ID    : DEVCODE-RES-013
REVISION       : 0.2.1
STATUS         : Working notes (measured, small sample)
SOURCES        : tests/m19_tui_dogfood.rs; live Gemini runs on /tmp/devcode-harder-20261125-150023
RELATED        : DEVCODE-RES-012; TODO Current; sessions-tui.md
DATE           : 2026-11-25
================================================================================
```

## Question

1. Does the hardened TUI (U1–U12) hold up when switching provider/model/effort and driving a live loop?
2. Does the harness still deliver minimal verified change on a task larger than the two-file RES-012 fixtures, with Gemini?

Labels: **production data** | **scripted** | **guess**.

## Non-goals

- Windows / installer / product rename (A1, A4, A5).
- Native Anthropic adapter; SSE streaming.
- Peer token comparison study.

## Setup

- Platform: macOS arm64.
- Provider: Gemini OpenAI-compat (`GEMINI_API_KEY`).
- Fixture: `/tmp/devcode-harder-20261125-150023/` — multi-file `shop` package (pricing / discount / tax / cart) with composition bug; 4 unittest cases, 3 failing before fix.
- Binary: workspace `target/debug/devcode`. Agent flags: `--plain -y --provider gemini --trace`.

## TUI dogfood checklist

| ID | Check | How | Result | Notes |
| :--- | :--- | :--- | :--- | :--- |
| D1 | Startup shows provider label | scripted m19 | **pass** | `App` with `gemini · …` |
| D2 | `/provider gemini` dispatches; blocked while Working | scripted m19 | **pass** | Busy path shows wait hint (by design) |
| D3 | `/model` catalog usable live | live ask used explicit model | **pass** | Switched to `gemini-flash-lite-latest` / `gemini-3-flash-preview` when default 503'd |
| D4 | `/effort` / `extra_body.reasoning_effort` live | live ask | **pass** | `gemini-3-flash-preview` + `reasoning_effort=high` → reply `effort-ok`, 1 step |
| D5 | Scroll pin + Ctrl-End | scripted m19 / U1 | **pass** | |
| D6 | Tool card Ctrl-T | scripted m19 / U3 | **pass** | |
| D7 | Diff modal scroll clamps on draw | scripted m19 / U4 | **pass** | Clamp is on draw, not on key |
| D8 | Ctrl-K palette | scripted m19 / U5 | **pass** | |
| D9 | Fuzzy model picker filter | scripted m19 / U6 | **pass** | |
| D10 | Status strip idle/working | scripted m19 / U7 | **pass** | |
| D11 | Shift+Enter multiline | scripted m19 / U8 | **pass** | |
| D12 | History vs scroll split | scripted m19 / U11 | **pass** | |
| D13 | Live ask without mutation | live `--plain --profile ask` | **pass** | 3 steps; `files edited: 0`; git clean of sources |
| D14 | Live build harder fixture | live `-y --profile build` | **pass** | See results table |

Interactive full-screen TUI (ratatui) was not driven end-to-end in a real terminal this pass; scripted `App` + live `--plain` cover the same Session/worker paths the TUI uses.

## Harder fixture

| Path | Role |
| :--- | :--- |
| `shop/pricing.py` | `unit_price` |
| `shop/discount.py` | `discount_rate` |
| `shop/tax.py` | `tax_rate` |
| `shop/cart.py` | **bug**: tax and discount as independent add/subtract on raw subtotal |
| `tests/test_cart.py` | expects `round(unit*qty*(1-d)*(1+t))` |

Pre-fix: 3/4 tests fail. Post-fix: 4/4 pass.

## Harder-task results (production data)

| Run | Provider / model | Steps | Tokens in | Tokens out | Edits | Outcome |
| :--- | :--- | ---: | ---: | ---: | ---: | :--- |
| D13 ask (paths only) | gemini / `gemini-flash-lite-latest` | 3 | 2,247 | 54 | 0 | Pass; answered `shop/cart.py` |
| D4 effort smoke | gemini / `gemini-3-flash-preview` + effort high | 1 | 443 | 2 | 0 | Pass; `effort-ok` |
| D14 build fix cart | gemini / `gemini-flash-lite-latest` | 7 | 12,998 | 467 | 1 (`shop/cart.py`) | Pass; gate Class A; existing tests red→green; rate tables untouched |

D14 summary line (harness): `files edited: 1, tests changed: 0, files changed by commands: 1, outside scope: 0, dependencies added: 0`.

The command-touched file was bytecode under `shop/__pycache__/` from running unittest (not a source edit). Rollback was not needed; git showed only `cart.py` + pyc.

Correct fix applied by the model:

```python
return round(unit * qty * (1 - disc) * (1 + tax))
```

## Problems found

| # | Severity | Where | Symptom | Disposition |
| :--- | :--- | :--- | :--- | :--- |
| 1 | Medium | Gemini preset default | `gemini-3.8-flash` often HTTP 503 under load; blocked D13 until model switch | **Fixed**: preset `default_model` → `gemini-3-flash-preview` (supports effort levels; answered in probes). `gemini_smoke` still falls back across flash catalog. |
| 2 | Low | TUI `/provider` while Working | Slash blocked with wait hint | **By design**; documented in m19. No change. |
| 3 | Low | Gate summary | `files changed by commands: 1` counts `__pycache__` | **Deferred**: pyc noise; consider ignoring `__pycache__` in reconcile later (A-track). |

## Comparison to RES-012 (NIM, 2-file)

| | RES-012 baseline-first one-line | RES-013 D14 multi-file |
| :--- | ---: | ---: |
| Steps | 6 | 7 |
| Tokens in | 8,115 | 12,998 |
| Tokens out | 414 | 467 |
| Files edited | 1 | 1 |

Same order of magnitude on a harder composition bug with Gemini flash-lite. n=1 each; not a peer bake-off.

## Stop condition

- Checklist D1–D14 filled: **met**.
- Harder-task row with measured steps/tokens: **met** (D14).
- Blocking bugs fixed or deferred: **met** (#1 fixed; #3 deferred).

## Changelog

- 0.1.0: opened note; checklist and fixture spec.
- 0.2.0: filled checklist; D13/D14/D4 live numbers; problem #1 preset default change.
- 0.2.1: TUI auto-fetches model list after connect/provider/reload/startup (`App.models` cache; `/model` opens from cache).
