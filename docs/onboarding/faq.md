# Frequently Asked Questions

Answers organized by topic. Click a category to jump to it, or search the page.

## Getting Started

### How do I install Hone?

Build from source using Cargo (Rust 1.85+):

```bash
git clone https://github.com/honelang/hone.git
cd hone
cargo build --release
sudo cp target/release/hone /usr/local/bin/
sudo cp target/release/honed /usr/local/bin/
```

See [Getting Started](../tutorials/getting-started.md) for detailed instructions.

### What Rust version do I need?

Rust 1.85 or later. Check your version with `rustc --version`. Upgrade via `rustup update`.

### Does Hone work on Windows?

Not yet. Hone currently supports macOS (10.5+) and Linux (5.13+ for best results, older kernels with fallback support). Windows support is planned for a future release. You can use [WSL2](https://docs.microsoft.com/en-us/windows/wsl/) as a workaround.

### Can I use Hone without an API key?

Yes. Hone supports three approaches without commercial API keys:

1. **Local models via Ollama** — Run models on your machine (free, no internet)
2. **Claude/Gemini CLI pass-through** — Use your existing `claude` or `gemini` CLI tool
3. **Self-hosted servers** — Run your own LLM server (Ollama, vLLM, LM Studio)

See [Providers Guide](../tutorials/providers.md) for setup.

### How do I use local models?

Install Ollama, pull a model, and start the server:

```bash
# Terminal 1
ollama pull llama2
ollama serve

# Terminal 2
hone --model ollama:llama2
```

No API key, no cost, zero data sent outside your machine. Trade-off: slower inference and less capable models.

---

## Models and Providers

### Which model should I use?

It depends on your task:

| Task | Model | Why |
|------|-------|-----|
| Complex reasoning, architecture | Claude Opus 4.5 | Most capable, 200K context |
| General-purpose coding | Claude Sonnet 4 | Fast, balanced capability/cost |
| Simple edits, typos | Claude Haiku 3.5 | Cheapest, 200K context, still powerful |
| Budget-conscious | Ollama llama2 | Free, runs locally |
| If you prefer OpenAI | gpt-4o | Latest, competitive capability |

For most users starting out: **Claude Sonnet 4** is the best default (fast and capable).

### Can I use my Claude/OpenAI/Google subscription through Hone?

Yes. Hone integrates directly with your API keys:

- **Anthropic:** Export `ANTHROPIC_API_KEY="sk-ant-..."` and use `--model anthropic:claude-opus-4-5`
- **OpenAI:** Export `OPENAI_API_KEY="sk-..."` and use `--model openai:gpt-4o`
- **Google:** Export `GOOGLE_API_KEY="..."` and use `--model google:gemini-pro`

See [Providers Guide](../tutorials/providers.md) for full setup.

### What is ACP and why should I care?

ACP (Agent Client Protocol) is a bidirectional protocol for custom model integrations. Instead of just calling a remote API, you run your own model server and Hone connects to it persistently. This is useful if:

- You want to wrap an existing model service
- You need custom authentication or logging
- You're building a multi-model orchestration layer

For most users: you don't need ACP. It's for advanced integrations. See [ACP Protocol](../api/acp-protocol.md).

### How do I switch models mid-session?

Use the `/model` command:

```
You: /model openai:gpt-4o
Assistant: Switched to gpt-4o. Conversation history preserved.
```

Your conversation history is preserved. The context manager re-evaluates token count for the new model's limits.

### How much does it cost?

Depends on the model:

| Model | Input | Output | Example (10K in, 1K out) |
|-------|-------|--------|--------------------------|
| Claude Opus 4.5 | $3/M | $15/M | ~$0.045 |
| Claude Sonnet 4 | $3/M | $15/M | ~$0.045 |
| Claude Haiku 3.5 | $0.80/M | $4/M | ~$0.012 |
| GPT-4o | $2.50/M | $10/M | ~$0.035 |
| Ollama (local) | Free | Free | $0.00 |

Local models (Ollama) cost nothing. Cloud models cost per million tokens. See [Providers Guide](../tutorials/providers.md) for full pricing table.

### Can I set a spending limit?

Yes. Set a budget before starting a session:

```bash
hone --budget 5.00  # stop after spending $5
```

When the limit is approached, you're asked to confirm:

```
Session cost: $4.90
Approaching budget limit ($5.00). Continue? (y/n)
```

You can also set a default budget in `~/.config/hone/config.toml`:

```toml
[agent]
default_budget_usd = 10.00
```

---

## Security and Privacy

### Is my code sent to the cloud?

No, not by Hone. Your code stays on your machine unless:

1. You explicitly use MCP servers that access external services (GitHub, Jira, etc.)
2. You use a cloud model provider (Anthropic, OpenAI, etc.) — in that case, your code is sent to that provider's API

You can avoid cloud entirely by using local models (Ollama). See [Security Guide](../tutorials/security.md).

### What is kernel sandboxing?

Kernel sandboxing restricts what code the agent can execute. When the agent runs a shell command, it doesn't happen in your main shell—it happens in a restricted environment where it can only touch files and executables you explicitly allow.

On macOS, Hone uses **Seatbelt**. On Linux, it uses **Landlock** (or seccomp). Think of it like giving the agent access to a locked room: they can see what's inside, but nothing outside.

This prevents:
- Agent deleting `/etc` by accident
- Malicious prompt injection exploiting agent to exfiltrate your SSH key
- Runaway processes consuming all disk space

See [Security Guide](../tutorials/security.md) for configuration.

### Can the agent delete my files?

Only if you explicitly allow it. By default, tools have write access to:
- The current project directory
- `/tmp` (temporary files)

Everything else is blocked by the sandbox. You can further restrict this:

```toml
[sandbox.tools.file_writer]
allow_write = ["{project_root}/generated"]  # only this directory
```

If the agent tries to delete `/etc/passwd`, the sandbox blocks it:

```
error: access denied (path not in allowlist)
```

### Can I run Hone in a Docker container?

Yes. Hone is a static Rust binary (no external dependencies), so it works in minimal Docker images:

```dockerfile
FROM debian:bookworm-slim
COPY target/release/hone /usr/local/bin/
ENTRYPOINT ["hone"]
```

Container security notes:
- Hone's sandboxing adds an extra security layer on top of Docker's isolation
- Run the container with `--security-opt seccomp=unconfined` if using Linux seccomp
- Use `--cap-drop=ALL` and `--cap-add=NET_BIND_SERVICE` for least privilege

### How do I use ephemeral mode?

Run with the `--ephemeral` (or `-E`) flag:

```bash
hone -E -i "Analyze this sensitive document"
```

In ephemeral mode:
- No session data is saved to disk
- No memories are recorded
- Conversation history is lost when the session ends
- Cost tracking is not persisted

Useful for analyzing sensitive data, contracts, or credentials.

---

## Tools and Extensions

### What tools does Hone have built in?

Hone's **Layer 2 (native tools)** includes:

- `read_file` — Read text files
- `write_file` — Write text files
- `search_files` — Grep-like file search
- `shell` — Execute shell commands
- `git` — Git operations (commit, log, diff, push)
- `semantic_search` — Vector-based code search
- `lsp_hover` — Hover information (types, docs)
- `lsp_completion` — Code completions
- `lsp_definition` — Go to definition
- `lsp_diagnostics` — Linter warnings and errors

See [Tools and MCP Guide](../tutorials/tools-and-mcp.md) for full reference.

### Can I add my own tools?

Yes, via **Layer 1 (MCP servers)**. Write an MCP server in any language that speaks JSON-RPC, and Hone will call it:

```toml
[[mcp_servers]]
name = "my_custom_tool"
command = "node"
args = ["my_server.js"]
```

Your server defines tools by implementing the MCP protocol. See [MCP Protocol Spec](https://spec.modelcontextprotocol.io/) for details.

### What is MCP?

**Model Context Protocol** is an open standard (JSON-RPC over stdio) for tools. It lets you integrate any service (GitHub, Jira, Slack, databases) without modifying Hone's source code.

Think of it as a plugin system: you write a server that speaks JSON-RPC, Hone talks to it, the agent uses the tools it exposes.

See [MCP Specification](https://spec.modelcontextprotocol.io/) and [Tools and MCP Guide](../tutorials/tools-and-mcp.md).

### How do I connect GitHub/Jira/Slack?

Use official MCP servers. Examples:

**GitHub:**
```toml
[[mcp_servers]]
name = "github"
command = "npx"
args = ["@modelcontextprotocol/server-github"]
env = { GITHUB_TOKEN = "${GITHUB_TOKEN}" }
```

**Jira:**
```toml
[[mcp_servers]]
name = "jira"
command = "npx"
args = ["@modelcontextprotocol/server-jira"]
env = { JIRA_URL = "${JIRA_URL}", JIRA_TOKEN = "${JIRA_TOKEN}" }
```

**Slack:**
```toml
[[mcp_servers]]
name = "slack"
command = "npx"
args = ["@modelcontextprotocol/server-slack"]
env = { SLACK_BOT_TOKEN = "${SLACK_BOT_TOKEN}" }
```

See [Providers Guide](../tutorials/providers.md) for complete setup.

---

## Workflows

### How is Hone different from GitHub Copilot?

| Aspect | Copilot | Hone |
|--------|---------|------|
| Where it runs | IDE plugin | Terminal/daemon |
| Scope | Current file, quick edits | Whole repo, multi-step tasks |
| Model | Claude or GPT-4o | 75+ providers (your choice) |
| Tools | Limited to IDE APIs | File, shell, git, semantic search, MCP |
| Memory | None | Adaptive, learns patterns |
| Cost tracking | Hidden in subscription | Transparent per-session |
| Offline | No | Yes (with local models) |
| Privacy | Data sent to Anthropic/OpenAI | Local-first (optional cloud) |

**Summary:** Copilot is great for quick inline suggestions. Hone is for multi-step reasoning tasks, refactoring, and architecture work.

### Can Hone write tests?

Yes. Hone can read your code, understand the test framework, and generate tests:

```
You: /shell pytest tests/
Agent: [runs tests, sees failures]
You: Write tests for the auth module
Agent: [generates test cases, runs pytest]
```

Hone can also use recipes to automate test generation. See [Recipes Guide](../tutorials/recipes.md).

### Can Hone work on multiple files?

Yes. The agent can:
- Read multiple files
- Understand relationships between them
- Make coordinated changes across files
- Run git to see diffs

Use semantic search to find related code:

```
You: Find all calls to the `authenticate` function
Agent: [semantic search finds 42 matches across 12 files]
You: Add a new parameter `timeout` to all of them
Agent: [generates patches, applies to all files]
```

### How do I undo a change?

Hone tracks changes via git. To undo:

```bash
git diff HEAD               # see what changed
git checkout <filename>     # undo changes to a file
git reset --hard HEAD       # undo all changes
```

Or in the agent session:

```
You: Undo the last changes
Agent: [runs git reset]
```

Hone doesn't have a built-in undo—it relies on git's history.

### What are recipes?

Recipes are YAML files that define multi-step workflows:

```yaml
name: refactor-logging
description: Upgrade log statements to structured logging

steps:
  - tool: file_search
    pattern: "console.log|print\\("

  - agent:
      prompt: "Replace with structured logging"
      model: claude-sonnet-4

  - tool: git_commit
    message: "refactor: upgrade to structured logging"
```

Run a recipe:

```bash
hone --recipe refactor-logging.yaml
```

See [Recipes Guide](../tutorials/recipes.md) for full syntax.

---

## Team and Enterprise

### Can multiple people use the same session?

Yes, via the `honed` daemon. Run the daemon on a server:

```bash
honed --port 7777
```

Multiple users connect:

```bash
hone --remote server.com:7777
```

Users see shared session history and can collaborate in real-time. See [Server and Remote Guide](../tutorials/server-and-remote.md).

### How do I set up RBAC?

Define roles in `~/.config/hone/config.toml`:

```toml
[[rbac.roles]]
name = "developer"
permissions = ["sessions:create", "tools:file", "tools:git", "tools:shell"]

[[rbac.users]]
user_id = "alice@company.com"
role = "developer"
```

Roles control what users can do (create sessions, call tools, etc.). See [Security Guide](../tutorials/security.md) for full permission list.

### Where are audit logs stored?

By default: `/var/log/hone/audit.log` (configurable via `~/.config/hone/config.toml`):

```toml
[audit]
enabled = true
log_file = "/var/log/hone/audit.log"
```

Logs are JSON, one event per line:

```json
{"timestamp": "2026-04-05T14:30:45Z", "session_id": "sess_abc123", "user": "alice@example.com", "event": "tool_call", "tool": "git_commit", ...}
```

Query logs:

```bash
grep "alice@example.com" /var/log/hone/audit.log | jq .
```

### Can I self-host everything?

Yes. Hone is designed for self-hosting:

- Use local models (Ollama) — no cloud required
- Run `honed` daemon on your own server
- Store memory in encrypted SQLite on your machine
- Audit logs saved locally
- No telemetry, no phone home

For deployment details, see [Server and Remote Guide](../tutorials/server-and-remote.md).

---

## Troubleshooting

### Hone is slow, what do I do?

Slow can mean different things:

**Slow startup (takes >5 seconds):**
- Check if semantic index is being built (`~/.local/share/hone/index/`)
- Disable indexing: `[index] enabled = false` in config
- Or run indexing in background: `honed --index-daemon`

**Slow responses (model is slow):**
- Use a faster model: Haiku instead of Opus, or local Ollama
- Reduce context window: `hone --context-window 50000`
- Enable memory (reduces context needed): `[memory] enabled = true`

**Slow tool execution:**
- Check sandbox overhead: try disabling sandbox (not recommended): `[sandbox] enforce = false`
- Profile with `RUST_LOG=debug hone -i "your prompt"`

### I'm getting "sandbox denied" errors

Example error:

```
error: access denied (path not in allowlist)
Agent called: read_file(path="/etc/shadow")
```

The sandbox is working correctly. The agent tried to access a path you didn't allow. Either:

1. **Allow the path** in config:
```toml
[sandbox.tools.file_reader]
allow_read = ["/new/path"]
```

2. **Or, the agent made a wrong choice.** Ask it differently:
```
You: Read the config file in the project root, not system files
```

### The model keeps running out of context

The model exhausted its context window (200K tokens for Claude, 128K for GPT-4o, etc.). Solutions:

1. **Switch to a model with more context:** Opus/Sonnet/Haiku (200K), not GPT-4o (128K)
2. **Reduce context window:** `hone --context-window 100000` (use less than the full limit)
3. **Enable memory:** Compacts history into learned patterns, freeing up tokens
4. **Summarize history:** Ask the agent to summarize the first half of the conversation
5. **Use recipes:** Break the task into smaller steps executed sequentially

### How do I reset memory?

Memory is stored in `~/.local/share/hone/memory.db`. To reset:

```bash
rm ~/.local/share/hone/memory.db
```

Next session, memory starts fresh.

To view what's in memory:

```bash
sqlite3 ~/.local/share/hone/memory.db
> SELECT * FROM patterns;
```

---

## Further Reading

- **Getting started guide:** [Getting Started](../tutorials/getting-started.md)
- **Security deep dive:** [Security Guide](../tutorials/security.md)
- **All about providers:** [Providers Guide](../tutorials/providers.md)
- **Tools and extensions:** [Tools and MCP Guide](../tutorials/tools-and-mcp.md)
- **Core concepts:** [Core Concepts](concepts.md)
- **Quick reference:** [Cheatsheet](cheatsheet.md)
- **Server setup:** [Server and Remote Guide](../tutorials/server-and-remote.md)
