# Hone Quick Reference

A single-page cheatsheet for daily use. Print this out or save it as a browser bookmark.

---

## Commands (Slash Commands in Session)

Run these in an active session:

| Command | Purpose |
|---------|---------|
| `/agent <name>` | Switch to a built-in or custom agent |
| `/model <provider:model>` | Switch providers (e.g., `/model openai:gpt-4o`) |
| `/budget <usd>` | Set spending limit for session |
| `/context <tokens>` | Adjust context window size |
| `/memory on\|off` | Enable/disable memory for session |
| `/sandbox on\|off` | Enable/disable kernel sandbox (not recommended) |
| `/files <pattern>` | List files matching pattern (e.g., `/files *.rs`) |
| `/git <command>` | Run git command (e.g., `/git log --oneline`) |
| `/shell <command>` | Run shell command with full output |
| `/semantic-search <query>` | Find related code using vectors |
| `/recipes list` | Show available recipes |
| `/agents list` | Show available agents |
| `/help` | Show all commands |
| `/exit` or Ctrl+C | Quit session |

---

## CLI Flags

Run these when starting `hone`:

| Flag | Example | Purpose |
|------|---------|---------|
| (prompt) | `hone "prompt"` | One-shot mode (no REPL) |
| `--orchestrate` | `hone --orchestrate "..."` | Multi-agent orchestrator |
| `--ensemble` | `hone --ensemble 3 "..."` | Run N candidates, rank best |
| `--recipe` | `hone --recipe tdd-feature` | Run a built-in recipe |
| `--list-recipes` | `hone --list-recipes` | List available recipes |
| `--agent` | `hone --agent code-reviewer` | Use a specific agent |
| `--model` | `hone --model openai:gpt-4o` | Choose model/provider |
| `--dir` | `hone --dir ~/myproject` | Target a directory |
| `--budget` | `hone --budget 5.00` | Max spend in USD |
| `--context-window` | `hone --context-window 64000` | Limit context (tokens) |
| `--remote` | `hone --remote localhost:7777` | Connect to daemon |
| `--ephemeral` | `hone --ephemeral` | No memory persistence |
| `--config` | `hone --config ~/.config/hone/config.toml` | Custom config file |
| `--help` | `hone --help` | Show all options |

---

## Keyboard Shortcuts

| Shortcut | Action |
|----------|--------|
| Ctrl+C | Stop current response / exit |
| Ctrl+L | Clear screen |
| Ctrl+D | EOF (exit gracefully) |
| Tab | Autocomplete (in input) |
| PageUp / PageDown | Scroll history |
| Ctrl+P | Previous prompt |
| Ctrl+N | Next prompt |
| Ctrl+R | Search history (if interactive) |

---

## Config Hierarchy (Priority Order)

CLI flags override everything. Lowest priority wins if flag not set.

1. **CLI flags** (highest priority)
   ```bash
   hone --model gpt-4o --budget 5.00
   ```

2. **Environment variables**
   ```bash
   export ANTHROPIC_API_KEY="sk-ant-..."
   export HONE_CONFIG="/custom/config.toml"
   ```

3. **Project config** (`./HONE.md` + `./.hone/config.toml`)
   ```yaml
   # HONE.md: project-level skills
   # .hone/config.toml: project-level settings
   ```

4. **User config** (`~/.config/hone/config.toml`)
   ```toml
   [agent]
   default_model = "anthropic:claude-sonnet-4"
   ```

5. **Defaults** (hardcoded in Hone)
   - Default model: Claude Sonnet 4
   - Default budget: unlimited
   - Default context: full model limit

---

## Built-in Agents (Quick Reference)

Use these agents for specialized tasks:

```bash
hone --agent code-reviewer        # Code quality and security reviews
hone --agent debugger              # Systematic bug investigation
hone --agent security-auditor      # Vulnerability and secret scanning
hone --agent performance-optimizer # Profiling and optimization
hone --agent documentation-agent   # API docs and architecture diagrams
hone --agent api-designer          # REST/GraphQL/gRPC design
hone --agent tdd-workflow          # Test-Driven Development
hone --agent rust-expert           # Rust best practices
hone --agent python-expert         # Python best practices
```

Find an agent by keyword: `hone --agent "test-driven"` (matches `tdd-workflow`).

## Built-in Recipes (Quick Reference)

Use these recipes for structured workflows:

