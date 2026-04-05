# Migrating from Claude Code to Hone

You know Claude Code. You're comfortable with CLAUDE.md, slash commands, the agent loop, and extended thinking. Here's how those concepts map to Hone—and what you gain.

## Concept Mapping

| Claude Code | Hone Equivalent | Status |
|-------------|-----------------|--------|
| `CLAUDE.md` (project conventions) | `HONE.md` | Implemented |
| `/compact` (reduce verbosity) | `/compact` | Implemented |
| `/model <name>` (switch models) | `/model <provider:model>` | Implemented |
| Agent tools (Read, Edit, Bash) | Layer 2 tools (read_file, edit_file, shell) | Implemented |
| Memory (remember context) | hone-memory (encrypted SQLite, adaptive learning) | Implemented |
| Extended thinking | Model routing (use opus for complex, sonnet for speed) | Implemented |
| TodoWrite tool | Recipes (YAML workflows) | Implemented |
| Agent orchestration | Multi-agent mode (plan + execute) | Planned |
| Hooks (pre/post commit) | config.toml hooks | Planned |
| MCP servers | MCP Layer 1 (same protocol, better integration) | Implemented |

## Before and After

### Claude Code Session

```bash
# Set up CLAUDE.md with conventions
cat > CLAUDE.md << 'EOF'
# Python Testing

Always use pytest. After writing code, run `pytest -v` to verify.
EOF

# Start interactive session
claude

# In session: agent reads CLAUDE.md, learns conventions (token cost: ~500 tokens)
# Type: Fix the failing test in test_utils.py
# Agent loops: reads file → finds bug → fixes → runs pytest → commits
```

**Cost:** Every session re-reads CLAUDE.md (token overhead).

### Hone Session

```bash
# Set up HONE.md (same format, same content)
cat > HONE.md << 'EOF'
# Python Testing

Always use pytest. After writing code, run `pytest -v` to verify.
EOF

# Start interactive session
hone

# HONE.md parsed once at startup (no token cost, compiled into system prompt)
# Type: Fix the failing test in test_utils.py
# Agent loops: reads file → finds bug → fixes → runs pytest → commits
```

**Benefit:** HONE.md is zero-cost. It's parsed once and baked into the system prompt (no token streaming overhead).

## What You Gain

### 1. Provider Freedom

Claude Code locks you to Anthropic. Hone supports 75+ providers:

```bash
# Same session, different models
hone --model anthropic:claude-opus-4-5   # Most capable
hone --model openai:gpt-4o                # Alternative
hone --model ollama:llama2                # Free, local
hone --model claude-cli:claude-opus       # Your existing CLI
```

Hot-swap mid-session without losing history:
```
You: /model openai:gpt-4o
Hone: Switched to gpt-4o. Conversation preserved.
```

### 2. Kernel Sandboxing

Claude Code relies on process isolation. Hone uses kernel-level sandboxing:

- **macOS:** Seatbelt (same as Safari)
- **Linux:** Landlock (same as systemd)
- **Other:** seccomp (same as Docker)

Tool calls are auditable and confined:

```bash
# See the sandbox policy in effect
hone --show-sandbox-policy
# Output: Using Seatbelt on macOS
#   Deny network (except: api.anthropic.com, api.openai.com)
#   Deny /etc, /var, /tmp (except: /tmp/.hone-*)
#   Allow read: project root, ~/.local/share/hone
#   Allow execute: /bin, /usr/bin, cargo, python3
```

### 3. Adaptive Memory

Claude Code has basic memory. Hone's memory learns patterns:

```bash
# After a few sessions where you run pytest, then clippy, then commit...
# Hone learns this workflow and suggests it automatically

You: Fix the bug in main.rs
Hone: Based on your recent pattern, should I also run `cargo clippy` and commit?
```

Memory is encrypted, stored locally in SQLite:

```toml
[memory]
db_path = "~/.local/share/hone/memory.db"
encryption_key_file = "~/.config/hone/key.bin"
```

### 4. Local Models (Free)

Run private, zero-cost inference:

```bash
ollama pull llama2
ollama serve  # in another terminal

hone --model ollama:llama2
# No API key needed, no privacy concerns
```

### 5. Remote Daemon with Team Governance

Claude Code is single-user, single-machine. Hone can run as a server:

```bash
# On a shared machine
honed --port 7070

# From your laptop (another terminal or network)
hone --remote myserver:7070
```

Team governance with roles:
- `admin` — manage settings, users, budgets
- `operator` — run sessions, read all code
- `viewer` — read-only access

See [Server and Remote](../tutorials/server-and-remote.md).

## What's Different

### 1. CLI Model Spec

**Claude Code:**
```bash
claude --model claude-opus-4-5
```

**Hone:**
```bash
hone --model anthropic:claude-opus-4-5
# Format: <provider>:<model>
# Providers: anthropic, openai, ollama, claude-cli, gemini-cli, acp, ...
```

### 2. Built with Rust (Not Node.js)

Claude Code is a Node.js CLI. Hone is a Rust binary.

**Benefits:**
- Single, self-contained binary (no Node.js dependency)
- Faster startup (~100ms vs ~500ms)
- Smaller memory footprint
- Native support for Seatbelt/Landlock sandboxing

