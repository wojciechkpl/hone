# Migrating from Codex CLI to Hone

You know Codex CLI. You're comfortable with kernel sandboxing, approval workflows, and running code safely. Here's how Hone takes those ideas further—and what's new.

## Concept Mapping

| Codex CLI | Hone Equivalent | Notes |
|-----------|-----------------|-------|
| seccomp sandbox | Seatbelt/Landlock/seccomp | Same approach, kernel-level |
| Approval workflow | Approval workflow + diffs | Implemented, more granular |
| OpenAI models | 75+ providers (including OpenAI) | Not locked to OpenAI |
| suggest mode | Agent loop with streaming | Same paradigm |
| auto-edit mode | AutoCommitter tool | Implemented |
| full-auto mode | Recipes with multi-step automation | More structured, auditable |
| Cost tracking | Real-time cost tracking per turn | Better visibility |
| Diff review | git_diff tool + approval | Natural git integration |

## Before and After

### Codex CLI Workflow

```bash
# Start session (suggest mode)
codex suggest

# In session:
# Prompt: Fix the null pointer exception in main.rs
# Codex suggests changes, you review diffs
# Accept or reject

# For automation, switch to auto-edit mode
codex auto-edit

# Prompt: Add logging to all database queries
# Codex makes changes automatically, asks for approval
```

**Strengths:** Safe by default (approvals required), OpenAI integration, cost tracking.

### Hone Workflow

```bash
# Start session
hone

# In session:
# Prompt: Fix the null pointer exception in main.rs
# Hone loops: reads file, identifies issue, suggests fix
# Shows diff (same as Codex)
# You accept or reject

# For automation, use a recipe
hone --recipe scripts/add-logging.yaml

# Executes multi-step workflow:
#   1. Find database queries
#   2. Add structured logging
#   3. Run tests
#   4. Commit (if approved)
```

**Key differences:**
- Approvals are more flexible (per-turn, per-tool, per-file)
- Recipes replace mode switching (more composable)
- Sandbox is kernel-enforced (more secure)

## What You Gain

### 1. Kernel Sandboxing (Better Than seccomp)

Codex CLI uses seccomp. Hone uses OS-native sandboxing:

| Platform | Codex CLI | Hone | Benefit |
|----------|-----------|------|---------|
| macOS | seccomp (emulated) | Seatbelt (native) | Same API as Safari, more reliable |
| Linux | seccomp | Landlock (newer) | Supports deny-on-access, more flexible |
| Other | seccomp | seccomp | Same |

**Real example:**

```bash
hone --show-sandbox-policy

# Output:
# Sandbox: Seatbelt on macOS
#   Deny: /etc/* (except whitelisted)
#   Deny: /var/* (except ~/.local/share/hone)
#   Deny: Network (except api.openai.com, api.anthropic.com)
#   Allow: read /Users/you/project, execute /bin/bash, /usr/bin/python3
```

Policies are auditable and configurable.

### 2. Provider Freedom (No OpenAI Lock-In)

Codex CLI only works with OpenAI. Hone supports 75+ providers:

```bash
hone --model openai:gpt-4o              # OpenAI (match Codex CLI)
hone --model anthropic:claude-opus-4-5  # Anthropic (more capable)
hone --model ollama:llama2              # Local (free, private)
```

Hot-swap mid-session:
```
You: /model anthropic:claude-opus-4-5
Hone: Switched to claude-opus-4-5. Conversation preserved.
```

Why this matters:
- Vendor negotiating power (shop around for pricing)
- Avoid API outages (fallback chains)
- Use local models for sensitive code
- Experiment with different models

### 3. Recipes (Better Than Mode Switching)

Codex CLI has 3 modes: suggest, auto-edit, full-auto. Hone has recipes:

**Codex CLI:**
```bash
# Mode switching
codex suggest   # interactive, show diffs
codex auto-edit # auto-apply, ask for approval
codex full-auto # fully automatic (risky)
```

**Hone:**
```yaml
# recipes/add-logging.yaml
name: add-structured-logging
description: Add structured logging to database queries

approvals:
  require_approval: true
  on: ["edit_file", "git_commit"]  # approve edits and commits

steps:
  - tool: file_search
    pattern: "src/**/*.rs"
    
  - agent:
      prompt: "Find database query calls"
      model: openai:gpt-4o
  
  - agent:
      prompt: "Add structured logging to each query"
      model: openai:gpt-4o
      approval_before: true  # ask before agent edits
  
  - tool: shell
    cmd: "cargo test"
    approval_before: true
  
  - tool: git_commit
    message: "feat: add structured logging to database queries"
    approval_before: true
```

