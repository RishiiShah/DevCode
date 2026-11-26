# RES-016 Peer evidence: model capabilities, token ledger, caching, fallback, OAuth terms

Informative, never normative. Source: read-only scouts over `references/` clones (file:line cited) plus one web check dated 2026-11-26. Feeds PRD FR-7 and ARCH §7.4 (planned).

## Usage and cost schema (code)

- pi `packages/ai/src/types.ts:477` `Usage`: input, output, cacheRead, cacheWrite, cacheWrite1h?, reasoning? (undefined when the provider gives no breakdown), totalTokens, cost{...}. Provider mapping: Anthropic `cache_read_input_tokens`, `cache_creation_input_tokens` (`anthropic-messages.ts:857`); Google `cachedContentTokenCount`, `thoughtsTokenCount` (`google-generative-ai.ts:248`); OpenAI Responses `cached_tokens`, `output_tokens_details.reasoning_tokens` (`openai-responses-shared.ts:677`).
- opencode `packages/ai/src/schema/events.ts:25`: inputTokens = nonCached + cacheRead + cacheWrite (invariant); cost = tokens times per-1M price, provider-reported cost preferred (`session/usage.ts:22-43`).
- codex `protocol/src/protocol.rs:2600` `TokenUsage`: input, cached_input, cache_write_input, output, reasoning_output, total. Estimate: bytes/4 (`utils/string/src/truncate.rs:6`). Context left: baseline 12,000 tokens excluded (`protocol.rs:2777`); `effective_context_window_percent` 95 (`openai_models.rs:484`).
- aider `base_coder.py:1994-2097`: cache hit = `prompt_cache_hit_tokens` or `cache_read_input_tokens`; cost uses write x1.25 and hit x0.10 for Anthropic; falls back to its own token count when usage is absent.
- gemini-cli `code_assist/converter.ts:323`: reads `thoughtsTokenCount`, `toolUsePromptTokenCount`, `cachedContentTokenCount`.

Decision for DevCode: adopt the invariant (input = uncached + cache-read + cache-write), keep reasoning inside output as a subset, use `Option` so unreported stays missing (pi's `reasoning?`), bytes/4 only as a labeled estimate.

## Capability record (code)

- pi `types.ts:879` `Model`: reasoning, thinkingLevelMap, promptCache lifetimes, contextWindow, maxTokens, cost, input modalities, plus `compat` flags that change wire behavior (`compat-schema.ts:197-338`: supportsUsageInStreaming, maxTokensField, requiresToolResultName, thinkingFormat, cacheControlFormat). Flags are auto-detected from the base URL when unset.
- opencode: models.dev gives tool_call, reasoning options, limits, cache prices (`models-dev/v1.ts:35`, `models-dev.ts:26-122`); user config overlays capabilities and limits per field (`config/plugin/provider.ts:208`), but not cost. Tool-schema sanitizer chosen by model name (gemini, kimi) (`protocols/utils/tool-schema.ts:56`).
- codex `ModelInfo` (`openai_models.rs:422`): shell type, reasoning-summary support, apply_patch tool type, truncation policy, context window.
- aider `ModelSettings` (`models.py:123`): edit_format, use_repo_map, cache_control, streaming, reasoning_tag, weak_model, extra_params; exact yaml entry then substring rules.

Decision: one record per (provider, model). Precedence user config, then provider /models, then bundled table, then conservative default; report the source.

## Tool-call fallback

- goose `ToolCallingMode { Auto, ForceNative, ForceEmulated }` (`llamacpp/mod.rs:114`); auto-detect by chat-template dry run.
- mini-swe-agent: text actions in a fenced block; exactly one action or `FormatError` re-prompt (`actions_text.py:15`); chosen by class, not capability.
- No peer downgrades the tool format at runtime after repeated malformed calls. This is DevCode's own mechanism, so it needs its own tests and is unproven.

## Prompt caching

- Anthropic needs explicit markers: aider marks system, repo and chat chunks (`chat_chunks.py:28`) and can send keepalive pings (`--cache-keepalive-pings`); roo marks the system prompt and last two user messages (`caching/anthropic.ts`); pi marks system, last tool, last messages, gated by `supportsCacheControlOnTools` and `cacheRetention`. Tool-level markers are not universal.
- OpenAI and Gemini cache automatically (vendor claim; peers only report counters).

## Subscription OAuth terms (vendor claim; web, 2026-11-26)

- Anthropic: forbids third-party use of Free/Pro/Max logins and storing Claude.ai credentials; use API keys (`code.claude.com/docs/en/legal-and-compliance`). Only the unmodified Claude Code binary is covered.
- Google: Gemini CLI terms call using third-party tools with Gemini CLI OAuth a violation; mass 403 suspensions reported on a Google forum (unverified; geminicli.com is a community mirror).
- OpenAI: "Sign in with ChatGPT" plan usage for third-party apps announced 2026-09-29 (press); terms and eligibility not verified from OpenAI. Scraping Codex CLI tokens: unclear.
- Consequence: DevCode's landed Gemini OAuth (P10b) carries ban risk; do not extend OAuth to Anthropic.

## Not checked

Peer behavior was read, not run. Cline cache prices, OpenHands SDK, opencode compaction, vendor primary terms for OpenAI, and Anthropic consumer-terms text are unread.