**For you:** No behavioral difference. Just works.

### 3. Recipes Replace Inline Prompts

**Claude Code approach:**
```bash
# Use TodoWrite to track inline TODOs
claude << 'EOF'
Read src/main.rs, find all TODO comments, create issues on GitHub
EOF
```

**Hone approach:**
```yaml
# Create a recipe: scripts/process-todos.yaml
name: process-todos
description: Find TODOs and create GitHub issues
steps:
  - tool: file_search
    query: "TODO|FIXME"
  - agent:
      prompt: "For each TODO, create a GitHub issue"
      model: anthropic:claude-opus-4-5

# Run it anytime
hone --recipe scripts/process-todos.yaml
```

Benefits of recipes:
- Reusable across projects
- Shareable with teammates
- Versioned with git
- No token overhead (syntax is simple YAML, not natural language)

See [Recipes Guide](../tutorials/recipes.md).

### 4. Daemon-First Architecture

Claude Code is CLI-only. Hone defaults to daemon:

```bash
# Start the daemon (persists across sessions)
honed

# In another terminal, connect
hone --remote localhost:7070

# Sessions are stored, replayable, auditable
```

For local use, the CLI still works:
```bash
hone  # Works as before, no daemon needed
```

## Step-by-Step Migration

### 1. Install Hone

```bash
git clone https://github.com/anthropics/hone.git
cd hone
cargo build --release
sudo cp target/release/hone /usr/local/bin/
```

See [Getting Started](../tutorials/getting-started.md) for details.

### 2. Copy Your CLAUDE.md to HONE.md

```bash
cd /your/project
cp CLAUDE.md HONE.md  # Exact same format, Hone understands it
```

Hone parses HONE.md at startup (zero token cost).

### 3. Configure Providers

Copy your API keys from Claude Code config to Hone config:

**Claude Code location:** `~/.claude/config.json` or env vars

**Hone location:** `~/.config/hone/config.toml`

Example migration:

```bash
# If you had Claude Code using Anthropic API
export ANTHROPIC_API_KEY="sk-ant-..."

# Hone will read the same env var automatically
hone --model anthropic:claude-opus-4-5
```

Or create `~/.config/hone/config.toml`:

```toml
[providers.anthropic]
api_key = "sk-ant-..."

[agent]
default_model = "anthropic:claude-opus-4-5"
context_window_tokens = 16000
max_iterations = 20

[memory]
enabled = true
db_path = "~/.local/share/hone/memory.db"
```

### 4. Run Your First Session

```bash
cd /your/project
hone
```

You'll see:
- HONE.md loaded (0 tokens spent)
- Agent ready with all conventions
- Familiar tool calls (read, edit, shell)
- Slash commands (same as Claude Code)

### 5. Migrate Inline Workflows to Recipes (Optional)

If you had TodoWrite tasks:

**Before (Claude Code):**
```bash
claude << 'EOF'
1. Read tests/
2. Run pytest
3. For each failure, fix the code
4. Commit changes
EOF
```

**After (Hone recipes):**

Create `scripts/fix-tests.yaml`:

```yaml
name: fix-failing-tests
description: Find and fix test failures
steps:
  - tool: file_search
    pattern: "tests/**/*.py"
  
  - agent:
      prompt: "Run pytest and identify failures"
      model: anthropic:claude-opus-4-5
  
  - agent:
      prompt: "For each failure, fix the underlying code"
      model: anthropic:claude-opus-4-5
  
  - tool: git_commit
    message: "fix: resolve failing tests"
```

Run it:
```bash
hone --recipe scripts/fix-tests.yaml
```

## FAQ

### Do I need to rewrite my CLAUDE.md?

No. Copy it to HONE.md as-is. Same format, same content. Hone understands it.

### Will my memory transfer?

No. Claude Code and Hone have separate memory systems. But Hone's memory is more powerful—it learns patterns automatically. You'll build up new memories quickly.

### Can I use both Claude Code and Hone?

Yes. They run independently. Keep both if you want to compare, then switch fully when ready.

### How do I set a budget like Claude Code's `/budget`?

Same command:
```
/budget 10.00  # max $10 per session
```

Or in config.toml:
```toml
[agent]
max_spend_usd = 10.00
```

### What about extended thinking?

Hone supports model-specific features via `/model`:
```bash
# Use a model that supports extended thinking
hone --model anthropic:claude-opus-4-5  # supports extended thinking
hone --model openai:gpt-4o              # does not (yet)
```

Hone also routes complex tasks automatically (planned feature).

## Next Steps

1. **Install:** Follow step 1 above
2. **Configure:** Create `~/.config/hone/config.toml` or set env vars
3. **Test:** Run `hone` in a small project and try a familiar task
4. **Explore:** Read [Providers Guide](../tutorials/providers.md) for hot-swapping
5. **Recipes:** Read [Recipes Guide](../tutorials/recipes.md) for automation
6. **Tools:** Read [Tools and MCP](../tutorials/tools-and-mcp.md) to extend capabilities

---

Migrating from Claude Code? Have questions? Open an issue on GitHub.
