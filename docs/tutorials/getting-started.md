# Getting Started with Hone

This guide walks you through setting up Hone, configuring your first model provider, and running your first agent session.

## Prerequisites

- **Rust 1.85+**: Check with `rustc --version`. Install from https://rustup.rs if needed.
- **Cargo**: Comes with Rust. Verify with `cargo --version`.
- **Git**: For cloning and version control features. `git --version` to verify.
- **System dependencies** (varies by platform):
  - **macOS**: XCode Command Line Tools (`xcode-select --install`)
  - **Linux**: `build-essential`, `pkg-config`, `libssl-dev` (Ubuntu/Debian: `sudo apt-get install build-essential pkg-config libssl-dev`)
  - **Windows**: Microsoft C++ Build Tools

## Building from Source

Clone the Hone repository and build:

```bash
git clone https://github.com/anthropics/hone.git
cd hone
cargo build --release
```

The binary is at `target/release/hone`. Add it to your PATH or use the full path:

```bash
export PATH="$PWD/target/release:$PATH"
hone --version
```

## First Run: Configure

Before starting your first session, run the interactive configuration wizard:

```bash
hone configure
```

This guides you step-by-step through:
1. Selecting a model provider (Anthropic, OpenAI, DeepSeek, OpenRouter, Ollama, etc.)
2. Entering your API key (securely via stdin, not command-line args)
3. Choosing your default model from that provider
4. Writing the configuration to `~/.config/hone/config.toml`

Configuration is one-time. Your provider and model are remembered for future sessions.

## Starting Your First Session

After configuration, start an interactive session:

```bash
hone
```

This launches the terminal UI. You can:
- Type prompts and press Enter
- Press `?` at any time for help
- Type `Ctrl+C` to quit

Your session is automatically saved with a default name. You can resume it later or create a named session:

```bash
hone --session my-project
```

## Choosing a Model Provider

The `hone configure` wizard guides you through provider selection. The available providers are:

Hone supports multiple model providers. Choose based on your needs:

### 1. Anthropic API (Cloud)

Most capable. Requires an API key.

```bash
export ANTHROPIC_API_KEY="sk-ant-..."
hone --model anthropic:claude-opus-4-5
```

**First session:** Type your API key when prompted, or set the environment variable above.

**Cost:** $3–15 per million input tokens, $15–60 per million output tokens (claude-opus-4-5).

### 2. OpenAI API (Cloud)

Alternative cloud provider.

```bash
export OPENAI_API_KEY="sk-..."
hone --model openai:gpt-4o
```

### 3. Ollama (Local)

Run models on your machine (no internet, free).

Install Ollama from https://ollama.ai, then:

```bash
ollama pull llama2
ollama serve  # in another terminal
hone --model ollama:llama2
```

**Pros:** Private, instant responses, free.
**Cons:** Slower (CPU/GPU dependent), less capable than Claude/GPT-4.

### 4. Claude CLI Pass-Through (Generation 1)

Use your existing `claude` CLI. Requires Generation 1 installed.

```bash
hone --model claude-cli:claude-opus
```

### 5. Gemini CLI Pass-Through (Generation 1)

Use Google's `gemini` CLI.

```bash
hone --model gemini-cli:gemini-pro
```

### 6. ACP (Agent Client Protocol)

Full tool support via persistent subprocess. For advanced users integrating custom tools.

```bash
hone --model acp:my-custom-model --acp-endpoint /path/to/acp/binary
```

## First Interactive Session

Once you select a provider and model, you're in the session loop. Type prompts and press Enter:

```
You: What files are in this directory?
Assistant: I'll list the files for you.
[tool call: shell -> ls]
Output: main.rs  Cargo.toml  README.md
Found 3 files. You have main.rs (Rust source), Cargo.toml (manifest), and README.md.
```

**Key features:**
- **Streaming:** Tokens appear as they arrive.
- **Tool calls:** The agent calls native tools (file, shell, git, LSP) automatically.
- **Transparency:** Each tool call and result is shown.

Type `/help` to see available commands:

```
/model <provider:model>   — switch models mid-session
/budget <usd>             — set spending limit
/clear                    — clear conversation history
/export <path>            — save session to JSON
/quit                     — exit
```

