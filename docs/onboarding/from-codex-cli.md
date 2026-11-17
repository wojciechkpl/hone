# Migrating from Codex CLI to Hone

You know Codex CLI. You're comfortable with kernel sandboxing, approval workflows, and running code safely. Here's how Hone takes those ideas further—and what's new.

## Concept Mapping

| Codex CLI | Hone Equivalent | Notes |
|-----------|-----------------|-------|
| Sandbox (Seatbelt on macOS, bubblewrap on Linux; on by default) | Seatbelt on macOS (always on); Landlock on Linux (off by default) | Shell commands only; no OS sandbox on other platforms |
| Approval workflow | Approval workflow + diffs | Implemented, more granular |
| OpenAI models | 75+ providers (including OpenAI) | Not locked to OpenAI |
| suggest mode | Agent loop with streaming | Same paradigm |
| auto-edit mode | AutoApprove in the TUI (<kbd>Shift</kbd>+<kbd>Tab</kbd>) | Stops approval prompts; permission rules still apply |
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

# For automation, use a built-in recipe (recipe files can't be run yet)
hone --recipe refactor --var test_command="cargo test"

# Executes multi-step workflow, without pausing:
#   1. baseline (run the tests)
#   2. analyze
#   3. execute
#   4. verify
#   cargo test runs after each step
```

**Key differences:**
- Approvals are more flexible (per-turn, per-tool, per-file)
- Recipes replace mode switching (more composable)
- Shell commands run under a kernel sandbox: Seatbelt on macOS; Landlock on Linux, off by default

## What You Gain

### 1. Kernel Sandboxing

Codex CLI sandboxes commands by default: Seatbelt on macOS and bubblewrap (`bwrap`) on
Linux ([Codex docs](https://learn.chatgpt.com/codex/sandboxing), checked 2026-11-17).
Hone's sandbox is on by default only on macOS.

Hone runs shell commands under the platform's own sandbox:

| Platform | Codex CLI | Hone |
|----------|-----------|------|
| macOS | Seatbelt, on by default | Seatbelt, always on |
| Linux | bubblewrap, on by default | Landlock (kernel 5.13+), **off by default**; turn it on with `HONE_SANDBOX_ENFORCE=1` |
| Other | See the Codex docs | No OS sandbox |

The sandbox covers only the `shell` tool. File tools, git tools and `web_fetch` are not
OS-sandboxed; your permission rules govern them.

**Real example:** on macOS, shell commands can write only the working directory, `/tmp` and `/dev/null`, and get no network. There is no command that prints the policy; [Permissions and sandbox](../permissions-and-sandbox.md) has the full table per platform.

Your own permission rules live in `~/.config/hone/permissions.toml` and `<repo>/.hone/permissions.toml`.

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

`hone --recipe` can't run this file yet: `--recipe` runs only the 9 built-in recipes by name (`hone --list-recipes`); a recipe file fails with `unknown recipe: <name>`. Recipes also never pause for approval.

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

Plus built-in budget limits, in `~/.config/hone/config.toml`:
```toml
[budget]
session_limit_usd = 5.00
# The next turn stops once the session has spent more than $5.00
```

Or in the TUI: `/budget 5`.

### 5. LSP Integration (No Language Gaps)

Codex CLI has basic language support. Hone bridges all language servers:

```bash
hone "Fix type errors in this Rust file"
# Agent calls: lsp_diagnostics(file="main.rs")
# Sees all type errors from rust-analyzer
# Fixes them systematically

hone "What does this Python function do?"
# Agent calls: lsp_hover(file="utils.py", line=42, col=10)
# Returns docstring + type info from Pylance
```

Supports Python, Rust, TypeScript, Go, Java, C++, etc.

### 6. Multi-User & Audit Logging

Codex CLI is single-user. Hone's daemon, `honed`, serves several clients at once and can keep a tamper-evident log of every run:

```bash
# Start daemon. There is no --audit-log option: HONE_PROVENANCE_DIR (an existing
# directory) gets one hash-chained JSONL file per POST /run.
mkdir -p /var/log/hone/provenance
export HONE_AUTH_TOKEN="$(openssl rand -hex 32)"
HONE_PROVENANCE_DIR=/var/log/hone/provenance honed 7070
```

From your laptop, connect the desktop app (daemon URL and token) or call the HTTP API with the token; the `hone` CLI doesn't connect to a daemon. `GET /provenance/{id}/verify` checks a log's chain.

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

### 3. Permission Rules Are Configurable, the Sandbox Is Not

Hone's OS sandbox has no policy file. It wraps shell commands only, and the platform picks
it: on macOS, Seatbelt is always on (commands can write only the working directory, `/tmp`
and `/dev/null`, and get no network); on Linux, Landlock is opt-in with
`HONE_SANDBOX_ENFORCE=1` and restricts files only. `HONE_SANDBOX_EXTRA_READ` adds read-only
roots. `config.toml` has no `[sandbox.*]` tables (they make it fail to load), and its
`[sandbox]` table is not applied.

Rules for files, hosts and commands go in `permissions.toml`:

```toml
# ~/.config/hone/permissions.toml
# Files: checked for read_file, write_file, edit_file, grep, ...
[[rules]]
id = "no-ssh-keys"
path = "~/.ssh/**"
decision = "deny"

