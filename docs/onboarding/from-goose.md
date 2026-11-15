# Migrating from Goose to Hone

You know Goose. You're comfortable with MCP-native architecture, recipes, extensions, and provider flexibility. Here's how Hone builds on those ideas—and what's new.

## Concept Mapping

| Goose | Hone Equivalent | Notes |
|-------|-----------------|-------|
| Extensions (custom tools) | Layer 2 native tools + Layer 1 MCP | Hone has faster native layer + MCP |
| Recipes (YAML workflows) | Recipes (same YAML format) | 100% compatible with Goose recipes |
| Providers | Providers (same abstraction) | 75+ providers, same flexibility |
| CLI providers (custom agents) | `claude-cli:` / `gemini-cli:` pass-through | ACP providers are library-only; `hone` can't select one yet |
| MCP servers | MCP Layer 1 (JSON-RPC) | Same protocol, same servers work |
| Goose config | HONE.md + config.toml | Config is TOML, conventions are HONE.md |

## Before and After

### Goose Workflow

```bash
# Create a recipe with custom tool
cat > recipes/analyze.yaml << 'EOF'
name: code-analysis
steps:
  - provider: gpt-4
  - extension: my-custom-linter  # custom Python extension
    input: "Analyze src/main.rs"
  - mcp: github
    call: create_issue
    with:
      title: "Linting issues found"
      body: "{output}"
EOF

goose --recipe recipes/analyze.yaml
```

### Hone Workflow

```bash
# Same recipe, simpler tooling
cat > recipes/analyze.yaml << 'EOF'
name: code-analysis
steps:
  - tool: file_search
    pattern: "src/**"
  - agent:
      prompt: "Analyze code for issues"
      model: openai:gpt-4
  - tool: github_create_issue
    title: "Linting issues found"
    body: "{output}"
EOF

hone --recipe recipes/analyze.yaml
```

**Key difference:** Hone splits tools into layers:
- **Layer 2** (native) — file_search, shell, git, LSP (fast, no IPC)
- **Layer 1** (MCP) — github, jira, slack, databases (flexible, pluggable)

## What You Gain

### 1. Kernel Sandboxing

Goose relies on process isolation. Hone adds kernel-level containment:

There is no command that prints the sandbox policy. On macOS, shell commands can write only the working directory, `/tmp` and `/dev/null`, and get no network; [Permissions and sandbox](../permissions-and-sandbox.md) lists what each platform allows, and your own rules live in `~/.config/hone/permissions.toml` and `<repo>/.hone/permissions.toml`.

Tool calls are confined at kernel level. No privilege escalation possible.

### 2. Native Layer (Zero-Cost Tools)

Goose routes everything through MCP (or extensions). Hone has a native layer for speed:

**Goose approach:**
```bash
# Every file read goes through extension IPC
extension: file_reader
  call: read_file
  with: path="main.rs"
# Latency: 10-100ms (IPC overhead)
```

**Hone approach:**
```bash
# Direct Rust function call, <1ms latency
hone "Read main.rs"
# Agent calls: read_file(path="main.rs")
# Latency: <1ms (no IPC)

# For MCP tools (if needed):
hone "Open a GitHub issue"
# Agent calls: github_create_issue(...)
# Latency: 10-100ms (JSON-RPC)
```

Layer 2 (native) is ~100x faster than Layer 1 (MCP). You get both.

### 3. Semantic Indexing

Goose has basic repo context. Hone has adaptive semantic indexing:

```bash
hone "Find functions that handle authentication"
# Hone searches by meaning (not keywords)
# Result: Returns auth() in auth.rs, verify_token() in middleware.rs, etc.
```

Powered by:
- Tree-sitter AST parsing (language-aware)
- SQLite vector embeddings (semantic search)
- Persistent index (cached, reused across sessions)

### 4. LSP Integration

Goose has no built-in LSP. Hone bridges to language servers:

```bash
hone "What's the type of this variable?"
# Agent calls: lsp_hover(file="main.rs", line=10, col=5)
# Returns: type information from language server

hone "Show me all errors in this file"
# Agent calls: lsp_diagnostics(file="main.rs")
```

Supports Python, Rust, TypeScript, Go, Java, etc.

### 5. Remote Access & Team Governance

Goose is single-user. Hone can be multi-user:

```bash
# Start daemon on a shared machine (a token is required unless it binds to loopback)
export HONE_AUTH_TOKEN="$(openssl rand -hex 32)"
honed 7070

# From your laptop: the desktop app (daemon URL and token) or the HTTP API.
# The hone CLI doesn't connect to a daemon.

# In HONE.md or config, define roles
[team.roles]
alice = "admin"      # manage settings, users
bob = "operator"     # run sessions, read code
charlie = "viewer"   # read-only access
```

There is no `--audit-log` option. To keep a tamper-evident log of every run, point `HONE_PROVENANCE_DIR` at an existing directory:
```bash
mkdir -p /var/log/hone/provenance
export HONE_AUTH_TOKEN="$(openssl rand -hex 32)"
HONE_PROVENANCE_DIR=/var/log/hone/provenance honed 7070
# One hash-chained JSONL file per POST /run: the prompt, tool calls and results
```

## What's Different

### 1. Extensions vs. Layers

**Goose:** All custom code is an "extension" (plugin).

**Hone:** Layers formalize tool categories:
- **Layer 2** — Native tools (implement in Rust, add to `hone-tools` crate)
- **Layer 1** — MCP servers (implement in any language, subprocess)
- **Layer 0** — Code Mode (agent generates TypeScript/Python, sandboxed execution)
- **Layer 3** — Skills (markdown conventions, zero cost)

For most use cases, use Layer 1 (MCP). It's the same as Goose extensions.

