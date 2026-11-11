# Fallback notice: say when a fallback model took over

Status: spec for TDD (rows N1–N11). Follows PR #87 (routing + `fallback_models`).

## Problem

Since #87, `fallback_models` (user config only) take over a model call when the
turn's model fails before answering. Nothing says so:

- the TUI shows the answer as if the turn's model gave it;
- one-shot prints nothing; `--json` reports `"model"` = the model the run chose;
- `HONE_CACHE` stores the fallback's answer under the request key of the
  requested model and replays it later, even when that model is back;
- the TUI's adaptive classifier records the turn's outcome under the turn's
  model, so it learns from answers another model gave.

## Design

```mermaid
%%{init: {"themeVariables": {"fontSize": "13px"}, "flowchart": {"nodeSpacing": 25, "rankSpacing": 30, "padding": 8}, "sequence": {"actorMargin": 30, "messageMargin": 25, "boxMargin": 6}, "gantt": {"barHeight": 16, "barGap": 3}}}%%
flowchart TD
  D["ModelDispatch / FallbackChain<br/>member 0 fails, member k takes over"] --> E["ProviderEvent::ModelFallback<br/>{requested, fallback, reason}<br/>first event of the stream"]
  E --> A["Agent: forwards as<br/>AgentEvent::ModelFallback"]
  E -.->|"stream contains it"| C["CachingProvider:<br/>not stored"]
  A --> O["one-shot: stderr line,<br/>once per pair per run"]
  A --> J["--json: \"fallbacks\" array"]
  A --> T["TUI: system message,<br/>once per pair per turn"]
  A --> R["TUI adaptive outcome:<br/>recorded under the fallback model"]
  A --> S["honed SSE: event name<br/>model_fallback"]
```

One new event, produced where the switch happens and passed through unchanged.
Each surface decides how often to show it.

## Rows

| Row | Contract |
|---|---|
| **N1** | `hone_proto::ProviderEvent::ModelFallback { requested: String, fallback: String, reason: String }` and `hone_proto::AgentEvent::ModelFallback { requested: String, fallback: String, reason: String }`. Both serialize as `{"type":"model_fallback","requested":…,"fallback":…,"reason":…}` and round-trip. |
| **N2** | `FallbackChain::complete`: when the first member answers, the stream is unchanged (no new event). When member *k* > 0 takes over (its attempt returned a stream), the returned stream's **first** event is `ModelFallback { requested: members[0].id(), fallback: members[k].id(), reason }`, followed by the carried `Usage` of failed members, then member *k*'s stream. One event per call however many members failed in between. When every member fails, the failure is reported exactly as today, with no `ModelFallback`. A request error that does not hand over is returned as today. A member *k* > 0 whose stream fails in a way that is not retried (e.g. 400) is still a switch: `[ModelFallback, …usage, Error]`. |
| **N3** | `reason` describes why member 0 did not answer: its stream failure message, its request error's `Display`, or its `Unavailable` health reason. Sanitized: every control character (Unicode `Cc`, including ESC, CR, LF, TAB) becomes a space, runs of whitespace collapse to one space, the ends are trimmed; longer than 200 characters → the first 200 characters (cut on a char boundary) followed by `…`; empty after this → `unknown error`. |
| **N4** | `ModelDispatch` with `fallback_models`: `requested` is the request's full model id (`anthropic:claude-…`), `fallback` the full id of the fallback model that took over (`openai:gpt-4o`). Without a fallback list, or when the requested model answers, the stream is unchanged. |
| **N5** | `Agent`: each `ProviderEvent::ModelFallback` is sent as `AgentEvent::ModelFallback` with the same fields, at its place in the stream (before that call's tokens). It adds no text to the response or the conversation, and the turn continues normally. Retry rules of #85 are unchanged: the event counts as output, so a call is never retried after it and the notice is never sent twice for one call. |
| **N6** | `CachingProvider`: a response whose events contain `ModelFallback` is passed through unchanged but **not stored** (like one containing `Error`). A later identical request goes to the provider again. |
| **N7** | One-shot (`hone "<prompt>"`, with or without `--json`; and the `--orchestrate` / `--ensemble` printers for the events they receive): for each `AgentEvent::ModelFallback`, stderr gets `hone: <requested> failed (<reason>); switched to fallback <fallback>` on its own line, **once per (requested, fallback) pair per run**, however many model calls of the run fell back. stdout (the answer, or the `--json` document) is unaffected. |
| **N8** | `--json` output gains `"fallbacks"`, always present: `[]` when no fallback took over; otherwise one object per (requested, fallback) pair in first-seen order: `{"requested": …, "fallback": …, "reason": <first reason seen for the pair>, "calls": <number of ModelFallback events for the pair>}`. `"model"` keeps its meaning (the model the run chose). `"success"` is unaffected. No other key changes. |
| **N9** | TUI (`hone_tui::App::handle_agent_event`): `AgentEvent::ModelFallback` pushes one system message `<requested> failed (<reason>); switched to fallback <fallback>`, **once per (requested, fallback) pair per turn**; the set resets when the turn ends (`Done` or `Error`). Tokens after it start a new assistant message (existing behaviour). Nothing else in the status changes. |
| **N10** | TUI adaptive classifier (`record_outcome_for_event` in `bins/hone/src/main.rs`): a turn in which a `ModelFallback` arrived is recorded under the fallback model of the last such event, instead of the turn's model. Reset per turn. (TUI loop: reviewed by reading.) |
| **N11** | `honed`: the SSE / execution / orchestration event-name maps name the event `model_fallback` (not `unknown`). |

## API names (§3)

- `hone_proto::ProviderEvent::ModelFallback { requested, fallback, reason }`
- `hone_proto::AgentEvent::ModelFallback { requested, fallback, reason }`
- `FallbackChain` names members by `ModelProvider::id()`; `ModelDispatch`'s
  per-model members must report their full model id there (how is up to the
  implementer).
- `--json` key: `fallbacks` (array of `{requested, fallback, reason, calls}`).

## Existing tests

Tests in `hone-providers` (`model_dispatch.rs`, `stream_retry_fallback.rs`,
`fallback*.rs`) and `bins/hone/tests/routing_entry.rs` that assert the exact
event sequence or `--json` document after a fallback took over gain the
`ModelFallback` event / the `fallbacks` key. The golden `--json` test gains
`"fallbacks": []` and nothing else.

## Out of scope (follow-ups)

- Orchestrator workers and ensemble candidates don't forward their events, so a
  fallback inside a worker isn't reported (only the lead / merge turn's are).
- `TierFallbackProvider` (unused by `hone`) emits nothing.
- Cost estimates still use the session's flat pricing for every model.
- The error text of `AgentEvent::Error` itself is still printed unsanitized.