```bash
hone --recipe tdd-feature    # Implement feature with TDD (Red-Green-Refactor)
hone --recipe bug-fix        # Fix a bug with regression test
hone --recipe code-review    # Comprehensive code review
hone --recipe refactor       # Refactor safely with test verification
```

## Multi-Agent Features

Hone supports advanced multi-agent patterns:

```bash
# Orchestrator: decompose complex tasks into parallel subtasks
hone --orchestrate "Build a secure user authentication system with tests"

# Ensemble: run 3 candidates with different temperatures, pick the best
hone --ensemble 3 "Optimize this query for performance"

# Complexity routing: auto-route Simple/Medium/Complex to right model
hone "What's 2+2?"  # routes to fast/cheap model

# Test-verify loop: agent runs tests after each change
hone "Add a new feature" --recipe tdd-feature
```

---

## Common Patterns: One-Liners

Quick commands for common tasks:

### Fix a Bug

```bash
hone --agent debugger -i "The login form throws 'TypeError: cannot read password'. Fix it."
```

### Code Review

```bash
hone --agent code-reviewer -i "Review this PR for security issues" --budget 2.00
```

### Implement a Feature (TDD)

```bash
hone --recipe tdd-feature
```

### Refactor Code

```bash
hone --recipe refactor
```

### Add a Feature

```bash
hone --model anthropic:claude-opus-4-5 -i "Add dark mode toggle to Settings. Use Tailwind CSS."
```

### Run on Remote Server

```bash
hone --remote prod.company.com:7777 -i "Deploy the latest build"
```

### Use Local Model (No Cost, No API Key)

```bash
ollama pull llama2
hone --model ollama:llama2 -i "Quick question"
```

### One-Shot with Maximum Privacy

```bash
hone --ephemeral --model ollama:llama2 -i "Analyze this contract"
```

### Set a Budget

```bash
hone --budget 1.00 -i "Long code review" --model anthropic:claude-haiku-3-5
```

---

## Model Quick Guide

**Choose based on your task:**

| Task | Model | Cost | Speed | Context |
|------|-------|------|-------|---------|
| Architecture, complex logic | Opus 4.5 | High | Slower | 200K |
| General coding (best default) | Sonnet 4 | Medium | Fast | 200K |
| Quick fixes, typos | Haiku 3.5 | Low | Very fast | 200K |
| Cost-conscious | Ollama llama2 | Free | Slow | 4K |
| OpenAI alternative | GPT-4o | Medium | Fast | 128K |
| Google alternative | Gemini Pro | Low | Fast | 32K |

**Quick selector:**
- Thinking hard? Use **Opus**
- Coding fast? Use **Sonnet**
- Simple edits? Use **Haiku**
- Tight budget? Use **Ollama** or **Haiku**

---

## Pricing Quick Reference

**Per 1 million tokens:**

| Model | Input | Output | Example (10K in, 1K out) |
|-------|-------|--------|--------------------------|
| Claude Opus 4.5 | $3 | $15 | $0.045 |
| Claude Sonnet 4 | $3 | $15 | $0.045 |
| Claude Haiku 3.5 | $0.80 | $4 | $0.012 |
| GPT-4o | $2.50 | $10 | $0.035 |
| GPT-3.5 Turbo | $0.50 | $1.50 | $0.007 |
| Ollama (local) | Free | Free | $0.00 |

**Typical session costs:**
- Typo fix: ~$0.01
- Small feature: ~$0.05
- Medium refactor: ~$0.15
- Large refactor: ~$0.40

---

## Sandbox Quick Config

**Default (most secure):**

```toml
[sandbox]
platform = "auto"  # seatbelt/landlock/seccomp
allow_network = false
```

**Allow specific paths for reading:**

```toml
[sandbox.tools.file_reader]
allow_read = ["{project_root}", "/usr/lib"]
```

**Allow specific paths for writing:**

```toml
[sandbox.tools.code_generator]
allow_write = ["{project_root}/generated"]
```

**Allow specific commands:**

```toml
[sandbox.tools.shell]
allow_exec = ["/usr/bin/git", "/usr/bin/cargo"]
```

**Allow network (for external APIs):**

```toml
[sandbox.tools.github_api]
allow_network = true
```

See [Security Guide](../tutorials/security.md) for complete reference.

---

## Provider Setup (API Keys)

### Anthropic (Claude)

