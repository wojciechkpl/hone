# Read-before-edit guard: scan its read and stop inventing tool ids — spec

**Date:** 2026-11-09 · **Branch:** `fix/guard-read-injection-scan` · **Found by:** PR #79 review

## 1. The problems

When the model edits a file it hasn't read this turn, the guard (`agent.rs`, "Guard 1:
Must-read-before-edit") calls `read_file` itself and adds the content to the history as a
tool-result message with a **made-up id** (`guard-read-<uuid>`). Then the edit runs as usual.

1. **No injection scan, no audit entry.** Successful results go through
   `scan_for_injection` (metric, egress risk label, SECURITY prefix) and an egress entry. The guard's
   read gets neither, although repository files can be attacker-written. (Redaction does apply:
   `tool_result_message(redact, …)`.)
2. **An id the model never issued.** The Anthropic adapter turns every tool message into a
   `tool_result` block with that `tool_use_id`; OpenAI-compatible adapters send it as `tool_call_id`.
   Both APIs document that a tool result must answer a tool call in the preceding assistant message,
   so this likely makes the next request fail (unverified live: no API key here; local vLLM does not
   validate ids). The guard's read tracking resets every turn, so in multi-turn sessions it fires
   often.

```mermaid
%%{init: {"themeVariables": {"fontSize": "13px"}, "flowchart": {"nodeSpacing": 25, "rankSpacing": 30, "padding": 8}, "sequence": {"actorMargin": 30, "messageMargin": 25, "boxMargin": 6}, "gantt": {"barHeight": 16, "barGap": 3}}}%%
flowchart LR
  A["assistant: tool_call edit_file (id c1)"] --> G["guard reads the file"]
  G -->|today| X["tool msg id guard-read-uuid<br/>(no matching call; unscanned)"]
  G -->|fix| Y["edit c1's own result:<br/>scanned + redacted read, then edit result"]
```

## 2. Contract

| # | Requirement | Test |
|---|---|---|
| G1 | **No invented ids.** The guard never adds a tool-role message whose id the model didn't issue. Its read is delivered inside the **edit call's own tool result**, as a leading block (`(guard: auto-read '<path>' before edit)` + content), on every path where that call gets a result in this iteration (ran, tool error, denied, skipped) | Model edits an unread file → in the provider's next request, every tool-role message's id is one of the tool-call ids of the preceding assistant message, and the edit call's result contains the file content |
| G2 | **Injection scan.** The guard's read goes through `scan_for_injection("read_file", …)`: metric, SECURITY prefix in front of the read block when flagged | File with injection-like text → the edit's result carries the SECURITY prefix before the read content |
| G3 | **Redaction** (unchanged in effect, now on the folded text): a secret in the file is redacted in what the model receives | File containing a secret (built at runtime) → not present in the next request |
| G4 | **Audit.** The guard's read records an egress entry (`tool: read_file`, preview of the redacted content, injection label) | `--audit`-style egress log contains the guard read with the label when flagged |

Unchanged: the guard still runs after hooks and the policy gate and only reads when the policy
allows it; a failed guard read is still skipped silently; the edit runs as before.

## 3. Out of scope

Running the guard's read through `PreToolUse`/`PostToolUse` hooks (none are loaded by any binary).
