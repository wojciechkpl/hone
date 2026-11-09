# Configuration Reference

Hone uses a three-level configuration hierarchy:

1. **CLI flags** (highest priority)
2. **TOML file** (`~/.config/hone/config.toml`)
3. **Environment variables**
4. **Defaults** (lowest priority)

This guide covers all configurable options.

## Config File Location

```bash
~/.config/hone/config.toml
```

Create the file if it doesn't exist:

```bash
mkdir -p ~/.config/hone
cat > ~/.config/hone/config.toml << 'EOF'
[agent]
default_model = "anthropic:claude-opus-4-5"

[memory]
enabled = true
encrypted = true
EOF
```

## Agent Configuration

```toml
[agent]
# Default model to use when no --model flag is provided
default_model = "anthropic:claude-opus-4-5"

# Tokens reserved for model output
reserved_output_tokens = 4000

# Tokens reserved for system prompt
reserved_system_prompt_tokens = 2000

# Safety margin (prevent hitting exact limit)
context_safety_margin = 0.1  # 10%

# Maximum iterations per session
max_iterations = 50

# Default temperature for non-ensemble turns
temperature = 0.3

# Maximum number of tool calls per turn
max_tool_calls_per_turn = 10

# Timeout for tool execution (seconds)
tool_timeout_secs = 30
```

## Memory Configuration

```toml
[memory]
# Enable memory persistence across sessions
enabled = true

# SQLite database path
db_path = "~/.local/share/hone/memory.db"

# Enable encryption for sensitive data
encrypted = false  # change to true to enable

# Encryption cipher (if encrypted = true)
cipher = "aes256-gcm"

# Memory decay: forget items after N days
decay_days = 90

# Maximum memory items to retain
max_items = 10000
```

### Memory Encryption

To enable encryption:

```bash
cargo build --features encrypted-memory
hone --init-encryption
# Enter passphrase: ****
```

This stores the encryption key in your system keychain:
- **macOS:** Keychain Services
- **Linux:** Secret Service or pass
- **Windows:** DPAPI (future)

Configuration:

```toml
[memory]
enabled = true
encrypted = true
cipher = "aes256-gcm"
keychain_service = "hone"
```

## Provider Configuration

### Anthropic

```toml
[providers.anthropic]
api_key = "${ANTHROPIC_API_KEY}"  # or hardcoded
base_url = "https://api.anthropic.com"
timeout_secs = 30
```

### OpenAI (and Compatible Endpoints)

```toml
[providers.openai]
api_key = "${OPENAI_API_KEY}"
base_url = "https://api.openai.com/v1"
timeout_secs = 30
```

**For custom OpenAI-compatible endpoints:**

```bash
export OPENAI_API_KEY="your-api-key"
export HONE_OPENAI_BASE_URL="https://api.deepseek.com"
export HONE_OPENAI_MODEL="deepseek-coder"
hone
```

Or in config:

```toml
[providers.openai]
api_key = "${OPENAI_API_KEY}"
base_url = "https://api.deepseek.com"
model_override = "deepseek-coder"
```

**Supported endpoints:**
- DeepSeek: `https://api.deepseek.com`
- Qwen: `https://dashscope.aliyuncs.com/compatible-mode/v1`
- OpenRouter: `https://openrouter.ai/api/v1`
- vLLM: `http://localhost:8000/v1`

### Ollama

```toml
[providers.ollama]
base_url = "http://localhost:11434"
timeout_secs = 300  # local models may be slow
```

### Claude CLI

```toml
[providers.claude_cli]
claude_path = "/usr/local/bin/claude"  # auto-detected if not specified
```

### Gemini CLI

```toml
[providers.gemini_cli]
gemini_path = "/usr/local/bin/gemini"  # auto-detected if not specified
```

## Pricing and Cost Tracking

