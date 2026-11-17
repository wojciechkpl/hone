# Frequently Asked Questions

Answers organized by topic. Click a category to jump to it, or search the page.

## Getting Started

### How do I install Hone?

Build from source using Cargo (Rust 1.88+):

```bash
git clone https://github.com/honelang/hone.git
cd hone
cargo build --release
sudo cp target/release/hone /usr/local/bin/
sudo cp target/release/honed /usr/local/bin/
```

See [Getting Started](../tutorials/getting-started.md) for detailed instructions.

### What Rust version do I need?

Rust 1.88 or later. Check your version with `rustc --version`. Upgrade via `rustup update`.

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
- **Google:** There is no `google:` prefix. Use `--model gemini-cli:gemini-2.5-pro`, which runs your installed `gemini` CLI with its own login and its own tools (not Hone's), or reach Gemini through an OpenAI-compatible endpoint such as OpenRouter, which `hone configure` offers

See [Providers Guide](../tutorials/providers.md) for full setup.

### What is ACP and why should I care?

ACP (Agent Client Protocol) is a bidirectional protocol for custom model integrations. Instead of just calling a remote API, you run your own model server and Hone connects to it persistently. This is useful if:

- You want to wrap an existing model service
- You need custom authentication or logging
- You're building a multi-model orchestration layer

For most users: you don't need ACP. It's for advanced integrations, and for now it's library-only: `hone` can't select an ACP provider. See [ACP](../tutorials/acp.md) and [ACP Protocol](../api/acp-protocol.md).

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

Yes. Set a session budget in `~/.config/hone/config.toml` (there is no `--budget` flag):

```toml
[budget]
session_limit_usd = 5.00  # stop after spending $5
```

Hone checks the limit before each turn. Once the session has spent more, the next turn stops with `budget exceeded: limit $5.0000, current $…`.

In the TUI, `/budget 5` sets the limit for the rest of the session.

---

## Security and Privacy

### Is my code sent to the cloud?

No, not by Hone. Your code stays on your machine unless:

1. You explicitly use MCP servers that access external services (GitHub, Jira, etc.)
2. You use a cloud model provider (Anthropic, OpenAI, etc.) — in that case, your code is sent to that provider's API

You can avoid cloud entirely by using local models (Ollama). See [Security Guide](../tutorials/security.md).

### What is kernel sandboxing?

Kernel sandboxing restricts what the agent's shell commands can touch. Each command runs in a restricted environment: it can write only to the working directory, `/tmp` and `/dev/null`, and read system directories, the working directory and a few toolchain folders. You can't widen the write list; `HONE_SANDBOX_EXTRA_READ` adds read-only roots.

On macOS, Hone uses **Seatbelt**, which is always on and also blocks network access. On Linux, it uses **Landlock**, which is off unless `HONE_SANDBOX_ENFORCE=1` and restricts files only. Hone doesn't use seccomp.

The sandbox covers shell commands only. File tools such as `read_file` and `write_file` run inside Hone: they refuse paths outside the working directory, and permission rules narrow them further. The sandbox sets no CPU, memory or disk limits.

See [Security Guide](../tutorials/security.md) and [Permissions and sandbox](../permissions-and-sandbox.md).

### Can the agent delete my files?

It can, unless a rule or the sandbox stops it:

- **Shell commands** can write only to the working directory, `/tmp` and `/dev/null` (always on macOS; on Linux only with `HONE_SANDBOX_ENFORCE=1`). The shell command checks also refuse destructive commands such as `rm -rf`.
- **File tools** (`write_file`, `edit_file`, `multi_edit`) refuse any path that resolves outside the working directory (symlinks included), but inside it they can change or overwrite any file your user can; they aren't OS-sandboxed. In the TUI they ask before each write unless you switch to AutoApprove.

To keep the agent out of a path, add a rule to `~/.config/hone/permissions.toml`:

```toml
# ~/.config/hone/permissions.toml
[[rules]]
id = "frozen-migrations"
path = "migrations/**"
op = "write"
decision = "deny"
```

A write to `migrations/` is then refused with `denied by policy` and the rule id. Without a matching rule a write inside the working directory is allowed; a write outside it is always refused. See [Permissions and sandbox](../permissions-and-sandbox.md).

### Can I run Hone in a Docker container?

Yes. Hone is a static Rust binary (no external dependencies), so it works in minimal Docker images:

```dockerfile
FROM debian:bookworm-slim
COPY target/release/hone /usr/local/bin/
ENTRYPOINT ["hone"]
```

Container security notes:
- On Linux, Hone's shell sandbox (Landlock) is off unless you set `HONE_SANDBOX_ENFORCE=1`. Hone doesn't use seccomp
- Use `--cap-drop=ALL` and `--cap-add=NET_BIND_SERVICE` for least privilege

### How do I use ephemeral mode?

Run with the `--ephemeral` flag:

```bash
hone --ephemeral "Analyze this sensitive document"
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

Hone's **Layer 2 (native tools)**, as the `hone` CLI registers them:

- Files: `read_file`, `write_file`, `edit_file`, `multi_edit`, `glob`, `grep`
- Shell: `shell`
- Git: `git_status`, `git_diff`, `git_commit`
- Code intelligence: `lsp_diagnostics`, `lsp_hover`, `lsp_code_actions`, plus `symbol_search` and `symbol_at` when the symbol index is available
- Web: `web_fetch`, and `web_search` (returns an error unless `HONE_WEBSEARCH_PROVIDER` and `HONE_WEBSEARCH_KEY` are set)
- Memory: `memory_read`, when long-term memory is on; `memory_write` too when `[memory] writer` is `tool` or `triggers+tool`
- Screen: `screen_capture`, `computer_control`

See [Tools and MCP Guide](../tutorials/tools-and-mcp.md) for full reference.

### Can I add my own tools?

Yes, via **Layer 1 (MCP servers)**. Write an MCP server in any language that speaks JSON-RPC, and add it to `~/.config/hone/mcp.json` (or `.hone/mcp.json` in the project):

```json
{
  "mcpServers": {
    "my_custom_tool": {
      "command": "node",
      "args": ["/path/to/my_server.js"]
    }
  }
}
```

Your server defines tools by implementing the MCP protocol. See [MCP Protocol Spec](https://spec.modelcontextprotocol.io/) for details.

### What is MCP?

**Model Context Protocol** is an open standard (JSON-RPC over stdio) for tools. It lets you integrate any service (GitHub, Jira, Slack, databases) without modifying Hone's source code.

Think of it as a plugin system: you write a server that speaks JSON-RPC, Hone talks to it, the agent uses the tools it exposes.

See [MCP Specification](https://spec.modelcontextprotocol.io/) and [Tools and MCP Guide](../tutorials/tools-and-mcp.md).

### How do I connect GitHub/Jira/Slack?

Add an MCP server for each one to `~/.config/hone/mcp.json` (or `.hone/mcp.json` in the project). GitHub, with its MCP server:

```json
{
  "mcpServers": {
    "github": {
      "command": "npx",
      "args": ["-y", "@modelcontextprotocol/server-github"],
      "env": { "GITHUB_PERSONAL_ACCESS_TOKEN": "${GITHUB_TOKEN}" }
    }
  }
}
```

Jira and Slack servers go in the same `mcpServers` object, each with its own `command`, `args` and `env`; the server's README lists its package and the variables it needs. `env` values may reference `${VAR}` from your environment. See [MCP servers](../configuration.md#mcp-servers) for all fields.

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

Run a built-in recipe by name; a file such as `refactor-logging.yaml` can't be run yet (`--recipe` runs only the 9 built-in recipes by name (`hone --list-recipes`); a recipe file fails with `unknown recipe: <name>`):

```bash
hone --recipe refactor --var test_command="cargo test"
```

See [Recipes Guide](../tutorials/recipes.md) for full syntax.

---

## Team and Enterprise

### Can multiple people use the same session?

Not as one shared conversation. Several people can use one `honed` daemon through the desktop app or the HTTP API (the `hone` CLI doesn't connect to a daemon), and its `/sessions/{id}/join` endpoints track who has joined a session. But `POST /run` is stateless: each client keeps its own conversation history.

Run the daemon on a server:

```bash
export HONE_AUTH_TOKEN="$(openssl rand -hex 32)"  # every client sends this as a bearer token
honed 7777
```

See [Run the daemon](../getting-started.md#8-run-the-daemon-honed) and the [Server and Remote Guide](../tutorials/server-and-remote.md).

### How do I set up RBAC?

You can't: Hone has no roles and no per-user permissions or budgets. `honed` checks one bearer token (`HONE_AUTH_TOKEN`), and every client that sends it gets the same access. To limit what the agent may do in every session, write `deny` rules in `~/.config/hone/permissions.toml` of the user that runs `honed`. Use `deny`, not `ask`: a client can pre-approve `ask` rules when it starts a run. See [Permissions and sandbox](../permissions-and-sandbox.md).

### Where are audit logs stored?

In `.hone/audit.jsonl` in the working directory, and only when you run with `--audit` or set `HONE_AUDIT_LOG=1`. There is no setting for the log's path or level.

```bash
hone --audit "fix the failing test"
```

Logs are JSON, one entry per line: one per tool call, including calls a permission rule refused, plus one `context` line per model call. A refused call:

```json
{"timestamp":"2026-11-12T09:30:00Z","tool":"read_file","arguments":{"path":"/home/alice/.ssh/id_ed25519"},"result_preview":"policy: deny (rule no-ssh-keys)","blocked":true,"block_reason":"policy: deny (rule no-ssh-keys)","injection_risk":null,"duration_ms":0,"trace_id":"019a77c4-2f1e-7d0a-9b1c-5e3f2a8d4c61","span_id":"019a77c4-3a02-7e4b-8c2d-6f4a3b9e5d72","parent_span_id":"019a77c4-2f1f-7a3c-a4e5-1b2c3d4e5f60","span_kind":"tool_call"}
```

Query logs from the same directory:

```bash
hone trace list                          # recent traces, one summary line each
jq 'select(.blocked)' .hone/audit.jsonl  # calls a rule refused
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
- The symbol index (`~/.local/share/hone/index.db`) is built in the background and doesn't block startup; there is no `[index]` setting and no `--index-daemon` option
- Skip the index and memory for a run with `--ephemeral`
- If the first answer is the slow part, skip the repository skeleton built on turn 1 with `--no-inject`

**Slow responses (model is slow):**
- Use a faster model: Haiku instead of Opus, or local Ollama
- Reduce context window: `HONE_CONTEXT_WINDOW=50000 hone`
- Memory is already on unless you run with `--ephemeral`; there is no `[memory] enabled` setting

**Slow tool execution:**
- Check sandbox overhead: there is no `[sandbox] enforce` setting. On macOS the shell sandbox (Seatbelt) is always on; on Linux it is off unless `HONE_SANDBOX_ENFORCE=1`
- Profile with `RUST_LOG=debug hone "your prompt"`

### I'm getting "sandbox denied" errors

Two different things refuse calls.

**A permission rule.** The call is refused with `denied by policy` and the rule's id, or, for an `ask` rule in a headless run, with `requires approval (rule <id>)`. Either:

1. **Change the rule** in `~/.config/hone/permissions.toml` (or the project's `.hone/permissions.toml`), or approve an `ask` rule for one run with `hone --approve <id>`.

2. **Or, the agent made a wrong choice.** Ask it differently:
```
You: Read the config file in the project root, not system files
```

**The OS sandbox.** A shell command fails with a permission error from the OS. Shell commands can write only to the working directory, `/tmp` and `/dev/null`, and that can't be widened. To let them read a directory outside the defaults, add it to `HONE_SANDBOX_EXTRA_READ` (colon-separated):

```bash
HONE_SANDBOX_EXTRA_READ="$HOME/.nvm" hone
```

### The model keeps running out of context

The model exhausted its context window (200K tokens for Claude, 128K for GPT-4o, etc.). Solutions:

1. **Switch to a model with more context:** Opus/Sonnet/Haiku (200K), not GPT-4o (128K)
2. **Reduce context window:** `HONE_CONTEXT_WINDOW=100000 hone` (use less than the full limit)
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
