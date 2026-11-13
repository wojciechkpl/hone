# claude-cli: each turn's context reaches the model: spec

Branch `fix/claude-cli-turn-context`, off `main`. Found while reviewing the Anthropic system-message fix (#106).

## Problem

The `claude-cli:` adapter keeps one `claude` process for the whole session, and sends only the last user message on each turn. It joins all of turn 1's system messages into `--system-prompt` when it spawns, and never updates them. Hone's system messages after the first change from turn to turn: recalled memories, matched skills, gathered context, and the memory index. So with `claude-cli:` the model sees turn 1's memories and skills for the rest of the session, and never sees later ones.

## Rows

| Row | Behaviour |
|---|---|
| CC1 | **Only the base prompt is fixed at spawn.** `--system-prompt` carries only the first system message of the turn that spawns the process (Hone's base prompt). |
| CC2 | **Turn context goes with the turn.** Every other system message of a turn is sent with that turn's user text, in one delimited block before it. The block starts with a line saying it is context from Hone for this turn, and each message is kept whole and in order. On turn N the model sees turn N's memories and skills. |
| CC3 | **Nothing is repeated.** A system message whose exact text was already sent by this process (at spawn or in an earlier turn's block) isn't sent again. When a turn has nothing new, it is sent without a block. After the process is re-spawned (it died), the dedupe resets and everything is sent again. |
| CC4 | **A changed base prompt restarts the session.** If a turn's first system message differs from the one the process was spawned with, the process is re-spawned with the new base prompt, and the dedupe resets. Hone logs a warning (`tracing::warn!`) that the `claude` session was restarted, so earlier turns aren't in its memory. It doesn't print to stderr, because under the TUI that would garble the screen. |

### Review round 1 (CC5–CC8)

CC1–CC3 work. CC4 restarted `claude` far more often than assumed. One provider instance also serves orchestrator workers (persona plus `## Task`) and inline `@persona` turns (`set_system_prompt`). In a 5-prompt TUI session it spawned 5 times against 1 on main. Each restart wiped the conversation, and nothing told the user. The context block's end marker could also be forged by a memory or skill. And deduplication assumed `claude` keeps everything, but it compacts its own history.

| Row | Behaviour |
|---|---|
| CC5 | **One process per base prompt.** The adapter keeps up to 4 `claude` processes, keyed by base prompt (the first system message), each with its own deduplication set. When a fifth base prompt arrives, the least recently used process is evicted. A turn with a different base prompt uses or spawns its own process, so the main conversation's process survives orchestrator-worker and `@persona` turns, and workers no longer write into the main conversation. This replaces CC4's restart. A persona's process doesn't see earlier turns of the main conversation; the PR and `docs/configuration.md` say so. |
| CC6 | **The context block can't be forged.** The block's opening and closing markers carry a random nonce, generated per process and never derived from content, for example `[Hone context 3f9a1c…: …]` and `[End of Hone context 3f9a1c…]`. The opening line says only the matching end marker closes the block. A context text that contains the exact current end marker has it neutralized before sending. |
| CC7 | **Context is sent again after `claude` compacts.** When `claude`'s output stream reports a compaction (a `system` event with subtype `compact_boundary`), that process's deduplication set is cleared, so the next turn sends every context message again. |
| CC8 | **Small fixes.** |
|  | - The base prompt is always the first system message, even when it's empty (then no `--system-prompt` flag is sent), so an empty persona can't promote the memories block into `--system-prompt`. |
|  | - The deduplication set stores SHA-256 hashes, not full texts. |
|  | - The `with_binary` doc comment and the spawn-failure error text describe the current behaviour. |

### Review round 2 (CC9–CC10)

The pool works for workers and personas (3 spawns instead of 5, and turn 5 reaches the process that saw turn 1). But the orchestrator runs up to 4 workers at once, each with a unique base prompt (the persona plus `## Task …`). The main conversation sits idle while the lead waits, so it is the least recently used: one 4-subtask orchestration evicted and killed it.

| Row | Behaviour |
|---|---|
| CC9 | **The main conversation is never evicted.** The first base prompt the provider sees is the main conversation, and its process is never evicted. When the pool is full, the least recently used of the other processes is evicted. Main plus 4 or more other bases in a burst leaves main's process alive, and the next main turn goes to it. |
| CC10 | **Idle side processes are closed.** A process other than main's that has been idle for more than 5 minutes is closed the next time the pool is used, so finished workers don't keep `claude` processes alive. A process mid-turn is never closed. `docs/configuration.md` describes the pool, CC9 and CC10. |

### Review round 3 (CC11)

Approved: main survives 4-subtask and 6-subtask orchestrations, and idle closing skips a mid-turn worker. One should-fix remains. A session whose first prompt is an `@persona` turn makes the persona "main". The real conversation is then a side process that can be evicted or closed after 5 minutes idle.

| Row | Behaviour |
|---|---|
| CC11 | **Use decides what's protected, not order.** A process that has served two or more turns is protected like main's: it is never closed for idleness. When the pool is full, eviction picks among the processes with the fewest turns, least recently used first. A session that starts with an `@persona` turn, then continues with normal turns, keeps the normal conversation's process through a pause longer than the idle timeout and through a burst of worker bases. |

## Testing notes

- Use the existing stub-binary harness in the claude-cli tests, which records argv and the NDJSON written to stdin.
- **CC1/CC2:** turn 1 with base, memory A and skill S; turn 2 with base, memory B and S. Check `--system-prompt` is the base only, turn 1's user payload has a block with A and S, and turn 2's has a block with B only (S already sent, CC3).
- **CC4:** a changed base prompt on turn 2 gives a re-spawn with the new prompt, and a notice.
- **CC3 after death:** after a re-spawn caused by death, all non-base messages are sent again.

## Out of scope

- Splitting the base prompt itself.
- Other CLI adapters (`gemini-cli:`); note any that share the pattern.
