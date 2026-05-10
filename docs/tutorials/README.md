# Hone Tutorials

Welcome to the Hone tutorial documentation. These guides will help you get started, configure providers, integrate tools, and deploy Hone securely.

## Quick Start

Start here if you're new to Hone:

1. [Getting Started](getting-started.md) — Install, build, and run your first session (20 min)
2. [Providers](providers.md) — Configure model providers and switch between them (15 min)

## Complete Guides

### For Users

- **[Getting Started](getting-started.md)** — Prerequisites, building from source, first interactive session, one-shot mode, understanding output (token streaming, tool calls, cost tracking)

- **[Providers](providers.md)** — Configuring Anthropic API, OpenAI (and custom endpoints like DeepSeek, OpenRouter, Qwen), Ollama (local), CLI pass-through tools, ACP, hot-swapping providers, fallback chains, model routing rules, pricing tables, cost tracking, health checks, custom timeouts

- **[Tools and MCP](tools-and-mcp.md)** — Layer architecture (L0–L3), native tools (file, shell, git, LSP, semantic index), MCP integration (JSON-RPC, external tools), Code Mode (agent-generated orchestration), skills (HONE.md conventions), tool registry and dispatch, writing custom tools

- **[Recipes](recipes.md)** — YAML recipe system, recipe format with annotations, conditional steps, TDD hook (test_cmd), running recipes interactively or in batch mode, recipe output and export, writing and sharing recipes

- **[Orchestration GUI](orchestration-gui.md)** — `/plan` plan-preview gate (TUI + web UI), `/budget` and `/permit` per-run governance, `/workflows` saved-recipe drawer, the `POST /orchestrate?dry_run=true` and `POST /recipes` daemon endpoints. Pairs with the [research doc](../research/gui-for-non-cli-users-2026-05.md) and [TDD plan](../plans/orchestration-gui-tdd-2026-05.md).

- **[ACP: Bidirectional Tool Dispatch](acp.md)** — Configure ACP providers (`claude --acp`, `gemini --acp`), the full lifecycle (initialize → schema announce → tool_call → toolResult → continuation → done), running the integration suite, schema authoring tips, cancellation semantics, troubleshooting

- **[TUI Keybindings and Themes](tui-keybindings-and-themes.md)** — Layout overview, every keybinding, the command palette, autocomplete with descriptions, switching themes (`crimson`, `neon`, `chalk`, `minimal`), `~/.config/hone/theme.toml` overrides, accessibility flags (`HONE_NO_COLOR`, `HONE_REDUCED_MOTION`, `HONE_ASCII_ONLY`), vim-style bindings, mouse selection

- **[Agent Rail](agent-rail.md)** — `Ctrl+R` toggle, four observability sections (FILES / AGENTS / CONTEXT / MEMORY), status glyphs, NO_COLOR fallback, data flow from `AgentEvent` to `TurnLedger` to widget, recorded vhs demo, troubleshooting empty sections

- **[TDD with Hone](tdd-with-hone.md)** — `/tdd` watcher (auto-detected test commands, status badges, fix-on-failure flow), `tdd-feature` recipe (full Red-Green-Refactor automation), `bug-fix` recipe (regression-first), separating test author from implementer via `@`-agents, fast inner loops (single-crate, `cargo nextest`), CI integration

- **[Debugging and Troubleshooting](debugging.md)** — `--doctor` first checks, provider issues (API keys, rate limits, model not found, hot-swap), tool dispatch (no calls, timeouts, ACP-specific failures), TUI issues (size, theme, mouse), sessions and state (memory, daemon crash), sandbox failures, recipes/TDD pitfalls, logging and observability, bug reports

- **[Memory and Pass-Through Caching](memory.md)** — the five layers (persistent store, HONE.md, context injection, Anthropic prompt-cache pass-through, local LRU), inspecting recalls in the TUI (`Ctrl+M`), `--ephemeral` mode, deleting memories, optional `EncryptedStore` (AES-256-GCM + Argon2id), Anthropic `cache_control` markers, debugging surprising recalls. Pairs with [`../architecture/memory.md`](../architecture/memory.md) for the design reference.

### For Operators

- **[Server and Remote](server-and-remote.md)** — Starting the daemon (`honed`), REST API overview (sessions, turns, SSE events), real-time streaming with Server-Sent Events, tunnel support (Cloudflare, Ngrok), authentication (bearer tokens), collaborative sessions, health checks, running multiple daemons, systemd integration

- **[Security](security.md)** — Hardened sandbox architecture (Seatbelt on macOS, Landlock on Linux, seccomp on containers), path traversal prevention, shell command classification, SandboxPolicy (file/network restrictions), network deny-by-default, RBAC roles and policies, audit logging, memory encryption (AES-256-GCM with Argon2id), ephemeral mode, threat model, best practices, troubleshooting

## Learning Paths

### Path 1: Interactive Agent (30 min)

1. [Getting Started](getting-started.md) — Set up and run first session
2. [Providers](providers.md) — Switch between models

You can now use Hone interactively to ask questions, analyze code, and run shell commands.

### Path 2: Automated Workflows (45 min)

1. [Getting Started](getting-started.md) — Set up
2. [Recipes](recipes.md) — Define multi-step workflows
3. [Tools and MCP](tools-and-mcp.md) — Extend with custom tools

You can now automate complex tasks via YAML recipes.

### Path 3: Team Deployment (60 min)