```toml
[pricing.anthropic]
"claude-opus-4-5" = { input_per_mtok = 3.00, output_per_mtok = 15.00 }
"claude-sonnet-4" = { input_per_mtok = 3.00, output_per_mtok = 15.00 }
"claude-haiku-3-5" = { input_per_mtok = 0.80, output_per_mtok = 4.00 }

[pricing.openai]
"gpt-4o" = { input_per_mtok = 2.50, output_per_mtok = 10.00 }
"gpt-4-turbo" = { input_per_mtok = 10.00, output_per_mtok = 30.00 }
"gpt-3.5-turbo" = { input_per_mtok = 0.50, output_per_mtok = 1.50 }

[pricing.ollama]
# Local models are free
"llama2" = { input_per_mtok = 0.00, output_per_mtok = 0.00 }
```

## Sandbox Configuration

```toml
[sandbox]
# Auto-detect: "seatbelt" (macOS), "landlock" (Linux), "seccomp" (fallback)
platform = "auto"

# Enforce sandbox policy (fail-closed)
enforce = true

# Default policy for all tools
[sandbox.defaults]
allow_network = false          # deny network by default
allow_read = ["{project_root}"]
allow_write = ["/tmp"]
allow_env = ["PATH", "HOME", "USER"]

# Per-tool policy overrides
[sandbox.tools.shell]
allow_read = ["{project_root}", "/usr/lib"]
allow_write = ["/tmp", "{project_root}/target"]
allow_exec = ["/bin", "/usr/bin", "/usr/local/bin"]
allow_network = false
allow_env = ["PATH", "HOME", "RUST_BACKTRACE"]

[sandbox.tools.git]
allow_read = ["{project_root}"]
allow_write = ["{project_root}"]
allow_exec = ["/usr/bin/git"]
allow_network = false
allow_env = ["GIT_AUTHOR_NAME", "GIT_AUTHOR_EMAIL"]

# Process resource limits
[sandbox.limits]
cpu_seconds = 30           # max CPU time per tool call
memory_mb = 512            # max RAM per tool call
disk_mb = 1024             # max disk writes per turn
open_files = 1024          # max open file descriptors
subprocess_count = 10      # max subprocesses per tool call
```

**Variables:**
- `{project_root}` — current working directory
- `{home}` — user's home directory

## Server Configuration

```toml
[server]
# HTTP/2 server listen address
host = "0.0.0.0"
port = 7070

# TLS certificate and key (optional)
tls_cert = "/etc/hone/cert.pem"
tls_key = "/etc/hone/key.pem"

# Max concurrent connections
max_connections = 1000

# Request timeout
request_timeout_secs = 300

# Health check interval (seconds)
health_check_interval_secs = 60
```

### Authentication

```toml
[server.auth]
enabled = true

# Bearer token (basic security)
[[server.auth.tokens]]
token = "sk-hone-abc123def456..."
user = "alice@example.com"

# OAuth2 (future)
# oauth2_provider = "https://auth.example.com"
```

Usage:

```bash
# Start daemon
honed --config ~/.config/hone/config.toml

# Connect with token
curl -H "Authorization: Bearer sk-hone-abc123def456..." \
  http://localhost:7070/sessions
```

## Team Configuration (RBAC)

```toml
[rbac]
enabled = true

# Role definitions
[[rbac.roles]]
name = "admin"
permissions = ["sessions:create", "sessions:delete", "tools:all", "mcp:all"]

[[rbac.roles]]
name = "developer"
permissions = ["sessions:create", "sessions:read", "tools:file", "tools:git", "tools:shell"]

[[rbac.roles]]
name = "viewer"
permissions = ["sessions:read", "mcp:read_only"]

# User assignments
[[rbac.users]]
user_id = "alice@example.com"
role = "admin"

[[rbac.users]]
user_id = "bob@example.com"
role = "developer"

[[rbac.users]]
user_id = "carol@example.com"
role = "viewer"
```

## Audit Logging

```toml
[audit]
enabled = true
log_file = "/var/log/hone/audit.log"
log_level = "info"  # info, warn, error
log_format = "json"
```

Audit logs include:
- Tool calls (input, output, duration)
- File access attempts (allowed and denied)
- Security events (sandbox violations, auth failures)
- Cost tracking (tokens, USD)

Query logs:

