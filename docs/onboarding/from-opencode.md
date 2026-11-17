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

Example: the agent tries to read ~/.ssh/id_ed25519
  read_file: refused by a rule in permissions.toml
    (path = "~/.ssh/**", decision = "deny")
  shell "cat ~/.ssh/id_ed25519" on macOS: Seatbelt denies the read
    (~/.ssh is not a readable root): Permission denied
```

The OS sandbox covers shell commands only, and has no policy file; file tools are governed
by your permission rules. See [Permissions and sandbox](../permissions-and-sandbox.md).

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

### 5. Audit Logging (No RBAC)

**OpenCode:**
```bash
# Basic: authenticated or not
# Limited team features
```

**Hone:** no roles, users or per-user budgets either. `honed` has one bearer token
(`HONE_AUTH_TOKEN`), and every client that has it gets the same access. A `[team.*]` table
makes `config.toml` fail to load. What Hone has instead:

- **Tool-call log:** `hone --audit` (or `HONE_AUDIT_LOG=1`) writes every tool call, plus one
  `context` line per model call, to `<cwd>/.hone/audit.jsonl`. Entries have no user field.
  `hone trace list` reads it.
- **Run log for the daemon:** with `HONE_PROVENANCE_DIR`, `honed` writes a hash-chained file
  per `POST /run`.
- **Cost totals per model** (not per user): `GET /team/usage` on `honed`.

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

**OpenCode:** JSON config

```jsonc
{
  "$schema": "https://opencode.ai/config.json",
  "model": "anthropic/claude-opus-4-5"
}
```

**Hone:** TOML config (human-editable)

```toml
# ~/.config/hone/config.toml
default_model = "anthropic:claude-opus-4-5"
```

The key comes from `ANTHROPIC_API_KEY`; `config.toml` has no `api_key` field under
`[providers.*]`. LSP has no setting: Hone detects the project's language and starts its
language server if it is installed. TOML is easier to edit manually and supports comments.

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
git clone https://github.com/wojciechkpl/hone.git
cd hone
cargo build --release
sudo cp target/release/hone /usr/local/bin/
sudo cp target/release/honed /usr/local/bin/
```

### 2. Migrate Your OpenCode Config

**OpenCode location:** `~/.config/opencode/opencode.json` (global) or `opencode.json` in
the project ([OpenCode docs](https://opencode.ai/docs/config/), checked 2026-11-17)

**Hone location:** `~/.config/hone/config.toml`

Example migration:

**OpenCode:**
```jsonc
// ~/.config/opencode/opencode.json
{
  "$schema": "https://opencode.ai/config.json",
  "model": "anthropic/claude-opus-4-5",
  "providers": {
    "anthropic": { "settings": { "apiKey": "{env:ANTHROPIC_API_KEY}" } }
  }
}
```

**Hone:**
```bash
export ANTHROPIC_API_KEY="sk-ant-..."
```

```toml
# ~/.config/hone/config.toml
default_model = "anthropic:claude-opus-4-5"
```

- **LSP:** no setting; Hone starts the language server for the project's language if it
  is installed.
- **Cost tracking:** always on. Hone has a built-in price table; to override it or add a
  model, put a `[pricing."provider:model"]` table in your user file,
  `~/.config/hone/config.toml` (see
  [Pricing and `[budget]`](../configuration.md#pricing-and-budget)).

Hone rejects keys it doesn't know (exit code 2); all keys are in
[Configuration](../configuration.md#configtoml).

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

You can't: Hone has no per-user budgets, and a `[team.users]` table makes `config.toml` fail
to load. `honed` has no budget options either (`--user-budget` and `--budget-period` don't
exist). The closest is a cap per session, in `~/.config/hone/config.toml`:

```toml
[budget]
session_limit_usd = 10.0
```

### Is Hone faster than OpenCode?

For local use: yes (TUI is faster than web). For remote: similar (both use daemon).

### Can I use both OpenCode and Hone?

Yes. They run independently. Keep both until you're comfortable with Hone.

### How do I export sessions like OpenCode?

In session:
```
/export session.json
```

One-shot runs are also saved in `~/.local/share/hone/sessions.db` (list them with
`hone session list`); the TUI doesn't save sessions yet. `memory.db` holds long-term memory,
not sessions.

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
