# Every system message reaches Anthropic models: spec

Branch `fix/anthropic-system-messages`, off `main`. Found while reviewing PR E (env-snapshot).

## Problem

The Anthropic adapter (`crates/hone-providers/src/adapters/anthropic.rs`, `build_request_body`) takes only the **first** system message (`.find(|m| m.role == Role::System)`) as the top-level `system` field, and `convert_message` drops every system message from `messages`. Hone sends several system messages:
- "Relevant Memories" (per-turn recall; #101 made it work);
- the skills block;
- the `[session history: … compressed]` marker after trimming;
- since the receipts and strategy PRs, environment and retrieval blocks.

With an `anthropic:` model none of these reaches the model, while receipts record them as sent. Reproduced in review: request roles `[System, System, User]`, the body's `system` holds only the base prompt, and `# Environment` is absent. The OpenAI and Ollama adapters send each system message; the Claude Code CLI adapter joins them; the Gemini CLI adapter labels each `SYSTEM`.

## Rows

| Row | Behaviour |
|---|---|
| AS1 | **All system messages are sent, in order.** `build_request_body` puts every system message of the request, in request order, into the top-level `system` array as text blocks (one block per message). No system message is dropped, and none appears in `messages`. A request with no system message has no `system` field, as today. |
| AS2 | **Prompt caching stays effective.** The first system block (the stable base prompt) keeps its `cache_control` breakpoint. When there are two or more system blocks, the last one also gets `cache_control`, so every tool-loop call within a turn reuses the whole system prefix. The total number of breakpoints in a request never exceeds Anthropic's limit of 4 (tools, first system block, last system block, conversation). |
| AS3 | **A system message from mid-history is still delivered.** A system message that sits after user or assistant messages (the trim marker) is delivered as a system block in its request order relative to the other system messages, and the conversation's user/assistant alternation is unaffected. |
| AS4 | **Images and empty text are handled.** A system message whose content is empty after trimming is skipped; non-text content in a system message (none today) is sent as its text form. |

## Testing notes

- **AS1–AS4** as unit tests on `build_request_body` with requests holding 0, 1, 3 system messages, one mid-history, and with tools (count `cache_control` occurrences across tools, system and messages).
- One end-to-end check through a real `Agent` with an `InMemoryStore` memory and a mock Anthropic SSE server: the request body's `system` contains "Relevant Memories".

## Out of scope

- Moving per-turn blocks out of the system prompt.