[[rules]]
id = "ask-ci"
path = ".github/**"
op = "write"
decision = "ask"

# Hosts: checked for web_fetch
[[rules]]
id = "github-ok"
host = "github.com"
decision = "allow"

[[rules]]
id = "other-hosts"
host = "*"
decision = "ask"

# Commands: checked for shell and the git tools
[[rules]]
id = "cargo-ok"
command = "cargo"
decision = "allow"

[[rules]]
id = "other-commands"
command = "*"
decision = "ask"
```

The first matching rule wins. A project's `<repo>/.hone/permissions.toml` can only tighten
your rules, never loosen them. See [Permissions and sandbox](../permissions-and-sandbox.md).

### 4. Distributed Execution

Codex CLI is single-machine. Hone can run on a server:

```bash
# Codex CLI: always local
codex suggest

# Hone: local, or orchestrated across honed workers
hone                                                       # local
hone --orchestrate --distributed http://myserver:7070 "…"  # through a honed coordinator (needs HONE_AUTH_TOKEN)
```

The coordinator and workers are `honed` processes. The coordinator needs `HONE_DISTRIBUTED_DB=<path>` (without it, `/distributed/*` returns 503) and each worker needs `HONE_WORKER_COORDINATOR=<url>`; see [Run the daemon](../getting-started.md#8-run-the-daemon-honed).

The daemon handles authentication (one bearer token), a run log (`HONE_PROVENANCE_DIR`) and cost totals per model (`GET /team/usage`), not per user.

## Step-by-Step Migration

### 1. Install Hone

```bash
git clone https://github.com/wojciechkpl/hone.git
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
default_model = "openai:gpt-4o"

[budget]
session_limit_usd = 10.0  # your budget
EOF
```

Hone reads the key from `OPENAI_API_KEY`, so it doesn't go in `config.toml`. There is no
sandbox setting either: see [section 3](#3-permission-rules-are-configurable-the-sandbox-is-not)
above.

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

`hone --recipe` can't run this file yet: `--recipe` runs only the 9 built-in recipes by name (`hone --list-recipes`); a recipe file fails with `unknown recipe: <name>`.

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

Yes. Set the `OPENAI_API_KEY` environment variable (or save the key with `hone configure`).
Hone reads the same keys.

### Is Hone's sandbox as safe as Codex CLI's?

Not on Linux by default. Codex CLI sandboxes commands by default on macOS and Linux.

On Hone's side: shell commands run under Seatbelt on macOS (always on). On Linux, Landlock
is off by default; set `HONE_SANDBOX_ENFORCE=1` to turn it on (kernel 5.13+). Other
platforms have no OS sandbox. File tools are not OS-sandboxed; your permission rules govern
them. See [Permissions and sandbox](../permissions-and-sandbox.md).

### Do recipes replace approval workflows?

No. Recipes define multi-step workflows; approvals control which steps require manual review. Use together:
- Recipes for orchestration
- Approvals for safety

### How do I migrate my Codex CLI workflows?

Turn them into recipes. Example:
- Codex CLI "full-auto mode" → Hone recipe with `approval_before: false` on most steps
- Codex CLI "suggest mode" → Hone session with `/model` and manual approval per turn

### Can I set per-step budgets?

No, but you can cap the whole session in `~/.config/hone/config.toml`:
```toml
[budget]
session_limit_usd = 10.0
```

Once the session has spent more than that, each further turn is refused, recipe steps
included.

### What about rate limiting?

Hone respects provider rate limits. If OpenAI rate-limits you:
1. Hone waits and retries (there is no setting for the wait)
2. Falls back to the next of your `fallback_models` (if configured; see
   [Routing and fallback](../configuration.md#routing-and-fallback))
3. Logs the event for debugging

## Next Steps

1. **Install:** Follow step 1 above
2. **Config:** Create `~/.config/hone/config.toml` with OpenAI settings
3. **Test:** Run `hone` and try a familiar task
4. **Recipes:** Read [Recipes Guide](../tutorials/recipes.md) to automate workflows
5. **Security:** Read [Permissions and sandbox](../permissions-and-sandbox.md) for permission rules and the sandbox
6. **Team:** Read [Server and Remote](../tutorials/server-and-remote.md) for multi-user setup

---

Migrating from Codex CLI? Questions? Open an issue on GitHub.
