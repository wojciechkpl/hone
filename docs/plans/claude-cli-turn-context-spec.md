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

## Testing notes

- Use the existing stub-binary harness in the claude-cli tests, which records argv and the NDJSON written to stdin.
- **CC1/CC2:** turn 1 with base, memory A and skill S; turn 2 with base, memory B and S. Check `--system-prompt` is the base only, turn 1's user payload has a block with A and S, and turn 2's has a block with B only (S already sent, CC3).
- **CC4:** a changed base prompt on turn 2 gives a re-spawn with the new prompt, and a notice.
- **CC3 after death:** after a re-spawn caused by death, all non-base messages are sent again.

## Out of scope

- Splitting the base prompt itself.
- Other CLI adapters (`gemini-cli:`); note any that share the pattern.
