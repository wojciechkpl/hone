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
context_window_tokens = 200000

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

# Total context window size in tokens (model-dependent)
context_window_tokens = 200000

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

Set a budget limit:

```bash
hone --budget 10.00  # max $10 per session
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

## CLI Flags

Most config options can be overridden via CLI flags:

```bash
hone --model openai:gpt-4o \
     --budget 10.00 \
     --context-limit 32000 \
     --temperature 0.5 \
     --max-iterations 20 \
     --sandbox-enforce
```

See `hone --help` for all flags.

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
context_window_tokens = 4096

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