```bash
# All events for a session
grep "session_id=sess_abc123" /var/log/hone/audit.log | jq .

# Access denials
grep '"event":"access_denied"' /var/log/hone/audit.log | jq .

# Cost tracking
grep '"event":"cost_incurred"' /var/log/hone/audit.log | jq .
```

## Semantic Index (PostgreSQL)

```toml
[index]
enabled = true
database_url = "postgres://user:pass@localhost/hone_index"

# Embedding model (default: sentence-transformers/all-MiniLM-L6-v2)
embedding_model = "sentence-transformers/all-MiniLM-L6-v2"
embedding_dimensions = 384

# Index type: "hnsw" or "ivfflat"
index_type = "hnsw"

# HNSW parameters
hnsw_m = 16              # neighbor list size
hnsw_ef_construction = 64  # construction effort

# Batch size for indexing
batch_size = 100

# File watcher for incremental updates
watch_enabled = true
watch_ignore_patterns = [".git", "target", "node_modules"]
```

If PostgreSQL is unavailable, the semantic index falls back to in-memory substring matching.

## MCP Servers

```toml
[[mcp_servers]]
name = "github"
command = "npx"
args = ["@modelcontextprotocol/server-github"]
hash = "sha256:abc123def456..."  # integrity check
timeout_secs = 30
```

## Fallback Chains

```toml
[[fallback_chains]]
name = "production"
providers = ["anthropic:claude-opus-4-5", "openai:gpt-4o", "ollama:llama2"]
retry_on = ["RateLimit", "Timeout", "ServiceUnavailable"]
retry_delay_secs = 5
max_retries = 3
```

Usage:

```bash
hone --fallback-chain production
```

Or set as default:

```toml
[agent]
default_fallback_chain = "production"
```

## Model Routing Rules

```toml
[[routing_rules]]
name = "quick_tasks"
condition = "input_length < 50"
provider = "anthropic:claude-haiku-3-5"

[[routing_rules]]
name = "complex_reasoning"
condition = "input_contains('analyze')"
provider = "anthropic:claude-opus-4-5"

[[routing_rules]]
name = "budget_conscious"
condition = "true"  # default
provider = "ollama:llama2"
```

## Theme Customization

Create `~/.config/hone/theme.toml` to override theme colors with 24-bit RGB hex values:

```toml
# Accent colors
accent-hot = "#FF1744"
accent-muted = "#FFB74D"

# Background colors
bg-normal = "#0d1117"
bg-secondary = "#1C1C24"

# Text colors
text-normal = "#E6E1E8"
text-muted = "#8B8B8B"

# Status colors
success = "#27AE60"
error = "#E74C3C"
warning = "#F39C12"
info = "#3498DB"
```

**All available color keys:**

| Key | Purpose |
|-----|---------|
| `accent-hot` | Active/focused elements (cursor, selection) |
| `accent-muted` | Secondary accent (disabled, dimmed) |
| `bg-normal` | Main background |
| `bg-secondary` | Secondary background (panels, sidebar) |
| `text-normal` | Primary text color |
| `text-muted` | Dimmed text (hints, timestamps) |
| `text-link` | Hyperlink text |
| `border-normal` | Normal borders |
| `border-muted` | Dimmed borders |
| `success` | Success states (green, checkmarks) |
| `error` | Error states (red) |
| `warning` | Warning states (yellow) |
| `info` | Information states (blue) |

Only list colors you want to override; all others use the theme defaults. Colors are merged with the selected theme, not replacing it entirely.

## Permissions

Permission rules decide which tool calls run, ask first, or are refused. With no
permission files, Hone behaves as before.

**Files**

- User: `~/.config/hone/permissions.toml`
- Project: `<working dir>/.hone/permissions.toml`. Only the working directory itself is
  checked: running from a subdirectory does not pick up a parent's `.hone/`.

**Rules.** Each rule has one matcher (`command`, `path` or `host`), a `decision`
(`allow`, `ask` or `deny`), an optional `id`, and for path rules an `op` filter
(`read`, `write` or `any`). Rules without an `id` get an automatic one such as
`user:2` or `project:1`. The full format is in
[`docs/plans/policy-v2-spec.md`](plans/policy-v2-spec.md).

