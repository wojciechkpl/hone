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

### Review round 1 (U5–U12)

| Row | Behaviour |
|---|---|
| U5 | **Every agent is priced the same way.** A process-wide default price lookup (`hone_core::set_default_price_lookup`) is set once at start by the CLI, `honed`, the server and `hone eval`, from `PricingTable::default_table()` plus the user's `[pricing]`. An agent uses its own lookup if set, else the process default, else `AgentConfig.pricing`. So orchestrator leads and workers, ensemble candidates and rankers, the reviewer loop, `honed`'s executors and eval runs are all priced per model, and their unpriced models are reported. The hard-coded `$3/$15` in `honed`, `hone-server` and the reviewer loop is removed. `--orchestrate` with `openai:gpt-4o` reports the real cost. |
| U6 | **Dated model ids match their base entry.** A model id ending in `-YYYY-MM-DD` (e.g. `openai:gpt-4o-2024-08-06`) is priced with its base id's entry when it has none of its own. |
| U7 | **`[budget]` warns up front about unpriced models.** When a session limit is set, Hone checks the session model, every `[[routing]]` model and every fallback model before the first call; for each with no price it prints (stderr; a system line in the TUI) `hone: [budget] can't limit <model>: it has no price; add [pricing."<model>"] to your config`, once. |
| U8 | **A held finish doesn't delay the turn.** After `finish_reason`, the adapter waits for a trailing usage chunk for at most 2 s (or until `[DONE]`, end of stream, or the usage chunk, whichever comes first), then emits `Done`. An error chunk after `finish_reason` doesn't fail the turn: the completion stands (the error is logged). |
| U9 | **Usage within one response is counted once.** If a response reports usage more than once (inline and trailing, or cumulative per chunk), the last report is used. |
| U10 | **Missing usage is visible.** A successful call that reported no usage at all marks the run `"usage_known": false` in `--json` (and `cost_known` is false too). |
| U11 | **The TUI doesn't query the agent for unpriced models.** The agent sends `AgentEvent::UnpricedModel { model }` the first time a model is unpriced; the TUI shows the note from the event (no round trip after `Done`). The server's event maps name it `unpriced_model`. |
| U12 | **Rebased on main.** The branch merges cleanly with `main` (which added receipts and `context` to `--json`); tests re-run after. |

### Review round 2 (U13–U16)

| Row | Behaviour |
|---|---|
| U13 | **The TUI shows the unpriced note.** `App::handle_agent_event` handles `UnpricedModel` (one system line per model), so the note appears whichever path delivers the event. |
| U14 | **Orchestrated and ensemble runs report unpriced models.** `--orchestrate` and `--ensemble` print the same one-per-model stderr note as one-shot runs when any of their agents' models is unpriced, and their totals say the cost is incomplete. |
| U15 | **`honed` uses the user's prices.** `honed` sets the process default from the default table plus the user's `[pricing]`. |
| U16 | **Anthropic dated ids match too.** A trailing `-YYYYMMDD` (e.g. `anthropic:claude-sonnet-4-5-20250929`) is stripped to find the base entry, like `-YYYY-MM-DD`. Usage from a failed attempt is priced by the model that was tried. `docs/configuration.md` notes that usage arriving more than 2 s after the final chunk isn't counted, and that `AgentConfig.pricing` only applies when no price lookup is set. |

## Testing notes

- **U1** in `hone-providers` with a mock SSE server (existing adapter tests show the harness): usage after the finish chunk, inline, absent, and with tool calls.
- **U2–U4** in `hone-core`/`bins/hone`: a scripted provider reporting usage, routed to two models with different prices; an `ollama:` model; a user `[pricing]` entry; a project `[pricing]` ignored.

## Out of scope

- Fetching live prices.
- Changing which models the default table lists.
