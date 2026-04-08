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

# Show the help overlay
F1
Sleep 3s
F1
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

# Quit cleanly
Type "/quit"
Enter
Sleep 1s
