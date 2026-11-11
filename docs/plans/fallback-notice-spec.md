# Fallback notice: say when a fallback model took over

Status: spec for TDD (rows N1–N12; N3, N7–N10 and N12 revised after review round 1). Follows PR #87 (routing + `fallback_models`).

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
| **N3** | `reason` describes why member 0 did not answer: its stream failure message, its request error's `Display`, or its `Unavailable` health reason. Sanitized, in this order: (a) every control character (Unicode `Cc`, including ESC, CR, LF, TAB, DEL, C1) and every bidi / zero-width format character (U+061C, U+200B–U+200F, U+202A–U+202E, U+2060–U+2064, U+2066–U+2069, U+FEFF) becomes a space; (b) runs of whitespace collapse to one space and the ends are trimmed; (c) URLs are redacted: in a URL (`scheme://…`, ending at whitespace or one of `) ] > " ' ,`), user info right after `://` (`user@` or `user:pass@`, before the first `/`) is removed, and a query string (from `?` to the end of the URL) is removed — e.g. `error sending request for url (http://u:p@h:1/v1/x?key=s)` → `error sending request for url (http://h:1/v1/x)`; (d) longer than 200 characters → the first 200 characters (cut on a char boundary), trailing whitespace trimmed, followed by `…`; (e) empty after this → `unknown error`. |
| **N4** | `ModelDispatch` with `fallback_models`: `requested` is the request's full model id (`anthropic:claude-…`), `fallback` the full id of the fallback model that took over (`openai:gpt-4o`). Without a fallback list, or when the requested model answers, the stream is unchanged. |
| **N5** | `Agent`: each `ProviderEvent::ModelFallback` is sent as `AgentEvent::ModelFallback` with the same fields, at its place in the stream (before that call's tokens). It adds no text to the response or the conversation, and the turn continues normally. Retry rules of #85 are unchanged: the event counts as output, so a call is never retried after it and the notice is never sent twice for one call. |
| **N6** | `CachingProvider`: a response whose events contain `ModelFallback` is passed through unchanged but **not stored** (like one containing `Error`). A later identical request goes to the provider again. |
| **N7** | One-shot (`hone "<prompt>"`, with or without `--json`; and the `--orchestrate` / `--ensemble` printers): for each `AgentEvent::ModelFallback`, stderr gets `hone: <requested> failed (<reason>); switched to fallback <fallback>` on its own line, **once per (requested, fallback) pair per run**, however many model calls of the run fell back (orchestrator workers and ensemble candidates included — they share the channel). "On its own line": a leading newline only when the last thing the printer wrote to stdout did not end with a newline (worker progress lines written with `writeln!` end the line). The `--ensemble` printer stops printing tokens at the first `Done` as today, but keeps reading the channel until it closes so later candidates' notices are still printed. stdout (the answer, or the `--json` document) is unaffected. |
| **N8** | `--json` output gains `"fallbacks"`, always present: `[]` when no fallback took over; otherwise one object per (requested, fallback) pair in first-seen order: `{"requested": …, "fallback": …, "reason": <first reason seen for the pair>, "calls": <number of ModelFallback events for the pair>}`. Built by `json_mode_output` itself (it takes the run's fallback log), so its unit tests see the whole document. `"model"` keeps its meaning (the model the run chose). `"success"` is unaffected. No other key changes. |
| **N9** | TUI (`hone_tui::App`): `AgentEvent::ModelFallback` pushes one system message `<requested> failed (<reason>); switched to fallback <fallback>`, **once per (requested, fallback) pair per user turn**. The set of shown pairs is cleared when the user starts a turn (every turn-start path of `submit_input`) and by `cancel_turn`; **not** by `Done`/`Error`, because an orchestrated (Complex) turn sends a `Done` per worker. If the last message is a streaming assistant message, it stops streaming before the notice is pushed (no stuck cursor); tokens after the notice start a new assistant message. Nothing else in the status changes. |
| **N10** | TUI adaptive classifier (`record_outcome_for_event` in `bins/hone/src/main.rs`): a turn in which a `ModelFallback` arrived is recorded under the fallback model of the last such event, instead of the turn's model. The remembered model is cleared when a turn starts (next to `current_tier = Some(tier)`) and after `Done`/`Error`, so a cancelled turn (Esc sends neither) can't leak it into the next turn. The pure helpers are unit-tested; the loop is reviewed by reading. |
| **N11** | `honed`: the SSE / execution / orchestration event-name maps name the event `model_fallback` (not `unknown`). |
| **N12** | (Pre-existing, found in review; makes N10 and #87's complex routing true for the common case.) A plain Complex TUI turn (no `@agent`) runs the orchestrator with the turn's config (`base_config_clone`: the routed / pinned model, `/budget`, `/permit`), as the `@agent` Complex path already does, instead of the session's base config. Reviewed by reading. |

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

- The ensemble ranker and `--distributed` workers don't forward their events, so
  a fallback there isn't reported. (Orchestrator workers and ensemble
  candidates share the turn's channel, so theirs are.)
- A fallback member that fails at the *request* with an error that doesn't hand
  over (e.g. a 400) is returned as that error, with no notice naming the switch.
- `TierFallbackProvider` (unused by `hone`) emits nothing.
- Cost estimates still use the session's flat pricing for every model.
- honed provenance receipts record completions under the configured model;
  honed doesn't use `ModelDispatch` yet, but would misattribute once it does.
- honed has three copies of the event-name map (`sse.rs`, `execution.rs`,
  `orchestration.rs`).
- The error text of `AgentEvent::Error` itself is still printed unsanitized
  (and reqwest's URL, with any credentials, is in it).
