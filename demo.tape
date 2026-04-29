# Hone Demo — run with: vhs demo.tape
# Install VHS: brew install vhs
#
# Showcases the major features of Hone v0.1:
# - One-shot CLI mode
# - Recipe listing and execution
# - Multi-agent orchestration
# - Agent ensemble (MASAI Ranker)
# - Interactive TUI with themes, split pane, autocomplete
# - Slash commands
# - Agent Rail observability sidebar (Ctrl+R)
# - Live /model switch with context-limit update
# - Accessibility flags (HONE_ASCII_ONLY, HONE_REDUCED_MOTION, NO_COLOR)
# - Session reports and hone reports CLI
#
# Not directly demoable (fire only on failure modes) but present in the
# agent loop and visible in session-report timelines when triggered:
# - Doom-loop detector — corrective prompt on 3+ identical tool calls
# - Context-budget graceful abort — strips tools at 95 % of context
# - Output-truncation retry — handles stop_reason=max_tokens / length

Output demo.gif
Output demo.mp4

Set Shell "bash"
Set FontSize 14
Set Width 1280
Set Height 720
Set Theme "Catppuccin Mocha"
Set Padding 20
Set TypingSpeed 50ms

# ─────────────────────────────────────────────────────────
# Section 1: CLI flags and one-shot mode
# ─────────────────────────────────────────────────────────

Type "hone --version"
Enter
Sleep 1s

Type "hone --list-recipes"
Enter
Sleep 2s

# One-shot mode (auto-detected provider streams the response)
Type "hone 'What is a binary search tree in one sentence?'"
Enter
Sleep 8s

# ─────────────────────────────────────────────────────────
# Section 2: Multi-agent orchestrator
# ─────────────────────────────────────────────────────────

# The orchestrator decomposes the request into specialist subtasks
Type "hone --orchestrate 'plan a small Rust CLI tool to count words in stdin'"
Enter
Sleep 15s

# ─────────────────────────────────────────────────────────
# Section 3: Agent ensemble (MASAI Ranker pattern)
# ─────────────────────────────────────────────────────────

# Run 3 parallel candidates at different temperatures, ranker picks best
Type "hone --ensemble 3 'write a one-line Rust closure that doubles a number'"
Enter
Sleep 20s

# ─────────────────────────────────────────────────────────
# Section 4: Interactive TUI
# ─────────────────────────────────────────────────────────

Type "hone"
Enter
# Wait for splash to dismiss
Sleep 3s

# Send a message
Type "Explain what tree-sitter is in 2 sentences"
Sleep 500ms
Enter
Sleep 12s

# Show the help overlay (slash command)
Type "/help"
Sleep 500ms
Enter
Sleep 3s
Escape
Sleep 500ms

# Slash commands
Type "/cost"
Sleep 500ms
Enter
Sleep 2s

# Demonstrate autocomplete by typing /th
Type "/th"
Sleep 2s
Backspace
Backspace
Backspace
Type "/theme neon"
Sleep 500ms
Enter
Sleep 2s

Type "/theme crimson"
Sleep 500ms
Enter
Sleep 1s

# Toggle split pane
Type "/split"
Sleep 500ms
Enter
Sleep 2s

Type "/split"
Sleep 500ms
Enter
Sleep 1s

# Theme preview
Type "/theme-preview"
Sleep 500ms
Enter
Sleep 3s

# Multi-turn: ask a follow-up
Type "now show me a basic example in Rust"
Sleep 500ms
Enter
Sleep 12s

# Show /cost again with the cumulative tally
Type "/cost"
Sleep 500ms
Enter
Sleep 2s

# ─────────────────────────────────────────────────────────
# Section 5: Agent Rail observability sidebar
# ─────────────────────────────────────────────────────────

# Terminal must be ≥100 columns wide for the Agent Rail to appear
# Ctrl+R toggles the 18-column right sidebar

Type "read the auth module and explain the flow"
Sleep 500ms
Enter
Sleep 10s

# Toggle Agent Rail on — shows Files, Context, Memory sections
Ctrl+r
Sleep 3s

# Toggle Agent Rail off
Ctrl+r
Sleep 1s

# ─────────────────────────────────────────────────────────
# Section 6: Live /model switch
# ─────────────────────────────────────────────────────────

# /model lists available models, switch updates the agent AND the context bar
Type "/model"
Sleep 500ms
Enter
Sleep 2s

Type "/model anthropic:claude-opus-4-6"
Sleep 500ms
Enter
Sleep 2s

# Status bar now shows "claude-opus-4-6 │ … 0% (0/200k)"
Sleep 1s

# ─────────────────────────────────────────────────────────
# Section 7: Accessibility flags
# ─────────────────────────────────────────────────────────

# Quit and relaunch with HONE_ASCII_ONLY=1
Type "/quit"
Enter
Sleep 1s

Type "HONE_ASCII_ONLY=1 hone"
Enter
Sleep 3s

Type "/quit"
Enter
Sleep 1s

# HONE_REDUCED_MOTION=1 — static spinner instead of animation frames
Type "HONE_REDUCED_MOTION=1 hone"
Enter
Sleep 3s

Type "/quit"
Enter
Sleep 1s

# ─────────────────────────────────────────────────────────
# Section 8: Session reports
# ─────────────────────────────────────────────────────────

# Reports are written automatically on /quit to .hone/reports/
Type "hone reports list"
Enter
Sleep 2s

Type "hone reports show 2026"
Enter
Sleep 2s

Type "hone reports diff"
Enter
Sleep 2s

# Quit cleanly
Type "/quit"
Enter
Sleep 1s
