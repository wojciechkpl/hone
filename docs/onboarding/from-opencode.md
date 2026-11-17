# Migrating from OpenCode to Hone

You know OpenCode. You're comfortable with 75+ providers, LSP integration, client-server architecture, and web UI. Here's how Hone relates—and what's different.

## Concept Mapping

| OpenCode | Hone Equivalent | Notes |
|----------|-----------------|-------|
| 75+ provider support | 75+ providers (same breadth) | Full compatibility |
| OpenCode LSP bridge | LSP bridges (same approach) | Embedded in Hone |
| OpenCode web UI | hone TUI, or the web UI served by honed | TUI is the main interface; the web UI needs `honed` |
| OpenCode client-server | hone CLI + honed daemon | HTTP/2 + SSE instead of WebSocket |
| OpenCode cost tracking | Real-time cost tracking | Per-turn granularity |
| Custom integrations | MCP + Layer 2 tools (ACP is library-only) | More modular |
| OpenCode recipes | 9 built-in recipes | Not compatible: `--recipe` runs only built-in names, not recipe files |

## Before and After

### OpenCode Workflow

```bash
# Start web UI
opencode serve --port 3000
# Open http://localhost:3000

# In UI:
# 1. Select provider (Anthropic, OpenAI, etc.)
# 2. Select model
# 3. Type prompt
# 4. View LSP diagnostics on the side
# 5. Agent makes changes
# 6. Review in web diff viewer
```

### Hone Workflow

```bash
# Option 1: TUI (faster, no web browser or daemon needed)
hone

# Option 2: Web UI, served by the daemon (build it in first: make webui, then rebuild honed)
HONE_BIND=127.0.0.1 honed 7070
open http://127.0.0.1:7070/ui

# In TUI:
# 1. Select provider and model (or use default)
# 2. Type prompt
# 3. View LSP diagnostics (integrated)
# 4. Agent makes changes
# 5. Review diffs (git-native, not custom viewer)
```

**Key difference:** Hone's TUI runs on its own (faster); the web UI needs the `honed` daemon.

## What You Gain

### 1. Kernel Sandboxing

OpenCode has process isolation. Hone adds kernel-level confinement:

**OpenCode:**
```
Process isolation: Parent container, restricted subprocess
Risk: Determined attacker could escape via kernel bugs
```

**Hone:**
```
Kernel sandboxing: OS-native restrictions (Seatbelt/Landlock)
Risk: Attacker must exploit kernel itself (much harder)

Example: Agent tries to read /etc/passwd
  1. Hone syscall intercept (kernel level)
  2. Policy check: deny_read = ["/etc/*"]
  3. Error returned to process: Permission denied
```

On macOS, Hone uses the same sandbox API as Safari.

### 2. Adaptive Memory

OpenCode has basic session memory. Hone learns patterns:

**OpenCode:**
```bash
# Session history stored in JSON
# No cross-session learning
```

**Hone:**
```bash
# Encrypted SQLite with pattern learning
# Hone remembers:
#   - Which tools succeed most often
#   - Which models work best for each task type
#   - Common workflows and their steps
#   - Frequently-used file patterns
```

Example:
```
You: (first session) Run the tests
Hone: [runs pytest]

You: (second session) Fix the failing test
Hone: Based on your pattern, should I run pytest after fixing?
```

### 3. Recipes with Conditional Logic

OpenCode recipes are linear. Hone recipes support branching:

**OpenCode recipe:**
```yaml
steps:
  - find_files: "tests/**/*.py"
  - agent: "Run pytest and fix failures"
  - commit: "fix: test failures"
```

**Hone recipe:**
```yaml
steps:
  - tool: file_search
    pattern: "tests/**/*.py"
  
  - agent:
      prompt: "Run pytest"
      model: openai:gpt-4o
  
  - condition: "if test_results.failed > 0"
    steps:
      - agent:
          prompt: "Fix failing tests"
          model: openai:gpt-4o
      
      - tool: shell
        cmd: "pytest -v"
  
  - condition: "if all tests pass"
    steps:
      - tool: git_commit
        message: "fix: test failures"
      - tool: slack_send
        message: "Tests passing. Ready to merge."
```

Conditional logic enables smarter automation.