1. [Getting Started](getting-started.md) — Set up
2. [Server and Remote](server-and-remote.md) — Run the daemon
3. [Security](security.md) — Configure sandboxes, RBAC, memory encryption, and audit logging
4. [Providers](providers.md) — Set up cost tracking, fallback chains, and custom endpoints
5. [Configuration](../configuration.md) — Fine-tune context optimization and budgets

You can now deploy Hone as a multi-user HTTP service with audit logging, access control, and encrypted memory.

### Path 4: Custom Integration (90 min)

1. [Getting Started](getting-started.md) — Set up
2. [Tools and MCP](tools-and-mcp.md) — Integrate external tools
3. [Providers](providers.md) — Hot-swap providers based on task type
4. [Server and Remote](server-and-remote.md) — Expose via HTTP API

You can now build custom applications on top of Hone.

## Typical Use Cases

### Use Case: Code Review

Start with [Getting Started](getting-started.md), then use [Recipes](recipes.md) to automate the review process:

```yaml
name: "Code Review"
steps:
  - name: "Analyze Changes"
    prompt: "Review the diff and identify issues"
  - name: "Suggest Improvements"
    prompt: "What are the top 3 improvements?"
  - name: "Generate Summary"
    prompt: "Write a review summary"
test_cmd: "cargo test"
```

Run it:

```bash
hone --recipe code-review.yaml
```

### Use Case: Semantic Code Search

Start with [Getting Started](getting-started.md), then use [Tools and MCP](tools-and-mcp.md) to access the semantic index:

```bash
hone -i "Find functions that handle authentication"
```

The agent calls the `semantic_search` tool (Layer 2 native) to search by meaning.

### Use Case: Multi-User Team Workspace

Start with [Server and Remote](server-and-remote.md), then configure [Security](security.md) for RBAC:

```bash
honed --port 7777 --config /etc/hone/config.toml
```

Multiple users can now create sessions and collaborate via the REST API.

### Use Case: Custom Tool Integration

Start with [Tools and MCP](tools-and-mcp.md) to write an MCP server:

```bash
# Your custom MCP server
python3 my-mcp-server.py
```

Then configure it in `config.toml`:

```toml
[[mcp_servers]]
name = "custom"
command = "python3"
args = ["my-mcp-server.py"]
```

The agent can now call your custom tools.

## File Structure

All tutorials live in this directory (`docs/tutorials/`):

```
docs/
├── tutorials/
│   ├── README.md                          # This file (overview & learning paths)
│   ├── getting-started.md                 # Installation & first session
│   ├── providers.md                       # Model providers & hot-swapping
│   ├── acp.md                             # ACP bidirectional tool dispatch
│   ├── tools-and-mcp.md                   # Tool system & integrations
│   ├── recipes.md                         # YAML recipes & automation
│   ├── tdd-with-hone.md                   # /tdd watcher + tdd-feature recipe
│   ├── tui-keybindings-and-themes.md      # TUI bindings, themes, accessibility
│   ├── agent-rail.md                      # Per-turn observability sidebar (Ctrl+R)
│   ├── debugging.md                       # Symptom-driven troubleshooting
│   ├── memory.md                          # Memory layers & pass-through caching
│   ├── server-and-remote.md               # Daemon & HTTP API
│   └── security.md                        # Sandboxing & audit logging
├── architecture.md                        # System design (read after tutorials)
├── implementation-plan.md                 # Build phases (for contributors)
└── ...
```

## Related Documentation

- **[Architecture](../architecture.md)** — System design, crate structure, data flows, trait definitions. Read after tutorials to understand how Hone works internally.
- **[Implementation Plan](../implementation-plan.md)** — For contributors: TDD workflow, crate build sequence, test strategy, CI/CD pipeline.

## Quick Reference

### Commands

```bash
# Interactive session
hone

# One-shot command
hone -i "List files in src/"

# Run a recipe
hone --recipe my-recipe.yaml

# Switch model
hone --model ollama:llama2

# Set budget limit
hone --budget 10.00

# Run daemon
honed --port 7777

# Connect to remote daemon
hone --server-url https://hone.example.com --auth-token "token"
```

### Configuration

```bash
# Config location
~/.config/hone/config.toml

# Edit
$EDITOR ~/.config/hone/config.toml
```

### Session Management

```bash
# List sessions (daemon only)
curl http://localhost:7777/sessions

# Get session info
curl http://localhost:7777/sessions/sess_abc123

# Delete session
curl -X DELETE http://localhost:7777/sessions/sess_abc123

# Stream events (SSE)
curl http://localhost:7777/sessions/sess_abc123/events
```

## Troubleshooting

### Can't find Hone after building

```bash
# Make sure you added to PATH
export PATH="$PWD/target/release:$PATH"
hone --version
```

### API key not found

```bash
export ANTHROPIC_API_KEY="sk-ant-..."
hone
```

Or create `~/.config/hone/config.toml`:

```toml
[providers.anthropic]
api_key = "sk-ant-..."
```

### Sandbox error on macOS

This is rare. See [Security Guide](security.md) troubleshooting section.

### Out of memory

The conversation history is too long. Clear the session:

```
/clear
```

## Support

- **Bug reports:** https://github.com/anthropics/hone/issues
- **Discussions:** https://github.com/anthropics/hone/discussions
- **Documentation issues:** https://github.com/anthropics/hone/issues/new?labels=docs

## License

All Hone documentation is licensed under the Creative Commons Attribution 4.0 International License (CC-BY-4.0). See LICENSE for details.
