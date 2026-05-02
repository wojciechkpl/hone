# Agent Rail Demo — render with: vhs docs/tutorials/agent-rail.tape
#
# This tape produces docs/assets/agent-rail-demo.gif. It deliberately uses
# the *empty-state* rail (no provider call) so it renders identically on any
# machine without an ANTHROPIC_API_KEY or claude CLI configured.
#
# Recorded sequence (~22 s):
#   1. Launch hone — rail OFF by default, just the conversation pane
#   2. Ctrl+R    — rail ON, crimson, all four sections visible (em-dashes)
#   3. /theme minimal — same rail rendered in ASCII (NO_COLOR-equivalent)
#   4. /theme crimson — back to the default look
#   5. Ctrl+R    — rail OFF again, full-width pane returns
#   6. /quit
#
# Prerequisites:
#   - vhs installed (brew install vhs)
#   - hone binary on PATH (cargo install --path bins/hone --force, or
#     export PATH="$PWD/target/release:$PATH" after `cargo build --release`)
#
# To record the *populated* rail (real [FILES]/[AGENTS]/[CONTEXT]/[MEMORY]
# data) instead of the empty-state walkthrough, first export
# ANTHROPIC_API_KEY (or configure ~/.config/hone/config.toml) and replace
# step 2 with a Type "..." prompt that touches files. The empty-state arc
# is preferred for the shipped docs because it is reproducible.

Output docs/assets/agent-rail-demo.gif

Require hone

Set Shell "bash"
Set FontSize 13
# 1400 px is comfortably above the 100-col rail threshold (RAIL_MIN_WIDTH = 100).
Set Width 1400
Set Height 720
Set Theme "Builtin Dark"
Set Padding 20
Set TypingSpeed 80ms

# ─────────────────────────────────────────────────────────
# 1. Launch hone — rail OFF by default.
# ─────────────────────────────────────────────────────────

Type "hone"
Enter
Sleep 4500ms

# ─────────────────────────────────────────────────────────
# 2. Ctrl+R turns the rail ON (crimson theme).
#    Sections shown: [FILES] Waiting…  [AGENTS] —  [CONTEXT] —  [MEMORY] —
# ─────────────────────────────────────────────────────────

Ctrl+r
Sleep 4s

# ─────────────────────────────────────────────────────────
# 3. Switch to the minimal theme — same rail with ASCII borders & glyphs.
# ─────────────────────────────────────────────────────────

Type "/theme minimal"
Sleep 400ms
Enter
Sleep 4500ms

# ─────────────────────────────────────────────────────────
# 4. Switch back to crimson — show the default look again.
# ─────────────────────────────────────────────────────────

Type "/theme crimson"
Sleep 400ms
Enter
Sleep 3500ms

# ─────────────────────────────────────────────────────────
# 5. Ctrl+R turns the rail OFF — full-width conversation pane returns.
#    Note: vhs can drop consecutive Ctrl-key presses if they are too close
#    together, so this second toggle is well-separated from step 2 by the
#    intervening /theme commands. Aim for ≥3 s between Ctrl-key events.
# ─────────────────────────────────────────────────────────

Ctrl+r
Sleep 2500ms

# ─────────────────────────────────────────────────────────
# 6. Quit cleanly. Short tail — viewer doesn't need post-exit shell output.
# ─────────────────────────────────────────────────────────

Type "/quit"
Enter
Sleep 600ms