## Session Management

Sessions are named conversations that persist across runs. Hone automatically saves your sessions after each turn.

### Listing and Managing Sessions

```bash
# See all your saved sessions
hone session list
```

Output example:
```
id                              created_at           updated_at           name             messages
──────────────────────────────  ───────────────────  ───────────────────  ───────────────  ────────
550e8400-e29b-41d4-a716-4467   2026-04-09 15:22:33  2026-04-09 15:45:12  my-project       42
550e8400-e29b-41d4-a716-4468   2026-04-09 14:10:00  2026-04-09 14:10:00  debug-session    5
```

### Resuming a Previous Session

```bash
# Resume a session by name
hone session resume my-project
```

All prior messages and context are restored. The session is updated with any new turns.

### Renaming and Deleting Sessions

```bash
# Rename a session
hone session rename old-name new-name

# Delete a session (irreversible)
hone session delete my-old-session
```

### Starting a Named Session

```bash
# Start a new session with a specific name (auto-created if doesn't exist)
hone --session debug-session
```

### Ephemeral (No-Save) Mode

To run without saving:

```bash
# Session won't be saved after this run
hone --ephemeral
```

This is useful for sensitive work, one-off queries, or testing.

### Crash Recovery and Checkpoint Resume

When using multi-agent orchestration, Hone saves checkpoints after each worker completes. If the process is interrupted (network failure, timeout, kill signal), you can resume the run from where it left off:

```bash
# Start an orchestrated task (auto-generates a run_id)
hone --orchestrate "Build a complex feature" 2>&1 | tee run.log

# If interrupted, resume the run using the run_id
hone --resume run_id_from_log
```

Checkpoints store:
- The original task plan and goal
- Completed worker results with costs and token usage
- Pending task IDs to resume
- Total cost accrued so far

This allows long-running orchestrations to survive transient failures without re-executing completed work.

## One-Shot Mode

Run a single command without the interactive loop:

```bash
hone "List all Python files in src/" --model anthropic:claude-opus
```

Output is streamed to stdout, no TUI.

## First Multi-Agent Orchestration Run

Hone's multi-agent orchestrator decomposes complex tasks, executes them in parallel, and merges results. This is research-backed by Anthropic, MASAI, and CodeR papers:

```bash
# Set up API key
export ANTHROPIC_API_KEY="sk-ant-..."

# Run with orchestrator
hone --orchestrate "Build a CLI tool that validates email addresses with comprehensive test coverage"
```

What happens:

1. **Decompose** — Orchestrator breaks down "build CLI tool" into parallel subtasks:
   - Write tests for email validation
   - Implement validation logic
   - Build CLI interface
   - Add error handling

2. **Execute** — Each worker agent runs in parallel on their assigned task:
   ```
   [PlanCreated] Task decomposed into 4 subtasks
   [WorkerStarted] Worker 1: "write_tests"
   [WorkerStarted] Worker 2: "implement_logic"
   [WorkerStarted] Worker 3: "build_cli"
   [WorkerStarted] Worker 4: "add_errors"
   ```

3. **Merge** — Orchestrator reads all results and produces a coherent merged output:
   ```
   [Merged] All workers completed
   Final output: Complete CLI tool with tests, logic, interface, and error handling
   ```

Expected output format:
- Token counts for each subtask
- Merged response combining all workers
- Total cost across all parallel executions

**When to use:**
- Complex features with multiple concerns
- Tasks that naturally decompose (API + CLI + tests)
- When you want parallel execution within a single request

## Context Window Management

Hone automatically optimizes context to stay within model limits while preserving code relevance:

```bash
# View context optimization details
hone --debug-context
```

**What happens behind the scenes:**

1. **Model-aware budgets** — Each model has a learned token budget. claude-opus-4-5 has 200K context; hone reserves 4K for output, 2K for system prompt, leaving ~177K for conversation.

2. **Automatic reranking** — When context gets large, Hone uses BM25 ranking to keep the most relevant code snippets and memories at the front of the context window.

3. **Graduated compression** — Old or less relevant items are progressively summarized: full code → code + summary → summary only → compressed reference.

4. **Phase detection** — Hone adapts context strategy based on conversation phase (exploration, implementation, testing, refactoring).

