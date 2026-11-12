# Token usage from OpenAI-compatible streams, and per-model pricing: spec

Branch `fix/usage-and-pricing`, off `main`. Found while reviewing `hone eval` (PR F); both bugs affect every one-shot, TUI and eval run.

## Problem

1. **Usage after the finish chunk is dropped.** Hone asks OpenAI-compatible servers for `stream_options.include_usage`. OpenAI (and compatible servers) send the usage in a separate last chunk with `choices: []`, after the chunk carrying `finish_reason`. The adapter (`crates/hone-providers/src/adapters/openai.rs`) emits `Done` on `finish_reason`, so that usage never arrives: token counts and cost are 0 for `openai:` models (reproduced against a mock that sends usage the OpenAI way; usage inline in the finish chunk works).
2. **Every model is priced as Sonnet.** `build_agent_config` (`bins/hone/src/main.rs`) hard-codes `$3/$15` per million tokens for whatever model runs. A `PricingTable` exists in `hone-providers` (used by `honed`'s routes) but the CLI ignores it. So a local Ollama model, which is free, is charged Sonnet prices, and `[budget]` session limits can stop a session over costs that don't exist. The agent also keeps the first model's price when routing (`[[routing]]`), a fallback or `/model` switches models.

## Rows

| Row | Behaviour |
|---|---|
| U1 | **Trailing usage is counted.** For OpenAI-compatible streams, a usage-only chunk after the `finish_reason` chunk (before `[DONE]`) is read, and its usage is reported once with the turn's result; usage inline in the finish chunk still works; a stream with no usage still finishes. Tool-call turns (`finish_reason: "tool_calls"`) report usage the same way. |
| U2 | **Prices come from the pricing table, by model.** The one-shot CLI, the TUI, `hone eval` and `honed`'s executor price each call with the entry for the model that served it, from `PricingTable::default_table()` merged with the user's `[pricing."<provider:model>"]` tables (user config only; keys `input_per_million`, `output_per_million`, optional `cache_read_per_million`, `cache_creation_per_million`; a project file's `[pricing]` is ignored with a warning). |
| U3 | **Unpriced models cost nothing, and say so.** A model with no entry (all `ollama:` models unless priced by the user, unknown `openai:` models) is costed at 0, and the first such call in a session prints one note on stderr: `hone: no price for <model>; its usage isn't counted towards cost or [budget]` (the TUI shows it once in the status line). `--json` gains `"cost_known": false` when any call was unpriced. |
| U4 | **Per-turn pricing follows the model.** When routing, a fallback model or `/model` changes the model, that turn's usage is priced with the new model's entry. |

## Testing notes

- **U1** in `hone-providers` with a mock SSE server (existing adapter tests show the harness): usage after the finish chunk, inline, absent, and with tool calls.
- **U2–U4** in `hone-core`/`bins/hone`: a scripted provider reporting usage, routed to two models with different prices; an `ollama:` model; a user `[pricing]` entry; a project `[pricing]` ignored.

## Out of scope

- Fetching live prices.
- Changing which models the default table lists.