```toml
version = 2

[[rules]]
id = "no-push"
command = "git push*"
decision = "deny"

[[rules]]
id = "ask-secrets"
path = "secrets/**"
op = "read"
decision = "ask"
```

**Precedence.** Built-in rules come first (none ship yet), then the user file, then the
project file. User rules replace the built-in verdict. Project rules can only tighten:
a project file cannot allow what the user file asks about or denies. A broken
permission file stops the run (exit code 2 for the CLI, HTTP 400 for honed).

**Asks.**

- Interactive runs (the TUI) pause and prompt. Approving "always" on a rule with a
  wildcard (any `*`, `?`, `[` or `{`, e.g. `*.example`, `{workdir}/**`, `git push*`)
  approves only that host, path or command, so approving one doesn't approve the others.
  Approving "always" on a rule without a wildcard remembers the whole rule for the
  session. A command rule without a wildcard matches the command **with any arguments**,
  so approving `git push` also covers `git push --force …`. Write a narrower rule if
  that matters.
- Headless runs (one-shot, `--json`, honed without a step channel) cannot prompt, so an
  `ask` is refused with `requires approval (rule <id>)`. A `deny` is always refused.

**`--approve <RULE_ID>`** pre-approves the `ask` decisions of a rule for one run. It is
repeatable and takes rule ids from your permission files (explicit or automatic), or
`builtin:unparsed-command` and `builtin:invalid-host`. An unknown id, or `--approve`
with no permission files, exits with code 2. It never turns a `deny` into an allow.

```bash
hone --approve ask-secrets "summarise the notes under secrets/"
```

**honed.** `POST /run` loads the policy for the run's working directory (the user file
comes from the daemon's `$HOME`) and accepts `"approve": ["rule-id", ...]` in the
request body. A broken policy file, an unknown id, or `approve` with no policy gives
400. A non-empty `approve` also needs an authenticated honed (an auth token set);
without one the request gets 400 `approve requires an authenticated server`.
Orchestration runs apply the policy but do not take `approve`. The distributed worker
loads the policy once at startup, from its own working directory, and exits with code 2
if a permission file is broken.

**`hone-mcp-server`** applies the policy to every `tools/call`: a `deny` or an `ask`
returns an error result and the tool is not called. There is no prompt, so use
`HONE_APPROVE=id1,id2` to pre-approve rules. An unknown id, or a broken policy file,
makes the server exit with code 2.

**Providers that run their own tools** (the `claude` CLI adapter, ACP agents and the
Gemini CLI adapter)
execute tools themselves, so the policy cannot check them. Under a non-empty policy Hone
refuses to use such a provider (exit code 2 / HTTP 400) unless
`HONE_ALLOW_UNGATED_PROVIDER=1` is set, in which case it prints a warning and continues.
With no policy or an empty one there is no refusal.