**Example:** Adding error handling to a 500-line authentication module:
- Turn 1-3: Exploration phase. Context shows file structure, not full code.
- Turn 4-8: Implementation phase. Context includes auth spec, test template, security patterns.
- Turn 9+: Testing phase. Context focuses on test failures and relevant code.

Set a custom context limit:

```bash
hone --context-limit 32000  # tokens
```

## Understanding the Output

### Token Streaming

Tokens arrive and render in real-time:

```
Assistant: I'm analyzing...your request. [tokens streaming]
```

You see partial text as computation happens, not the final response all at once.

### Tool Calls

When the agent needs to act, it calls a tool:

```
[Tool: read_file]
Input: {"path": "main.rs"}
Output: (file contents)
```

Tools available in Layer 2 (native, <1ms latency):
- **File I/O:** read, write, edit, glob patterns
- **Shell:** execute commands, capture output
- **Git:** commit, branch, diff, undo/redo
- **LSP bridge:** hover, diagnostics, code actions
- **Semantic index:** search code by meaning

Tools in Layer 1 (MCP, JSON-RPC):
- GitHub, Jira, Slack, databases, custom integrations

### Cost Tracking

After each turn, the output shows:

```
Usage: 4521 input tokens, 312 output tokens
Cost: $0.068 USD
Session total: $0.24 USD
Budget remaining: $9.76 / $10.00
```

All costs are tracked in real USD for your chosen model. Set a budget limit to avoid surprises:

```bash
hone --budget 5.00  # max $5 per session
```

When you hit the limit, the session stops.

## Memory and Encryption

For sensitive sessions, Hone can encrypt all memories and session data:

```bash
# Build with encryption support
cargo build --features encrypted-memory

# Initialize encryption (stores key in system keychain)
hone --init-encryption
# Enter passphrase: ****

# Run in ephemeral mode (no data saved)
hone --ephemeral
```

See [Security Guide](security.md) for memory encryption and ephemeral mode.

## Next Steps

- **Configure providers:** See [Providers Guide](providers.md) for hot-swapping, custom endpoints (DeepSeek, OpenRouter), fallback chains, and pricing tables.
- **Context optimization:** Hone automatically optimizes context using BM25 reranking and graduated compression. See [Configuration](../configuration.md) for tuning.
- **Write recipes:** See [Recipes Guide](recipes.md) to automate multi-step workflows.
- **Run the daemon:** See [Server Guide](server-and-remote.md) to use Hone as a persistent HTTP service with RBAC.
- **Custom tools:** See [Tools Guide](tools-and-mcp.md) to integrate external MCP servers and custom tools.
- **Security:** See [Security Guide](security.md) for sandbox policies, audit logging, and memory encryption.
- **Architecture:** See [Architecture Guide](../architecture.md) for deep dives into context optimization, memory encryption, and security hardening.

## Troubleshooting

### API Key Not Found

```
error: ANTHROPIC_API_KEY not set
```

Set the environment variable:

```bash
export ANTHROPIC_API_KEY="sk-ant-..."
hone
```

Or create `~/.config/hone/config.toml`:

```toml
[providers.anthropic]
api_key = "sk-ant-..."
```

### Model Not Found

```
error: model 'anthropic:claude-unknown' not supported
```

List available models:

```bash
hone --list-models
```

### Network Error (Firewall)

```
error: failed to connect to Anthropic API
```

Check your firewall and proxy settings. If behind a corporate proxy:

```toml
[http]
proxy = "http://proxy.example.com:8080"
```

### Sandbox Error on macOS

```
error: sandbox policy not supported (Seatbelt unavailable)
```

This is rare. It means the sandbox layer failed. Hone uses fail-closed policy and aborts rather than running unsandboxed. Check System Preferences > Security & Privacy.

### Out of Memory

```
error: context window exceeded maximum
```

The conversation history is too long. Clear the session:

```
/clear
```

Or reduce the context window in config:

```toml
[agent]
context_window = 32000  # tokens
```

## Support

- Bug reports: https://github.com/anthropics/hone/issues
- Discussions: https://github.com/anthropics/hone/discussions
- Documentation: See ../