```bash
export ANTHROPIC_API_KEY="sk-ant-..."
hone --model anthropic:claude-opus-4-5
```

Config:
```toml
[providers.anthropic]
api_key = "${ANTHROPIC_API_KEY}"
```

### OpenAI (GPT-4o)

```bash
export OPENAI_API_KEY="sk-..."
hone --model openai:gpt-4o
```

Config:
```toml
[providers.openai]
api_key = "${OPENAI_API_KEY}"
```

### Local Models (Ollama)

```bash
ollama pull llama2
ollama serve  # terminal 1
hone --model ollama:llama2  # terminal 2
```

No API key needed.

### Claude CLI Pass-Through

```bash
hone --model claude-cli:claude-opus
```

Uses your existing `claude` CLI tool. Must have `claude --version` working.

---

## Daemon Mode (honed)

### Start the Daemon

```bash
honed --port 7777 --config ~/.config/hone/config.toml
```

### Connect Multiple Clients

```bash
hone --remote localhost:7777
```

### Access Control

```toml
[rbac]
enabled = true

[[rbac.roles]]
name = "developer"
permissions = ["sessions:create", "tools:all"]

[[rbac.users]]
user_id = "alice@company.com"
role = "developer"
```

### Health Check

```bash
curl http://localhost:7777/providers/health
```

---

## Memory Management

**Enable memory (learns patterns):**

```toml
[memory]
enabled = true
db_path = "~/.local/share/hone/memory.db"
```

**View memory contents:**

```bash
sqlite3 ~/.local/share/hone/memory.db
sqlite> SELECT pattern, confidence FROM patterns LIMIT 10;
```

**Reset memory (start fresh):**

```bash
rm ~/.local/share/hone/memory.db
```

**Disable for sensitive work:**

```bash
hone --ephemeral
```

---

## Recipes (YAML Workflows)

**Create a recipe file (`refactor.yaml`):**

```yaml
name: refactor-logging
description: Upgrade to structured logging
steps:
  - tool: file_search
    pattern: "console.log|print\\("

  - agent:
      prompt: "Replace with structured logging"
      model: anthropic:claude-sonnet-4

  - tool: git_commit
    message: "refactor: upgrade to structured logging"
```

**Run it:**

```bash
hone --recipe refactor.yaml
```

See [Recipes Guide](../tutorials/recipes.md) for full syntax.

---

## Troubleshooting Quick Fixes

| Problem | Fix |
|---------|-----|
| Model context exceeded | Use smaller model (`--model claude-haiku-3-5`) or reduce context (`--context-window 50000`) |
| Out of budget | Set a budget: `hone --budget 10.00` |
| Sandbox denying access | Add path to allowlist in config under `[sandbox.tools.TOOL_NAME]` |
| Slow startup | Disable indexing: `[index] enabled = false` |
| Out of memory | Use local model or reduce context window |
| API key not working | Check env var: `echo $ANTHROPIC_API_KEY` |
| Daemon not responding | Check port: `curl http://localhost:7777` |

---

## Useful Definitions

- **Token:** Smallest unit of text (roughly 1 word = 1.3 tokens)
- **Context window:** Maximum tokens the model can process (e.g., 200K for Claude)
- **Sandbox:** Kernel-enforced restrictions on what code can do
- **MCP:** Model Context Protocol; a standard for integrating external tools
- **Recipe:** YAML file defining a multi-step workflow
- **Memory:** Local encrypted database of learned patterns
- **Daemon (`honed`):** Long-running server for multi-user sessions
- **RBAC:** Role-Based Access Control; permissions per user
- **Fallback chain:** List of providers to try if one fails

---

## Essential Links

- **Setup:** [Getting Started](../tutorials/getting-started.md)
- **Deep concepts:** [Core Concepts](concepts.md)
- **Answers:** [FAQ](faq.md)
- **Security:** [Security Guide](../tutorials/security.md)
- **Providers:** [Providers Guide](../tutorials/providers.md)
- **Tools:** [Tools and MCP Guide](../tutorials/tools-and-mcp.md)
- **Recipes:** [Recipes Guide](../tutorials/recipes.md)
- **Architecture:** [Architecture Document](../architecture.md)
- **Server:** [Server and Remote Guide](../tutorials/server-and-remote.md)

---

**Last updated:** 2026-04-05  
**Version:** Hone 1.0

For the latest docs, visit: https://github.com/honelang/hone/tree/main/docs