**Limits.** Command rules match the command text after parsing, which is a speed bump
rather than a security boundary: shell aliases, interpreters (`python -c ...`), paths
inside tool options and similar indirections are not seen. The sandbox is the boundary;
see [Sandbox Configuration](#sandbox-configuration).

## Environment Variables

| Variable | Purpose |
|----------|---------|
| `ANTHROPIC_API_KEY` | Anthropic API key |
| `OPENAI_API_KEY` | OpenAI API key (or custom endpoint token) |
| `HONE_OPENAI_BASE_URL` | Override OpenAI endpoint URL |
| `HONE_OPENAI_MODEL` | Override model name for custom endpoint |
| `RUST_LOG` | Logging level (`debug`, `info`, `warn`, `error`) |
| `HONE_CONFIG` | Override config file path |
| `DATABASE_URL` | PostgreSQL connection string (if not in config) |
| `OLLAMA_HOST` | Ollama server URL (default: `http://localhost:11434`) |
| `HONE_DISABLE_PERSISTENT_STORES` | Skip SQLite session/memory setup for this run (same effect as `--ephemeral`; set to `1` in CI, demos, or expect scripts) |
| `HONE_AUDIT_LOG` | Enable JSONL egress audit log (equivalent to `--audit`) |
| `HONE_CONTEXT_INJECTION` / `HONE_NO_INJECTION` | Force-enable or force-disable proactive context injection |
| `HONE_CONTEXT_WINDOW` | Override the model's context window in tokens (4096–10000000), e.g. `32768` for a model served with a 32k window. Applies to every agent and model in the process. Hone compacts conversation history at its per-model budget ratio of this value (Qwen: 45%); the system prompt and tool schemas come on top, so windows below about 16k may still overflow. Unset: the window is inferred from the model name. An invalid value is ignored and logged at warn level (visible with `RUST_LOG=warn`). |
| `HONE_ALLOW_UNGATED_PROVIDER` | `1` lets a provider that runs its own tools be used under a non-empty permission policy (see [Permissions](#permissions)) |
| `HONE_APPROVE` | Comma-separated permission rule ids pre-approved for `hone-mcp-server` |
| `HONE_SANDBOX_ENFORCE` | `1` turns on Landlock enforcement for shell commands on Linux (opt-in while it is validated; Seatbelt on macOS is always on) |
| `HONE_SANDBOX_EXTRA_READ` | Extra read-only roots for the shell sandbox, colon-separated |
| `HONE_REDUCED_MOTION` | Disable Agent Rail animation |
| `HONE_ASCII_ONLY` | Force ASCII box drawing and file-op badges |
| `HONE_NO_BELL` | Disable terminal bell on 10s+ turns |
| `NO_COLOR` | Disable all colorized output |

## CLI Flags

A few options can be set per run with CLI flags:

```bash
hone --model openai:gpt-4o --ephemeral --audit "fix the failing test"
```

Run `hone --help` for the full list. There are no flags for budget, temperature,
iteration limit, context window or sandbox mode. The context window is set with
`HONE_CONTEXT_WINDOW`.

## Example Configs

### Minimal (Quick Start)

```toml
[agent]
default_model = "anthropic:claude-opus-4-5"

[providers.anthropic]
api_key = "${ANTHROPIC_API_KEY}"
```

### Production (Server + Team)

```toml
[agent]
default_model = "anthropic:claude-opus-4-5"
max_iterations = 50

[memory]
enabled = true
encrypted = true

[providers.anthropic]
api_key = "${ANTHROPIC_API_KEY}"
timeout_secs = 30

[server]
host = "0.0.0.0"
port = 7070
max_connections = 1000

[server.auth]
enabled = true
[[server.auth.tokens]]
token = "sk-hone-prod-abc123..."
user = "ops@example.com"

[rbac]
enabled = true
[[rbac.roles]]
name = "admin"
permissions = ["sessions:create", "sessions:delete", "tools:all", "mcp:all"]

[audit]
enabled = true
log_file = "/var/log/hone/audit.log"

[sandbox]
enforce = true
platform = "landlock"
```

### Budget-Conscious (Ollama Local)

```toml
[agent]
default_model = "ollama:llama2"

[providers.ollama]
base_url = "http://localhost:11434"
timeout_secs = 300

[sandbox]
enforce = false  # local-only, less critical
```

## Troubleshooting

### Config file not found

```
error: config file not found at ~/.config/hone/config.toml
```

Create the file:

```bash
mkdir -p ~/.config/hone
touch ~/.config/hone/config.toml
```

### API key not recognized

```
error: ANTHROPIC_API_KEY not set or invalid
```

Check environment variable:

```bash
echo $ANTHROPIC_API_KEY
```

Or add to config:

```toml
[providers.anthropic]
api_key = "sk-ant-..."
```

### Sandbox policy violation

```
error: sandbox policy not met (path not in allowlist)
```

Add the path to the allowlist:

```toml
[sandbox.tools.file_reader]
allow_read = ["/new/path"]
```

### PostgreSQL connection failed

```
error: failed to connect to PostgreSQL at postgres://localhost/hone_index
```

Check PostgreSQL is running:

```bash
psql -U postgres -d hone_index -c "SELECT 1"
```

If unavailable, semantic index falls back to in-memory substring matching.

## See Also

- [Architecture: Context Optimization](architecture.md#10-context-optimization)
- [Security Guide](tutorials/security.md)
- [Provider Configuration](tutorials/providers.md)