Run it:
```bash
hone --recipe scripts/add-logging.yaml
# Executes step by step, asks for approval at each approval point
```

Benefits:
- Fully composable (tools + agents + conditions)
- Auditable (each step is logged)
- Reusable (version with git)
- Safer than "full-auto" (explicit approval points)

### 4. Real-Time Cost Tracking (Transparent Billing)

Both Codex CLI and Hone track costs. Hone is more granular:

**Codex CLI:**
```bash
# Session total at end
Total cost: $0.42 (OpenAI API usage)
```

**Hone:**
```bash
# Per-turn cost tracking (streaming)
Usage: 4521 input tokens, 312 output tokens
Cost: $0.019 USD (openai:gpt-4o)
Session total: $0.24 USD
Budget remaining: $9.76 / $10.00

# Turn 2
Usage: 1200 input tokens, 89 output tokens
Cost: $0.004 USD
Session total: $0.28 USD
```

Plus built-in budget limits:
```bash
hone --budget 5.00
# Session stops when $5.00 is spent
```

Or in config:
```toml
[agent]
max_spend_usd = 5.00
```

### 5. LSP Integration (No Language Gaps)

Codex CLI has basic language support. Hone bridges all language servers:

```bash
hone -i "Fix type errors in this Rust file"
# Agent calls: lsp_diagnostics(file="main.rs")
# Sees all type errors from rust-analyzer
# Fixes them systematically

hone -i "What does this Python function do?"
# Agent calls: lsp_hover(file="utils.py", line=42, col=10)
# Returns docstring + type info from Pylance
```

Supports Python, Rust, TypeScript, Go, Java, C++, etc.

### 6. Multi-User & Audit Logging

Codex CLI is single-user. Hone can be multi-user with full audit logging:

```bash
# Start daemon
honed --port 7070 --audit-log /var/log/hone/audit.log

# From your laptop
hone --remote myserver:7070

# Audit log shows:
# 2026-04-05 12:34:56 [user=alice, session=abc123]
#   Tool: read_file, path=/home/alice/project/src/main.rs
#   Result: OK
# 2026-04-05 12:34:57 [user=alice, session=abc123]
#   Tool: shell, cmd=cargo test
#   Result: OK (0 failures)
# 2026-04-05 12:35:00 [user=alice, session=abc123]
#   Tool: git_commit, message=fix: null pointer exception
#   Result: OK
```

Useful for:
- Compliance (HIPAA, SOC 2, etc.)
- Team accountability
- Security investigations
- Cost tracking per user

## What's Different

### 1. Approval Workflow Is More Granular

**Codex CLI:** Approve or reject entire turn

**Hone:** Approve specific actions:

```bash
# In session, Hone shows each tool call
[Tool: read_file] src/main.rs
[Tool: edit_file] src/main.rs (line 42)
  - Old: if ptr != null { ... }
  - New: if let Some(ptr) = ptr { ... }
  [Approve? y/n] y

[Tool: shell] cargo test
[Output: 15 tests passed]
  [Approve? y/n] y

[Tool: git_commit] fix: null pointer exception
  [Approve? y/n] y
```

More fine-grained control.

### 2. Recipes Are YAML, Not Modes

Codex CLI modes are implicit. Hone recipes are explicit YAML files.

**Benefit:** Reusable, versionable, shareable.

### 3. Sandbox Policy Is Configurable

Codex CLI sandbox is hardcoded. Hone sandbox is policy-based:

```toml
# ~/.config/hone/config.toml

[sandbox]
strategy = "seatbelt"  # macOS

[sandbox.policies.file_system]
allow_read = ["/Users/you/project", "/usr/lib", "/usr/local/bin"]
deny_read = ["/etc", "/var", "/private/var"]
allow_write = ["/Users/you/project", "/tmp"]
deny_write = ["/etc", "/var", "/private/var"]

[sandbox.policies.network]
deny_all = true
allow_exceptions = ["api.openai.com", "github.com"]

[sandbox.policies.process]
deny_all = true
allow_execute = ["/bin/bash", "/usr/bin/python3", "/usr/local/bin/cargo"]
```

You can tighten or loosen policies per project.

### 4. Distributed Execution

