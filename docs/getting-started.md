# Getting Started with Hone

This comprehensive guide walks you through installing Hone, configuring your first AI provider, and running your first agent session. Whether you're a developer fixing bugs or architecting features, you'll be productive in minutes.

## Table of Contents

1. [Prerequisites](#1-prerequisites)
2. [Installation](#2-installation)
3. [First-Time Setup](#3-first-time-setup-hone-configure)
4. [Provider Setup](#4-provider-setup)
5. [Configuration Reference](#5-configuration-reference)
6. [Your First Task](#6-your-first-task)
7. [Session Management](#7-session-management)
8. [Modes of Operation](#8-modes-of-operation)
   - Shell Commands
   - Vim Keybindings
   - TDD Workflow
   - One-Shot Mode
   - Interactive Mode
   - Orchestrate Mode
   - Ensemble Mode
   - Recipe Mode
   - Agent Persona Mode
9. [MCP Extensions](#9-mcp-extensions)
10. [Remote Access (honed)](#10-remote-access-honed)
11. [Next Steps](#11-next-steps)
12. [Troubleshooting](#12-troubleshooting)

---

## 1. Prerequisites

Ensure your system has the following installed:

- **Rust 1.85+**: Check with `rustc --version`. If not installed, download from [rustup.rs](https://rustup.rs).
- **Cargo**: Comes bundled with Rust. Verify with `cargo --version`.
- **Git**: Required for cloning the repository and integration features. Verify with `git --version`.
- **macOS or Linux**: Hone is tested on macOS 12+ and Linux (Ubuntu 20.04+). Windows support is in progress.

### Platform-Specific Dependencies

**macOS:**
```bash
xcode-select --install
```

**Linux (Ubuntu/Debian):**
```bash
sudo apt-get update
sudo apt-get install build-essential pkg-config libssl-dev
```

**Optional:**
- **Docker** (for running SWE-bench or containerized workloads)
- **Node.js 18+** (for MCP JavaScript extensions)

---

## 2. Installation

### Clone and Build from Source

Clone the Hone repository and build the release binary:

```bash
git clone https://github.com/wojciechkpl/hone.git
cd hone
cargo build --release
```

The binary is located at `target/release/hone`. Copy it to your PATH:

```bash
# Option 1: Copy to a directory already in PATH
sudo cp target/release/hone /usr/local/bin/

# Option 2: Add the build directory to your shell profile
echo 'export PATH="$HOME/path/to/hone/target/release:$PATH"' >> ~/.zshrc
source ~/.zshrc
```

### Verify Installation

```bash
hone --version
```

Expected output:
```
hone 0.1.0
```

If you also intend to run the daemon server:

```bash
cargo build --release --bin honed
hone --version
honed --version
```

Both `hone` and `honed` should now be available in your PATH.

---

## 3. First-Time Setup (hone configure)

Before running your first session, complete the interactive configuration wizard:

```bash
hone configure
```

This command guides you through provider selection, API key entry, and model selection, writing everything to `~/.config/hone/config.toml`.

### Step-by-Step Walkthrough

Here's what the wizard looks like:

```
Welcome to the Hone configuration wizard.

Step 1: Select a provider

  1. Anthropic (Claude)
  2. OpenAI
  3. DeepSeek
  4. OpenRouter
  5. Ollama (local)
  6. Claude CLI (claude binary)

Enter your choice (1-6): 1
```

After selecting Anthropic (option 1):

```
Step 2: Enter your API key

Note: Your key is read from stdin and not echoed. It will be stored
securely in ~/.config/hone/config.toml.

Hint: sk-ant-...
API key: [hidden input]

Step 3: Select a default model

  1. claude-sonnet-4-5
  2. claude-opus-4-5
  3. claude-haiku-4-5
  4. claude-3-7-sonnet-latest

Enter your choice (1-4): 2
```

After model selection:

```
Configuration written to ~/.config/hone/config.toml
Provider: Anthropic
Model: claude-opus-4-5

Ready to use hone! Start with:
  hone
```

### What Gets Written to config.toml

After completing the wizard, your configuration file at `~/.config/hone/config.toml` looks like:

```toml
[agent]
default_model = "anthropic:claude-opus-4-5"
context_window_tokens = 200000
max_iterations = 50
temperature = 0.3

[memory]
enabled = true
encrypted = false

[providers.anthropic]
api_key = "sk-ant-..."
base_url = "https://api.anthropic.com"
timeout_secs = 30

[sandbox]
enforce = true
platform = "auto"
```

### Configuration Hierarchy (Priority Order)

Hone applies configuration in this order (top wins):

1. **CLI Flags**: `hone --model openai:gpt-4o --budget 10.00`
2. **Environment Variables**: `export ANTHROPIC_API_KEY=sk-ant-...`
3. **Config File**: `~/.config/hone/config.toml`
4. **Defaults**: Built-in defaults

Example: If both a CLI flag and env var are set, the CLI flag takes precedence.

---

## 4. Provider Setup

Hone supports multiple providers. Choose based on your needs: capability, cost, and privacy.

### Provider Comparison

| Provider | Tool Calling | Cost | Best For | Setup Time |
|----------|-------------|------|----------|------------|
| **Anthropic (Claude)** | Yes | $$ | Production, full features | 2 min |
| **OpenAI** | Yes | $ | Budget teams | 2 min |
| **DeepSeek** | Yes | $ | Cheap alternatives | 2 min |
| **OpenRouter** | Yes | $ | Multi-model access | 2 min |
| **Ollama** | No | Free | Local/offline work | 5 min |
| **Claude CLI** | No | Varies | Existing users | 1 min |
| **Gemini CLI** | No | Free tier | No setup needed | 1 min |

### Provider Auto-Detection Order

If no provider is explicitly specified, Hone checks in this order:

1. `ANTHROPIC_API_KEY` environment variable → Anthropic
2. `OPENAI_API_KEY` environment variable → OpenAI
3. `claude` command in PATH → Claude CLI
4. `gemini` command in PATH → Gemini CLI
5. `OLLAMA_HOST` environment variable → Ollama
6. Fallback → Mock provider (testing only)

---

### Anthropic (Recommended)

Full feature set including advanced tool calling and the 200K context window.

**Get your API key:**

1. Visit https://console.anthropic.com
2. Click "API Keys" → "Create Key"
3. Copy the key (starts with `sk-ant-`)

**Configure:**

```bash
export ANTHROPIC_API_KEY="sk-ant-abc123def456ghi789"
hone "hello"
```

Or using the configure wizard:

```bash
hone configure
# Choose: 1 (Anthropic)
# Paste API key when prompted
# Choose model: 2 (claude-opus-4-5)
```

**Verify it works:**

```bash
hone "What is 2 + 2?"
```

Expected output:
```
[Model: anthropic:claude-opus-4-5]
2 + 2 equals 4.

Usage: 34 input tokens, 12 output tokens
Cost: $0.001 USD
```

**Available models:**

- `claude-opus-4-5` — 200K context, best for complex reasoning
- `claude-sonnet-4` — 200K context, fast, balanced
- `claude-haiku-4-5` — 200K context, cheapest

**Pricing (per million tokens):**

- Opus 4-5: $3 input / $15 output
- Sonnet 4: $3 input / $15 output
- Haiku 4-5: $0.80 input / $4 output

---

### OpenAI

Cost-effective alternative with gpt-4o and gpt-4-turbo.

**Get your API key:**

1. Visit https://platform.openai.com
2. Go to "API Keys" → "Create new secret key"
3. Copy the key (starts with `sk-`)

**Configure:**

```bash
export OPENAI_API_KEY="sk-proj-abc123..."
hone --model openai:gpt-4o "hello"
```

Or via configure wizard:

```bash
hone configure
# Choose: 2 (OpenAI)
# Paste API key
# Choose model: 1 (gpt-4o)
```

**Available models:**

- `gpt-4o` — Latest, most capable
- `gpt-4o-mini` — Fast and cheap
- `gpt-4-turbo` — Previous generation
- `gpt-3.5-turbo` — Budget option

---

### DeepSeek (Cheapest)

Great for development and cost-conscious teams. ~90% cheaper than Anthropic.

**Get your API key:**

1. Visit https://platform.deepseek.com
2. Create API key

**Configure:**

```bash
export OPENAI_API_KEY="sk-..."
export HONE_OPENAI_BASE_URL="https://api.deepseek.com"
export HONE_OPENAI_MODEL="deepseek-chat"
hone "hello"
```

Or in config file:

```toml
[providers.openai]
api_key = "sk-..."
base_url = "https://api.deepseek.com"
model_override = "deepseek-chat"
```

**Available models:**

- `deepseek-chat` — General-purpose
- `deepseek-coder` — Code generation and analysis
- `deepseek-reasoner` — Extended reasoning

**Pricing:** ~$0.14 input / $0.28 output per million tokens (vs $3/$15 for Claude)

---

### OpenRouter (Multi-Model, Single Key)

Access 100+ models from a single API key. Great for comparing models.

**Get your API key:**

1. Visit https://openrouter.ai
2. Sign up and copy your API key

**Configure:**

```bash
export OPENAI_API_KEY="sk-or-abc123..."
export HONE_OPENAI_BASE_URL="https://openrouter.ai/api/v1"
export HONE_OPENAI_MODEL="anthropic/claude-opus-4-5"
hone "hello"
```

**Available models (examples):**

```
anthropic/claude-opus-4-5
openai/gpt-4o
google/gemini-2.5-pro
meta-llama/llama-3.3-70b-instruct
mistralai/mistral-large
```

See the full list at https://openrouter.ai/models

---

### Ollama (Free, Local)

Run open-source models locally. No internet, no API key, completely free.

**Install and start Ollama:**

```bash
# Download from https://ollama.ai

# Start the server (runs on localhost:11434)
ollama serve
```

In another terminal, pull and test a model:

```bash
ollama pull llama3.2
```

**Configure Hone:**

```bash
hone --model ollama:llama3.2 "hello"
```

Or add to config:

```toml
[providers.ollama]
base_url = "http://localhost:11434"
timeout_secs = 300
```

**Available models:**

- `llama3.2` — Latest, 8B
- `llama3.1` — Previous, 8B
- `mistral` — Fast, 7B
- `neural-chat` — Conversational
- `qwen2.5-coder` — Code generation

**List installed models:**

```bash
ollama list
```

**Pros:**

- Completely free
- Runs locally (privacy)
- No API key needed
- Works offline

**Cons:**

- Slower than cloud (depends on your GPU/CPU)
- Less capable than Claude Opus
- No tool calling support (yet)

---

### Claude CLI (Pass-Through)

Use your existing Claude CLI binary if you have it installed.

**Install Claude CLI first:**

```bash
# macOS (Homebrew)
brew install anthropic/brew/claude-cli

# Or download from https://github.com/anthropics/anthropic-sdk-python
```

**Verify Claude CLI is on PATH:**

```bash
claude --version
```

**Configure Hone:**

```bash
hone --model claude-cli:claude-opus "hello"
```

Or via configure wizard:

```bash
hone configure
# Choose: 6 (Claude CLI)
```

**Pros:**

- Uses your existing Claude CLI credentials
- No extra API key setup

**Cons:**

- No tool calling support
- Generation 1 models only
- Slower than API calls

---

### Gemini CLI (Pass-Through)

Use Google's Gemini CLI if you have it installed.

**Install Gemini CLI:**

```bash
# macOS
brew install google-cloud-sdk
gcloud init

# Or download: https://cloud.google.com/sdk/docs/install
```

**Verify installation:**

```bash
gemini --version
```

**Configure Hone:**

```bash
hone --model gemini-cli:gemini-pro "hello"
```

**Pros:**

- Free tier available
- No paid API setup needed

**Cons:**

- No tool calling support
- Less capable than Claude

---

### Provider Detection Flow

Hone automatically selects a provider unless you override it. Here's the decision tree:

```
┌─ Start hone session
│
├─ Is --model flag set?
│  ├─ YES → Use that provider
│  └─ NO → Check config.toml
│
├─ Is default_model in config.toml?
│  ├─ YES → Use that provider
│  └─ NO → Auto-detect
│
├─ Auto-detect flow:
│  ├─ Check ANTHROPIC_API_KEY env var?
│  │  ├─ YES → Use Anthropic
│  │  └─ NO → Continue
│  │
│  ├─ Check OPENAI_API_KEY env var?
│  │  ├─ YES → Use OpenAI
│  │  └─ NO → Continue
│  │
│  ├─ Check 'claude' command exists?
│  │  ├─ YES → Use Claude CLI
│  │  └─ NO → Continue
│  │
│  ├─ Check 'gemini' command exists?
│  │  ├─ YES → Use Gemini CLI
│  │  └─ NO → Continue
│  │
│  ├─ Check OLLAMA_HOST env var?
│  │  ├─ YES → Use Ollama
│  │  └─ NO → Fallback
│  │
│  └─ Fallback → Mock provider (testing)
│
└─ Session starts with selected provider
```

---

## 5. Configuration Reference

### Environment Variables

All environment variables in one place. CLI flags and config file override these.

| Variable | Purpose | Example |
|----------|---------|---------|
| `ANTHROPIC_API_KEY` | Anthropic API authentication | `sk-ant-abc123...` |
| `OPENAI_API_KEY` | OpenAI/compatible endpoint key | `sk-proj-abc123...` |
| `HONE_OPENAI_BASE_URL` | Override OpenAI endpoint | `https://api.deepseek.com` |
| `HONE_OPENAI_MODEL` | Override OpenAI model name | `deepseek-chat` |
| `OLLAMA_HOST` | Ollama server URL | `http://localhost:11434` |
| `RUST_LOG` | Logging verbosity | `debug`, `info`, `warn`, `error` |
| `HONE_CONFIG` | Override config file path | `~/.config/hone/config.toml` |
| `HONE_NO_INJECTION` | Disable context injection for this run | `1` |
| `HONE_INJECT_CONTEXT` | Explicitly enable context injection (already default) | `1` |

### Config File Format

Full annotated example at `~/.config/hone/config.toml`:

```toml
# ── Agent settings ────────────────────────────────────────────────────────

[agent]
# Default model to use when no --model flag is provided
default_model = "anthropic:claude-opus-4-5"

# Total context window in tokens (model-dependent)
context_window_tokens = 200000

# Tokens reserved for model output
reserved_output_tokens = 4000

# Tokens reserved for system prompt
reserved_system_prompt_tokens = 2000

# Temperature (0.0 = deterministic, 1.0 = creative)
temperature = 0.3

# Maximum iterations per session (prevents infinite loops)
max_iterations = 50

# Maximum tool calls per turn
max_tool_calls_per_turn = 10


# ── Context injection (enabled by default) ────────────────────────────────

[context_injection]
# Enable proactive context injection (default: true)
# Injects repo skeleton, high-confidence signatures, and exploration budgets
# Disable with --no-inject or HONE_NO_INJECTION=1
enabled = true

# BM25 confidence gap threshold for signature injection (default: 0.4)
# Lower = more aggressive, Higher = conservative
confidence_gap_threshold = 0.4

# Token budget for repository skeleton (default: 1500)
context_skeleton_budget = 1500

# Number of signatures to inject from top matches (default: 3)
signature_count = 3

# Enable exploration budget hints (default: true)
budget_hints_enabled = true


# ── Provider configuration ────────────────────────────────────────────────

[providers.anthropic]
api_key = "${ANTHROPIC_API_KEY}"  # or hardcoded: "sk-ant-..."
base_url = "https://api.anthropic.com"
timeout_secs = 30

[providers.openai]
api_key = "${OPENAI_API_KEY}"
base_url = "https://api.openai.com/v1"
timeout_secs = 30

[providers.ollama]
base_url = "http://localhost:11434"
timeout_secs = 300  # local models may be slow


# ── Memory and session persistence ────────────────────────────────────────

[memory]
enabled = true
db_path = "~/.local/share/hone/memory.db"
encrypted = false
decay_days = 90
max_items = 10000


# ── Sandbox security ──────────────────────────────────────────────────────

[sandbox]
enforce = true
platform = "auto"  # "seatbelt" (macOS), "landlock" (Linux), "seccomp"

[sandbox.defaults]
allow_read = ["{project_root}"]
allow_write = ["/tmp"]
allow_network = false

[sandbox.tools.shell]
allow_read = ["{project_root}", "/usr/lib"]
allow_write = ["/tmp", "{project_root}/target"]
allow_exec = ["/bin", "/usr/bin", "/usr/local/bin"]
```

### HONE.md Project Config

Create a `HONE.md` file in your project root to configure Hone per-project:

```yaml
# HONE.md
project_name: my-app
default_model: anthropic:claude-opus-4-5
context_window_tokens: 150000

# Tools to enable/disable
tools:
  shell: true
  git: true
  file: true
  lsp: true

# Sandbox policy
sandbox:
  enforce: true
  allow_read:
    - src/
    - tests/
    - Cargo.toml
  allow_write:
    - src/
    - tests/
    - target/

# Agent-specific instructions
agent_instructions: |
  You are a senior Rust engineer.
  - Always use 2-space indentation
  - Write tests alongside implementation
  - Check clippy warnings

# Cost budget for this project
budget_usd: 50.0
```

Place this in your repository root. Hone reads it on session start.

---

## 6. Your First Task

Now let's run Hone on a real task. We'll fix a simple bug and add a feature.

### Example 1: One-Shot (Quick Question)

Ask Hone a one-shot question without starting interactive mode:

```bash
hone "What files are in the current directory?"
```

**Output:**

```
I'll list the files in the current directory.

[Tool: shell]
Input: ls -la
Output:
total 32
drwxr-xr-x   8 user  staff   256 Apr  9 10:30 .
drwxr-xr-x   5 user  staff   160 Apr  9 10:30 ..
-rw-r--r--   1 user  staff  1234 Apr  9 10:30 Cargo.toml
-rw-r--r--   1 user  staff   567 Apr  9 10:30 README.md
drwxr-xr-x  10 user  staff   320 Apr  9 10:30 src

Found 5 entries: Cargo.toml, README.md, src directory, and 2 others.

Usage: 245 input tokens, 89 output tokens
Cost: $0.004 USD
```

### Example 2: Interactive Session (Multi-Turn)

Start an interactive session to work on a multi-step task:

```bash
hone --session my-project
```

**Terminal demo:**

```
Hone Session: my-project
Type prompts and press Enter. Type /help for commands.

You: Show me the main.rs file
Assistant: I'll read the main.rs file for you.

[Tool: read_file]
Input: {"path": "src/main.rs"}
Output: (file contents shown)

You: Add error handling to the parse function
Assistant: I'll add error handling to the parse function.

[Tool: edit_file]
Input: {"path": "src/main.rs", "old_text": "fn parse(s: &str) {", "new_text": "fn parse(s: &str) -> Result<()> {"}
Output: Edit successful

Usage: 1523 input tokens, 412 output tokens
Cost: $0.028 USD
Session total: $0.032 USD
Budget remaining: $9.97 / $10.00

You: /quit
Session my-project saved.
```

### What Happens Behind the Scenes

1. **Token Count**: Every turn shows input and output tokens
2. **Cost Tracking**: Real USD cost based on your provider's pricing
3. **Tool Calls**: File reads, edits, shell execution, git operations
4. **Context Optimization**: Hone automatically keeps relevant code in context
5. **Session Memory**: Previous turns are remembered and compressed when context gets large

---

## 7. Session Management

Sessions are named conversations that persist across runs. Start a named session once, resume it later—context is preserved.

### Create and List Sessions

```bash
# Start a new named session
hone --session debug-auth

# List all saved sessions
hone session list
```

**Output of `session list`:**

```
ID                              Created              Updated              Name              Messages
──────────────────────────────  ──────────────────  ──────────────────  ──────────────    ────────
550e8400-e29b-41d4-a716-4467   2026-04-09 15:22   2026-04-09 15:45    debug-auth        42
550e8400-e29b-41d4-a716-4468   2026-04-09 14:10   2026-04-09 14:10    my-project        5
550e8400-e29b-41d4-a716-4469   2026-04-08 18:30   2026-04-08 18:30    refactor-api      18
```

### Resume a Session

Pick up where you left off:

```bash
hone session resume debug-auth
```

All prior messages and context are restored. New turns are added to the session.

### Rename and Delete

```bash
# Rename a session
hone session rename debug-auth debug-authentication

# Delete a session (permanent)
hone session delete old-session
```

### Ephemeral Mode (No Persistence)

For sensitive work or testing, skip auto-save:

```bash
hone --ephemeral
```

The session runs but isn't saved after it exits. Useful for:

- Sensitive code review
- One-off debugging
- Cost-limited testing

### Session Workflow Example

```bash
# Day 1: Start a feature branch
hone --session feature/user-auth
# Conversation happens, auto-saved after each turn

# Day 2: Resume work
hone session list
# Find feature/user-auth in the list

hone session resume feature/user-auth
# Pick up right where you left off, full context restored

# Done, rename it
hone session rename feature/user-auth feature/user-auth-complete
```

---

## 8. Modes of Operation

Hone supports multiple modes. Choose the right one for your task.

### Shell Commands

Run native shell commands directly from Hone without exiting the TUI.

```bash
!ls                 # List files, output shown inline
!git status         # Check git status
!cargo test         # Run tests
!cd src             # Change directory (confined to project)
!nvim file.rs       # Open editor, suspend TUI, restore on exit
```

**Full Example:**

```
> !cargo test
  running 12 tests
  test test_add ... ok
  test test_subtract ... ok
  12 passed, 0 failed

  claude-sonnet │ $0.00 │ ▓▓▓░░░░░░░ 28% ctx │ !shell
```

**Features:**

- All standard shell commands supported
- Output captured and displayed inline
- Directory changes persist for the session
- `!cd` confined to project root (security)
- `!nvim`, `!vim`, `!vi`, or any `$EDITOR` suspend TUI and restore after exit

---

### Vim Keybindings

Enable vim-style keybindings for navigating and editing within Hone.

**Start vim mode:**

```bash
hone --vim
```

Or toggle in interactive mode:

```
:set vim      # Enable vim mode
:set novim    # Disable vim mode
```

**Normal Mode:**

```
j         Move down
k         Move up
h         Move left
l         Move right
gg        Jump to top
G         Jump to bottom
w         Jump to next word
b         Jump to previous word
i         Enter insert mode (before cursor)
a         Enter insert mode (after cursor)
o         Insert new line below
O         Insert new line above
d         Delete (dd = delete line)
y         Yank/copy (yy = copy line)
p         Paste
u         Undo
Ctrl-r    Redo
:q        Quit to previous view
:wq       Save and quit
```

**Insert Mode:**

```
Type normally to edit text
Esc       Return to normal mode
```

**Full Example:**

```
> :set vim
Vim mode enabled.

> gg              # Jump to top of input
> i               # Enter insert mode
> (type new prompt)
> Esc             # Return to normal mode
> :q              # Submit
```

---

### TDD Workflow

Watch source files and automatically run tests on change. Perfect for test-driven development.

**Start TDD mode:**

```
:tdd cargo test
```

Hone will now:
1. Watch all source files (300ms debounce)
2. Run `cargo test` when files change
3. Display PASS/FAIL inline
4. Repeat until you stop

**Stop TDD mode:**

```
:tdd off
```

**Full Example:**

```
> :tdd cargo test
  [TDD] watching for changes, running: cargo test

  ... (waiting for edits) ...

  [TDD] change detected: src/lib.rs — running tests...
  [TDD] tests PASSED
    12 passed in 0.4s

  [TDD] change detected: src/main.rs — running tests...
  [TDD] tests FAILED
    test test_parse ... FAILED (expected Ok, got Err)

  [TDD] change detected: src/main.rs — running tests...
  [TDD] tests PASSED
    12 passed in 0.5s
```

**Use Cases:**

- Red-Green-Refactor workflow
- Continuous verification while implementing
- Early detection of regressions
- Works with any test command: `cargo test`, `pytest`, `npm test`, etc.

---

### One-Shot Mode (Fast)

Run a single prompt and exit. No interactive TUI.

```bash
hone "List all Python files in src/"
```

**Use when:**

- Quick questions
- Scripting (output to stdout)
- Quick file exploration

**Output goes to stdout**, suitable for piping:

```bash
hone "Analyze error in this log" < error.log > analysis.txt
```

---

### Interactive Mode (Default)

Start a conversational session with the TUI.

```bash
hone
```

or

```bash
hone --session my-project
```

**Use when:**

- Multi-step tasks (exploration → implementation → testing)
- Iterative development
- Real-time feedback

**TUI features:**

- Type prompts freely
- Press `?` for help
- Type `/cmd` for commands
- Streaming tokens in real-time
- Cost tracking on each turn

**Commands in interactive mode:**

```
/model <provider:model>   — Switch models mid-session
/budget <usd>             — Set spending limit
/clear                    — Clear conversation history
/export <path>            — Save session to JSON
/help                     — Show all commands
/quit                     — Exit
```

---

### Orchestrate Mode (Complex Tasks)

Decompose complex tasks into parallel subtasks, executed by worker agents, then merged.

**Recommended for:** Features with multiple concerns (API + CLI + tests), complex refactors.

```bash
export ANTHROPIC_API_KEY="sk-ant-..."
hone --orchestrate "Add comprehensive email validation: write tests, implement validation logic, build CLI interface, document API"
```

**What happens:**

```
1. Orchestrator decomposes: test_suite, validation_impl, cli_build, api_docs
2. 4 workers execute in parallel
3. Results merged into coherent output
4. Final response shows all changes
```

**Output example:**

```
[Orchestrator] Decomposing task...

[PlanCreated]
Subtask 1: write_test_suite (worker 0)
Subtask 2: implement_validation (worker 1)
Subtask 3: build_cli_interface (worker 2)
Subtask 4: document_api (worker 3)

[WorkerStarted] Worker 0: write_test_suite
[WorkerStarted] Worker 1: implement_validation
[WorkerStarted] Worker 2: build_cli_interface
[WorkerStarted] Worker 3: document_api

[WorkerCompleted] Worker 0: test_suite.rs (450 tokens)
[WorkerCompleted] Worker 1: validation.rs (1200 tokens)
[WorkerCompleted] Worker 2: cli.rs (890 tokens)
[WorkerCompleted] Worker 3: docs.md (340 tokens)

[MergingResults]
Combining all outputs...

[Final Output]
Complete email validation system with tests, logic, CLI, and docs.

Total tokens: 4521 input, 892 output
Total cost: $0.084 USD
```

---

### Ensemble Mode (Ranking Best Response)

Run N parallel candidates with varied temperatures and automatically rank the best.

```bash
hone --ensemble 3 "Implement a binary search function"
```

**What happens:**

```
1. Run 3 workers with T=0.1, T=0.5, T=0.8
2. Each produces a different response
3. Ranker evaluates all 3
4. Returns the best one
```

**Use when:**

- You want the best result, not just any result
- Task quality matters more than speed
- Budget allows 3x API calls

---

### Recipe Mode (Structured Workflows)

Run a pre-defined workflow (TDD, bug-fix, code-review, refactor).

```bash
hone --recipe tdd-feature
```

**Built-in recipes:**

- `tdd-feature` — Write tests first, then implementation
- `bug-fix` — Root cause analysis, fix, test
- `code-review` — Systematic code review with checklist
- `refactor` — Analyze, plan, implement, verify

Each recipe is a YAML workflow defining the steps, prompts, and validation rules.

---

### Agent Persona Mode

Switch to a specialized agent persona.

```bash
hone --agent debugger "There's a segfault in main"
hone --agent security-auditor "Review this authentication module"
hone --agent code-reviewer "Review the PR changes"
```

**Available personas:**

- `code-reviewer` — Code quality, style, testing
- `debugger` — Debugging, root cause analysis
- `security-auditor` — Security issues, vulnerabilities
- `test-architect` — Test design and coverage
- `ml-recommendations-expert` — ML/AI algorithm design
- `ai-researcher` — Research, papers, novel ideas
- `python-expert` — Python-specific improvements
- `rust-expert` — Rust idioms and performance

---

### Decision Guide: Which Mode for Which Task?

| Task | Mode | Command |
|------|------|---------|
| Quick Q&A | One-shot | `hone "What is..."` |
| Multi-turn debugging | Interactive | `hone --session debug` |
| Complex feature (API + tests + CLI) | Orchestrate | `hone --orchestrate "..."` |
| Choose best of 3 solutions | Ensemble | `hone --ensemble 3 "..."` |
| TDD workflow | Recipe | `hone --recipe tdd-feature` |
| Code review focus | Agent | `hone --agent code-reviewer` |
| Complex reasoning problem | Agent | `hone --agent ai-researcher` |

---

## 9. MCP Extensions

Model Context Protocol (MCP) servers extend Hone with external tools: GitHub, Slack, databases, custom integrations.

### What is MCP?

MCP is a protocol for AI agents to call external services. Instead of hardcoding GitHub, Slack, and database tools into Hone, we use MCP servers as plugins.

**Example:** With the GitHub MCP server, Hone can:

- List open PRs
- Create issues
- Add comments
- Merge PRs
- Query workflows

---

### Quick Setup

Create `.hone/mcp.json` in your project:

```json
{
  "mcp_servers": [
    {
      "name": "github",
      "command": "npx",
      "args": ["@modelcontextprotocol/server-github"],
      "env": {
        "GITHUB_TOKEN": "${GITHUB_TOKEN}"
      }
    },
    {
      "name": "slack",
      "command": "npx",
      "args": ["@modelcontextprotocol/server-slack"],
      "env": {
        "SLACK_BOT_TOKEN": "${SLACK_BOT_TOKEN}"
      }
    }
  ]
}
```

Set environment variables:

```bash
export GITHUB_TOKEN="ghp_..."
export SLACK_BOT_TOKEN="xoxb-..."
hone
```

Hone now has GitHub and Slack available as tools.

---

### Available MCP Servers

See the [MCP Extensions Guide](tutorials/mcp-extensions.md) for a complete list and setup instructions.

---

## 10. Remote Access (honed)

Run Hone as a persistent daemon server accessible over the network.

### Start the Daemon

```bash
honed 7070  # Listen on port 7070
```

**With auth token:**

```bash
export HONE_AUTH_TOKEN="sk-hone-abc123def456..."
honed 7070
```

**With custom session database path:**

```bash
export HONE_SESSION_DB="$HOME/.local/share/hone/sessions.db"
honed 7070
```

**Daemon output:**

```
honed 0.1.0

Listening on http://0.0.0.0:7070
Auth enabled: true
Session storage: SQLite at /Users/user/.local/share/hone/sessions.db
```

### Connect from Another Machine

```bash
curl -H "Authorization: Bearer sk-hone-abc123..." \
  https://remote-hone-server.example.com:7070/health
```

### API Endpoints

| Method | Endpoint | Purpose |
|--------|----------|---------|
| `POST` | `/sessions` | Create a new session |
| `GET` | `/sessions/{id}` | Get session info |
| `DELETE` | `/sessions/{id}` | Delete a session |
| `GET` | `/health` | Health check |
| `GET` | `/openapi.json` | OpenAPI spec |

See [Server & Remote Guide](tutorials/server-and-remote.md) for full details.

---

## 11. Next Steps

You've completed the Getting Started guide. Here's where to go next:

### Learn More

- **[Providers Guide](tutorials/providers.md)** — Hot-swap providers, fallback chains, pricing tables, custom endpoints
- **[Configuration Reference](configuration.md)** — All config options, environment variables, TOML format
- **[Architecture Guide](architecture.md)** — How Hone works: context optimization, memory, sandbox, multi-agent orchestration
- **[Tools & MCP Guide](tutorials/tools-and-mcp.md)** — Available tools, integrations, MCP extensions
- **[Security Guide](tutorials/security.md)** — Sandbox policies, audit logging, memory encryption
- **[Server & Remote Guide](tutorials/server-and-remote.md)** — Running honed daemon, team setup, RBAC
- **[Recipes Guide](tutorials/recipes.md)** — Pre-built workflows for common tasks

### Use Cases

- **Fix a bug:** `hone --agent debugger "The error is..."`
- **Add a feature:** `hone --orchestrate "Add feature X with tests"`
- **Review code:** `hone --agent code-reviewer --session pr-review`
- **Understand codebase:** `hone --session architecture "How does X work?"`
- **Write tests:** `hone --recipe tdd-feature`

### Community

- Bug reports: https://github.com/wojciechkpl/hone/issues
- Discussions: https://github.com/wojciechkpl/hone/discussions
- Documentation: https://wojciechkpl.github.io/hone/

---

## 12. Troubleshooting

### API Key Not Found

**Error:**

```
error: ANTHROPIC_API_KEY not set
```

**Solution:**

Set the environment variable:

```bash
export ANTHROPIC_API_KEY="sk-ant-..."
hone
```

Or add to `~/.config/hone/config.toml`:

```toml
[providers.anthropic]
api_key = "sk-ant-..."
```

Verify:

```bash
echo $ANTHROPIC_API_KEY
```

---

### Model Not Found

**Error:**

```
error: model 'anthropic:claude-unknown' not supported
```

**Solution:**

List available models:

```bash
hone --list-models
```

Use a valid model name:

```bash
hone --model anthropic:claude-opus-4-5 "hello"
```

---

### Network Error or Timeout

**Error:**

```
error: failed to connect to Anthropic API (timeout after 30s)
```

**Solutions:**

1. Check internet connection:
   ```bash
   curl https://api.anthropic.com
   ```

2. Check firewall/proxy:
   ```toml
   [http]
   proxy = "http://proxy.example.com:8080"
   ```

3. Increase timeout:
   ```bash
   hone --timeout 60
   ```

4. Check API status: https://status.anthropic.com

---

### Sandbox Policy Violation

**Error:**

```
error: sandbox policy denied access to /secret/data
```

**Explanation:**

Hone's sandbox prevents unauthorized file access for security. The file path isn't in the allowlist.

**Solution:**

Add the path to `~/.config/hone/config.toml`:

```toml
[sandbox.defaults]
allow_read = ["{project_root}", "/secret/data"]
```

---

### Out of Memory / Context Exceeded

**Error:**

```
error: context window exceeded (64k > 200k limit)
```

**Solution:**

Clear the session history:

```
/clear
```

Or reduce context limit:

```bash
hone --context-limit 32000
```

---

### Configuration File Not Found

**Error:**

```
error: config file not found at ~/.config/hone/config.toml
```

**Solution:**

Create the directory and file:

```bash
mkdir -p ~/.config/hone
hone configure
```

---

### Ollama Connection Failed

**Error:**

```
error: failed to connect to Ollama at http://localhost:11434
```

**Solutions:**

1. Start Ollama server:
   ```bash
   ollama serve
   ```

2. Check it's running:
   ```bash
   curl http://localhost:11434/api/tags
   ```

3. Verify models are installed:
   ```bash
   ollama list
   ```

4. If using custom host:
   ```bash
   export OLLAMA_HOST="http://192.168.1.10:11434"
   hone --model ollama:llama3.2
   ```

---

### Sessions Database Issues

**Error:**

```
error: failed to open sessions database
```

**Solutions:**

1. Ensure directory exists:
   ```bash
   mkdir -p ~/.local/share/hone
   ```

2. Check permissions:
   ```bash
   ls -la ~/.local/share/hone/
   ```

3. Repair database:
   ```bash
   rm ~/.local/share/hone/sessions.db
   hone session list  # Creates fresh database
   ```

---

### Still Need Help?

- Check the [Architecture Guide](architecture.md) for deep dives
- Search [GitHub Issues](https://github.com/wojciechkpl/hone/issues)
- Ask in [GitHub Discussions](https://github.com/wojciechkpl/hone/discussions)
- Read [Security Guide](tutorials/security.md) for security-related questions