### 4. Better LSP Integration

OpenCode embeds LSP bridges. Hone standardizes and extends them:

**Both support:**
- Hover information (types, docs)
- Diagnostics (errors, warnings)
- Code actions (quick fixes)

**Hone adds:**
- Workspace symbols (search code by meaning)
- Semantic indexing (cross-file understanding)
- Cost accounting for LSP calls

```bash
hone "What's the return type of authenticate()?"
# Agent calls: lsp_hover(file="auth.rs", line=42, col=10)
# Hone tracks: 1 LSP call (no cost, local)

hone "Find all calls to the deprecated API"
# Agent calls: semantic_search(query="deprecated API")
# Hone returns: 12 matching locations
```

### 5. Team Governance (RBAC)

OpenCode has basic permissions. Hone has fine-grained RBAC:

**OpenCode:**
```bash
# Basic: authenticated or not
# Limited team features
```

**Hone:**
```bash
# Roles: admin, operator, viewer
# Per-user budgets
# Audit logging (who did what, when)
# Cost tracking per user/team

[team.users]
alice = { role = "admin", budget = "unlimited" }
bob = { role = "operator", budget = "$500/month" }
charlie = { role = "viewer" }
```

Audit log:
```
2026-04-05 12:34:56 [user=alice] Tool: edit_file, path=/src/main.rs
2026-04-05 12:34:57 [user=alice] Tool: shell, cmd=cargo test
2026-04-05 12:34:58 [user=bob] Tool: read_file, path=/src/main.rs
```

### 6. ACP Protocol (Standardized Custom Agents)

OpenCode has custom integrations. Hone standardizes via ACP:

**ACP (Agent Client Protocol):**
- Persistent bidirectional connection
- Full tool support (not limited to JSON-RPC)
- Better for complex integrations

ACP providers are library-only for now: `hone` can't select one. There is no `acp:` model prefix, and a `[providers.acp]` table makes `config.toml` fail to load. See [ACP](../tutorials/acp.md).

## What's Different

### 1. TUI Instead of Web UI

**OpenCode:** Web UI in browser (familiar, but slower, requires port forwarding)

**Hone:** TUI in terminal (faster, lower latency, easier SSH access)

**In comparison:**
- Web UI: Better for non-technical users, remote collaboration
- TUI: Better for developers, lower latency, no browser overhead

Hone also has a web UI, served by `honed` at `/ui` (build it in first: `make webui`, then rebuild `honed`).

### 2. Daemon Architecture

Both OpenCode and Hone have client-server. But different protocols:

**OpenCode:** WebSocket (stateless, polling)

**Hone:** HTTP/2 + SSE (stateful, push-based)

Benefits of Hone's approach:
- Lower latency (push vs. poll)
- Less network traffic
- Better streaming (SSE)
- Easier debugging (HTTP inspector)

### 3. TOML Config Instead of JSON

**OpenCode:** JSON config (UI-editable)

```json
{
  "providers": {
    "default": "anthropic",
    "anthropic": { "api_key": "sk-ant-..." }
  },
  "lsp": { "enabled": true }
}
```

**Hone:** TOML config (human-editable)

```toml
[providers.anthropic]
api_key = "sk-ant-..."

[agent]
default_model = "anthropic:claude-opus-4-5"

[lsp]
enabled = true
```

TOML is easier to edit manually and supports comments.

### 4. Git-Native Diff Review

**OpenCode:** Custom diff viewer in web UI

**Hone:** Git diff (standard tool)

```bash
# In Hone session
git diff          # See changes
git diff --cached # See staged changes
git status        # See file status
```

Why? Standard git tools are more powerful and integrate with your existing workflow.

## Step-by-Step Migration

### 1. Install Hone

```bash
git clone https://github.com/anthropics/hone.git
cd hone
cargo build --release
sudo cp target/release/hone /usr/local/bin/
sudo cp target/release/honed /usr/local/bin/
```

### 2. Migrate Your OpenCode Config

**OpenCode location:** `~/.opencode/config.json` or environment variables

**Hone location:** `~/.config/hone/config.toml`

Example migration:

**OpenCode:**
```json
{
  "providers": {
    "default": "anthropic",
    "anthropic": {
      "api_key": "sk-ant-...",
      "model": "claude-opus-4-5"
    }
  },
  "lsp": { "enabled": true },
  "cost_tracking": { "enabled": true }
}
```

**Hone:**
```toml
[providers.anthropic]
api_key = "sk-ant-..."

[agent]
default_model = "anthropic:claude-opus-4-5"

[lsp]
enabled = true

[pricing.anthropic]
"claude-opus-4-5" = { input_per_mtok = 3.00, output_per_mtok = 15.00 }
```

### 3. Start the Daemon

The daemon is optional: it serves the web UI, the desktop app and the HTTP API. The `hone` CLI doesn't connect to it.

```bash
# If running locally (loopback only, no token needed)
HONE_BIND=127.0.0.1 honed 7070
# Web UI: http://127.0.0.1:7070/ui (build it in first: make webui, then rebuild honed)

# Or remotely on a server (a token is required off loopback)
ssh myserver
export HONE_AUTH_TOKEN="$(openssl rand -hex 32)"
honed 7070
# From your laptop: the desktop app (daemon URL and token) or the HTTP API
```

`honed` has no `--config` option; its settings are environment variables, listed in [Environment variables](../configuration.md#environment-variables) (`honed --help` lists only some of them).

### 4. Test Your First Session

```bash
cd /your/opencode/project
hone
```

Try a familiar task:
```
Refactor the authentication module
```

Hone will:
1. Find related files via semantic search
2. Read them
3. Make changes
4. Show diffs
5. Commit (if you approve)

### 5. Create HONE.md (Project Conventions)

If you had OpenCode system prompts or guidelines:

```markdown
# Project Skills

## Skill: Code Review

Before committing:
1. Run tests with pytest
2. Check for error handling
3. Review with `git diff`

## Skill: Refactoring

When refactoring:
1. Write tests first (if not present)
2. Make changes incrementally
3. Run tests after each change
4. Commit per logical unit
```

### 6. Migrate Your Recipes (If Any)

Hone can't run your OpenCode recipes yet: `--recipe` runs only the 9 built-in recipes by name (`hone --list-recipes`); a recipe file fails with `unknown recipe: <name>`, and it reads no recipes directory. Use the closest built-in recipe instead.

## FAQ

### Can I still use the web UI?

Yes, through `honed`: it serves a web UI at `/ui` once you build it in (`make webui`, then rebuild `honed`). See [What about the web UI? I prefer it.](#what-about-the-web-ui-i-prefer-it) below.

### Do my OpenCode recipes work in Hone?

No. `--recipe` runs only the 9 built-in recipes by name (`hone --list-recipes`); a recipe file fails with `unknown recipe: <name>`.

### How do I set per-user budgets like OpenCode?

In config.toml:

```toml
[team.users]
alice = { budget = "unlimited" }
bob = { budget = "$500/month" }
```

`honed` has no budget options: `--user-budget` and `--budget-period` don't exist.

### Is Hone faster than OpenCode?

For local use: yes (TUI is faster than web). For remote: similar (both use daemon).

### Can I use both OpenCode and Hone?

Yes. They run independently. Keep both until you're comfortable with Hone.

### How do I export sessions like OpenCode?

In session:
```
/export session.json
```

Sessions are also stored in memory database:
```bash
~/.local/share/hone/memory.db
```

### What about the web UI? I prefer it.

`honed` serves one at `/ui` (build it in first: `make webui`, then rebuild `honed`). SSH tunneling works well for remote use: run `HONE_BIND=127.0.0.1 honed 7070` on the server, then from your laptop:

```bash
ssh -L 7070:localhost:7070 myserver
open http://localhost:7070/ui   # the web UI, if it was built into that honed
```

## Next Steps

1. **Install:** Follow step 1 above
2. **Config:** Migrate your OpenCode config to TOML
3. **Daemon (optional):** Run `honed` for the web UI, the desktop app or the HTTP API (`hone` itself doesn't connect to it)
4. **Test:** Try a familiar task
5. **Recipes:** Read [Recipes Guide](../tutorials/recipes.md) for automation
6. **Team:** Read [Server and Remote](../tutorials/server-and-remote.md) for multi-user setup

---

Migrating from OpenCode? Questions? Open an issue on GitHub.