Codex CLI is single-machine. Hone can run on a server:

```bash
# Codex CLI: always local
codex suggest

# Hone: local or remote
hone                        # local
hone --remote myserver:7070 # remote (daemon)
```

Remote daemon handles authentication, audit logging, cost tracking per user.

## Step-by-Step Migration

### 1. Install Hone

```bash
git clone https://github.com/anthropics/hone.git
cd hone
cargo build --release
sudo cp target/release/hone /usr/local/bin/
```

### 2. Configure OpenAI (Your Existing Provider)

If you've been using Codex CLI with OpenAI:

```bash
export OPENAI_API_KEY="sk-..."  # same as Codex CLI

# Create config
mkdir -p ~/.config/hone
cat > ~/.config/hone/config.toml << 'EOF'
[providers.openai]
api_key = "${OPENAI_API_KEY}"

[agent]
default_model = "openai:gpt-4o"
max_spend_usd = 10.00  # your budget

[sandbox]
strategy = "seatbelt"  # macOS
EOF
```

### 3. Test Your First Session

```bash
cd /your/codex/project
hone
```

Try a familiar task:
```
Fix the null pointer exception in main.rs
```

Hone will:
1. Find the file
2. Read it
3. Identify the issue
4. Suggest a fix
5. Show the diff
6. Ask for approval
7. Apply the fix
8. Commit (if you approve)

### 4. Create Your First Recipe

If you had workflows you ran repeatedly with Codex CLI, turn them into recipes.

Example: "Refactor legacy Python to use type hints"

Create `scripts/add-type-hints.yaml`:

```yaml
name: add-type-hints
description: Add type hints to legacy Python code

approvals:
  require_approval: true
  on: ["edit_file", "git_commit"]

steps:
  - tool: file_search
    pattern: "src/**/*.py"
  
  - agent:
      prompt: "Add comprehensive type hints to these Python files"
      model: openai:gpt-4o
      approval_before: true
  
  - tool: shell
    cmd: "python -m mypy src/"
    approval_before: false  # no approval for read-only check
  
  - tool: git_commit
    message: "refactor: add type hints to Python modules"
    approval_before: true
```

Run it:
```bash
hone --recipe scripts/add-type-hints.yaml
```

### 5. Explore Approval Workflows

Unlike Codex CLI's rigid modes, Hone lets you customize approval points:

```yaml
steps:
  - agent:
      prompt: "Refactor the main function"
      approval_before: true   # approve before agent runs
      approval_after: false   # auto-apply changes (still review via git_diff)
  
  - tool: git_diff
    # Show changes (read-only)
  
  - tool: git_commit
    message: "refactor: improve main function"
    approval_before: true    # approve before committing
```

## FAQ

### Can I use the same OpenAI API key with Hone?

Yes. Set `OPENAI_API_KEY` env var or config file. Hone reads the same keys.

### Is Hone's sandbox as safe as Codex CLI's?

Yes, and arguably better. Hone uses OS-native sandboxing (Seatbelt on macOS, Landlock on Linux) instead of seccomp emulation. Policies are auditable.

### Do recipes replace approval workflows?

No. Recipes define multi-step workflows; approvals control which steps require manual review. Use together:
- Recipes for orchestration
- Approvals for safety

### How do I migrate my Codex CLI workflows?

Turn them into recipes. Example:
- Codex CLI "full-auto mode" → Hone recipe with `approval_before: false` on most steps
- Codex CLI "suggest mode" → Hone session with `/model` and manual approval per turn

### Can I set per-step budgets?

Not yet, but you can set global budget:
```toml
[agent]
max_spend_usd = 10.00
```

When recipe hits the budget, it stops.

### What about rate limiting?

Hone respects provider rate limits. If OpenAI rate-limits you:
1. Hone waits (configurable timeout)
2. Falls back to next provider (if configured)
3. Logs the event for debugging

## Next Steps

1. **Install:** Follow step 1 above
2. **Config:** Create `~/.config/hone/config.toml` with OpenAI settings
3. **Test:** Run `hone` and try a familiar task
4. **Recipes:** Read [Recipes Guide](../tutorials/recipes.md) to automate workflows
5. **Security:** Read [Security Guide](../security.md) for sandbox policies
6. **Team:** Read [Server and Remote](../tutorials/server-and-remote.md) for multi-user setup

---

Migrating from Codex CLI? Questions? Open an issue on GitHub.