### 2. Recipes Are Identical (Mostly)

Goose recipes work in Hone with minimal changes:

**Goose recipe:**
```yaml
name: lint-project
steps:
  - provider: anthropic:claude-opus
  - extension: eslint
    call: lint_files
    with:
      path: "src/"
  - extension: github_client
    call: comment_on_pr
    with:
      issue: "{pr_number}"
      body: "Linting issues: {output}"
```

**Hone recipe:**
```yaml
name: lint-project
steps:
  - tool: file_search
    pattern: "src/**/*.js"
  - agent:
      prompt: "Lint these files using eslint"
      model: anthropic:claude-opus-4-5
  - tool: github_create_comment
    issue: "{pr_number}"
    body: "Linting issues: {output}"
```

The structure is the same; tool names may differ (Layer 1 MCP tools have different naming).

### 3. ACP (Formalized Custom Agents)

Goose has CLI providers (spawn a subprocess). Hone formalizes this as ACP:

**Goose CLI provider:**
```bash
goose --provider cli:/path/to/my-agent --model custom
```

**Hone ACP provider:** ACP providers are library-only for now. `hone` can't select one: there is no `acp:` model prefix, and a `[providers.acp]` table makes `config.toml` fail to load. See [ACP](../tutorials/acp.md).

ACP is the Agent Client Protocol (same idea, standardized).

### 4. Config Format

**Goose:** YAML or JSON config files

**Hone:** TOML config + HONE.md conventions

```toml
# ~/.config/hone/config.toml
[agent]
default_model = "anthropic:claude-opus-4-5"
max_iterations = 20

[memory]
enabled = true

[providers.anthropic]
api_key = "${ANTHROPIC_API_KEY}"

[[mcp_servers]]
name = "github"
command = "npx"
args = ["@modelcontextprotocol/server-github"]
```

```markdown
# HONE.md (in your project)
## Skill: Testing

Always run pytest after making changes.

## Skill: Code Review

Check for unwrap() calls without justification.
```

## Step-by-Step Migration

### 1. Install Hone

```bash
git clone https://github.com/anthropics/hone.git
cd hone
cargo build --release
sudo cp target/release/hone /usr/local/bin/
```

### 2. Copy Your Goose Config to Hone Config

**Goose config location:** `.goose/config.yaml` or `~/.goose/config.json`

**Hone config location:** `~/.config/hone/config.toml`

Example migration (Goose YAML → Hone TOML):

**Goose:**
```yaml
providers:
  default: anthropic
  anthropic:
    api_key: sk-ant-...
    model: claude-opus-4-5
mcp_servers:
  - name: github
    command: npx
    args:
      - "@modelcontextprotocol/server-github"
    env:
      GITHUB_TOKEN: ghp_...
```

**Hone:**
```toml
[providers.anthropic]
api_key = "sk-ant-..."

[agent]
default_model = "anthropic:claude-opus-4-5"

[[mcp_servers]]
name = "github"
command = "npx"
args = ["@modelcontextprotocol/server-github"]
env = { GITHUB_TOKEN = "ghp_..." }
```

### 3. Migrate Your Recipes

Copy Goose recipes to Hone:

```bash
cp .goose/recipes/* ~/.config/hone/recipes/
```

Update tool names if needed:
- Goose extensions → Hone MCP tools (names may differ)
- Goose providers → Hone providers (same names)

### 4. Create HONE.md for Project Conventions

If you had Goose system prompts or guidelines:

```markdown
# Project Skills

## Skill: Testing

Always run pytest after changes. Example:
  pytest -v tests/

## Skill: Commit Messages

Use conventional commits:
  fix: resolve race condition in cache
  feat: add async batch processing
```

### 5. Migrate Custom Extensions (Optional)

If you built Goose extensions in Python or Node:

**Option A: Use MCP** (recommended)
- Wrap your extension as an MCP server (same as Goose)
- Register in config.toml (same format as Goose)

**Option B: Contribute to Hone** (advanced)
- Implement in Rust as a Layer 2 native tool
- Add to hone-tools crate
- Submit a PR (see Contributing guide)

### 6. Test Your First Session

```bash
cd /your/goose/project
hone
```

Hone should work with your recipes and MCP servers immediately.

## FAQ

### Can I use my Goose recipes in Hone?

Yes, mostly. Recipe syntax is similar. Tool names may differ (check Layer 1 MCP tool names). Copy recipes over and test.

### Do my Goose MCP servers work in Hone?

Yes, 100%. Hone is MCP-compatible. Register them in config.toml exactly as you did in Goose.

### What about my Goose extensions?

If they're MCP servers, they work as-is. If they're language-specific plugins, you can either:
1. Wrap them as MCP servers
2. Contribute them to Hone as Layer 2 tools (Rust)

### Can I run both Goose and Hone?

Yes. They're independent. Keep Goose until you're fully comfortable with Hone.

### How do I debug MCP servers in Hone?

Same as Goose. Enable logging in config:

```toml
[logging]
level = "debug"
mcp_trace_file = "/tmp/hone-mcp.log"
```

### Is Hone backward-compatible with Goose recipes?

Mostly. Recipe syntax is similar; tool names may differ. Test your recipes and adjust as needed.

## Next Steps

1. **Install:** Follow step 1 above
2. **Config:** Migrate your Goose config to HONE.toml
3. **Recipes:** Copy your recipes and test
4. **Test:** Run `hone --recipe your-recipe.yaml`
5. **Extend:** Read [Tools and MCP](../tutorials/tools-and-mcp.md) for advanced integrations
6. **Team:** Read [Server and Remote](../tutorials/server-and-remote.md) for remote daemon setup

---

Questions about Goose → Hone migration? Open an issue on GitHub.
