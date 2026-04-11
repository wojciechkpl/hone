# MCP Extensions Guide

Hone supports 70+ community MCP (Model Context Protocol) extensions, allowing you to extend your agent's capabilities beyond native tools. This guide covers the most popular and useful extensions with copy-paste-ready configurations.

## Quick Start: Set Up Your First MCP Extension

### 1. Create a `.hone/mcp.json` file in your project

```json
{
  "mcpServers": {
    "filesystem": {
      "command": "npx",
      "args": ["-y", "@modelcontextprotocol/server-filesystem", "/workspace"]
    }
  }
}
```

### 2. Set environment variables (if needed)

Most extensions require API keys. Pass them via your shell:

```bash
export GITHUB_TOKEN="ghp_..."
export DATABASE_URL="postgresql://..."
hone
```

Hone will make these available to MCP servers automatically.

### 3. Start Hone and use the extension

```bash
hone
# Your agent now has access to filesystem operations beyond the working directory
```

That's it! The agent can now call the MCP server's resources and tools.

## How Hone Loads MCP Configs

Hone looks for MCP servers in two places (in order):

1. **Global config**: `~/.config/hone/mcp.json` — applies to all projects
2. **Project config**: `.hone/mcp.json` — applies to this project only

When server names collide, project settings win. Files that don't exist or are malformed are silently skipped.

## Top 10 MCP Extensions

### 1. Filesystem

**Package**: `@modelcontextprotocol/server-filesystem`

**Description**: Read and write files outside the working directory, manage directory structures, and perform file operations beyond Hone's native sandboxed I/O.

**What it enables**:
- Read files outside the current project
- Write to system directories (e.g., `/tmp`, home directory)
- Create symlinks and manage permissions
- Recursively walk directory trees
- Operations on large file sets

**Installation**:
```bash
# No installation needed — npx fetches it automatically
```

**Configuration**:
```json
{
  "mcpServers": {
    "filesystem": {
      "command": "npx",
      "args": ["-y", "@modelcontextprotocol/server-filesystem", "/workspace"],
      "timeout_ms": 10000
    }
  }
}
```

The argument `/workspace` is the root directory Hone's agent can access. Change it to restrict or expand access.

**Example usage**:

```
User: "Check for config files in my home directory and list them"

Agent calls filesystem MCP:
- list_dir(path="~/.config")
- read_file(path="~/.config/app.conf")
- Returns: file contents and directory structure
```

---

### 2. GitHub

**Package**: `@modelcontextprotocol/server-github`

**Description**: Create and manage GitHub issues, pull requests, branches, and inspect repository contents without `git clone`.

**What it enables**:
- Read repo structure and file contents
- Create/update issues and PRs
- List and manage branches
- Get commit history
- Search repositories
- Manage labels and milestones

**Installation**:
```bash
# No installation — uses npx
```

**Configuration**:
```json
{
  "mcpServers": {
    "github": {
      "command": "npx",
      "args": ["-y", "@modelcontextprotocol/server-github"],
      "env": {
        "GITHUB_PERSONAL_ACCESS_TOKEN": "${GITHUB_TOKEN}"
      },
      "timeout_ms": 15000
    }
  }
}
```

**Environment setup**:
```bash
# Create a Personal Access Token at https://github.com/settings/tokens
# Minimum scopes: repo, read:org
export GITHUB_TOKEN="ghp_..."
hone
```

**Example usage**:

```
User: "Create a GitHub issue in my-org/my-repo about fixing the auth bug"

Agent calls GitHub MCP:
- create_issue(
    owner="my-org",
    repo="my-repo",
    title="Fix authentication timeout issue",
    body="When idle for >30min, auth token expires...",
    labels=["bug", "auth"]
  )
- Returns: issue URL and number

Later:

User: "Show me open PRs in that repo"

Agent calls:
- list_pull_requests(owner="my-org", repo="my-repo", state="open")
- Returns: titles, authors, review status
```

---

### 3. PostgreSQL

**Package**: `@modelcontextprotocol/server-postgres`

**Description**: Query databases, inspect schemas, run migrations, and manage PostgreSQL instances.

**What it enables**:
- Execute queries (SELECT, INSERT, UPDATE, DELETE)
- Inspect table schemas and indexes
- Run migrations or raw SQL
- Get query execution plans
- Manage transactions

**Installation**:
```bash
# No installation — npx handles it
```

**Configuration**:
```json
{
  "mcpServers": {
    "postgres": {
      "command": "npx",
      "args": ["-y", "@modelcontextprotocol/server-postgres"],
      "env": {
        "DATABASE_URL": "${DATABASE_URL}"
      },
      "timeout_ms": 30000
    }
  }
}
```

**Environment setup**:
```bash
# PostgreSQL connection string (use a read-only user for safety)
export DATABASE_URL="postgresql://hone_user:password@localhost:5432/mydb"
hone
```

**Example usage**:

```
User: "Show me all tables in the current database with their column counts"

Agent calls Postgres MCP:
- query(sql="SELECT table_name FROM information_schema.tables WHERE table_schema='public'")
- For each table: query(sql="SELECT COUNT(*) as columns FROM information_schema.columns WHERE table_name=$1")
- Returns: formatted table list with schemas

User: "Find slow queries (>100ms) from the last hour"

Agent calls:
- query(sql="SELECT query, calls, total_time FROM pg_stat_statements WHERE total_time > 100 AND query != '<insufficient privilege>'")
- Analyzes patterns and suggests indexes
```

---

### 4. Brave Search

**Package**: `@modelcontextprotocol/server-brave-search`

**Description**: Perform web searches and retrieve real-time information from the internet.

**What it enables**:
- Full-text web search
- News search
- Image search
- Real-time API data
- Bypass information cutoff

**Installation**:
```bash
# No installation — npx handles it
```

**Configuration**:
```json
{
  "mcpServers": {
    "brave-search": {
      "command": "npx",
      "args": ["-y", "@modelcontextprotocol/server-brave-search"],
      "env": {
        "BRAVE_API_KEY": "${BRAVE_API_KEY}"
      },
      "timeout_ms": 10000
    }
  }
}
```

**Environment setup**:
```bash
# Get a free key at https://api.search.brave.com
export BRAVE_API_KEY="..."
hone
```

**Example usage**:

```
User: "What's the latest React version and its key features?"

Agent calls Brave Search MCP:
- search(query="React latest version 2025")
- Returns: top 10 results with titles, snippets, URLs

Agent synthesizes: "React 19.1 includes Server Components, Actions, and..."

User: "Find recent articles about AI safety"

Agent calls:
- search(query="AI safety 2025", count=10)
- Returns: latest news with publication dates
```

---

### 5. Memory

**Package**: `@modelcontextprotocol/server-memory`

**Description**: Persistent key-value memory across sessions, enabling agents to remember context and decisions between runs.

**What it enables**:
- Store and retrieve arbitrary data
- Maintain project state across sessions
- Track decisions and rationale
- Cross-session learning
- Session metadata

**Installation**:
```bash
# No installation — npx handles it
```

**Configuration**:
```json
{
  "mcpServers": {
    "memory": {
      "command": "npx",
      "args": ["-y", "@modelcontextprotocol/server-memory"],
      "timeout_ms": 5000
    }
  }
}
```

**Example usage**:

```
Session 1:

User: "We're using PostgreSQL 14. Remember this."

Agent calls Memory MCP:
- write_memory(key="db_version", value="PostgreSQL 14")
- write_memory(key="db_connection_pool", value="pgbouncer")

---

Session 2 (next day):

User: "Optimize the database queries"

Agent calls Memory MCP:
- read_memory(key="db_version")
- Returns: "PostgreSQL 14"

Agent: "I see you're on PostgreSQL 14 with pgbouncer. Let me optimize..."
```

---

### 6. Fetch

**Package**: `@modelcontextprotocol/server-fetch`

**Description**: Make HTTP requests, call APIs, and fetch web content for processing.

**What it enables**:
- HTTP GET/POST/PUT/DELETE requests
- Web scraping (fetch HTML)
- API integration (REST, GraphQL)
- Download files
- Header and authentication management

**Installation**:
```bash
# No installation — npx handles it
```

**Configuration**:
```json
{
  "mcpServers": {
    "fetch": {
      "command": "npx",
      "args": ["-y", "@modelcontextprotocol/server-fetch"],
      "timeout_ms": 30000
    }
  }
}
```

**Example usage**:

```
User: "Fetch the pricing page from our API docs and extract the tier information"

Agent calls Fetch MCP:
- fetch(url="https://api.example.com/docs/pricing")
- Returns: HTML content

Agent parses and extracts:
- Tier names, prices, features

User: "Check the health of all our services"

Agent calls:
- fetch(url="https://service1.example.com/health", method="GET")
- fetch(url="https://service2.example.com/health", method="GET")
- Returns: status codes, response times
- Agent reports: "All services healthy, avg response 42ms"
```

---

### 7. SQLite

**Package**: `@modelcontextprotocol/server-sqlite`

**Description**: Query and manage SQLite databases stored locally or on disk.

**What it enables**:
- Execute SQL queries on SQLite files
- Inspect schemas
- Create/modify tables
- Run migrations
- Analyze large datasets

**Installation**:
```bash
# No installation — npx handles it
```

**Configuration**:
```json
{
  "mcpServers": {
    "sqlite": {
      "command": "npx",
      "args": ["-y", "@modelcontextprotocol/server-sqlite", "/path/to/data.db"],
      "timeout_ms": 15000
    }
  }
}
```

**Example usage**:

```
User: "How many users signed up last week? Show me by country."

Agent calls SQLite MCP:
- query(sql="SELECT country, COUNT(*) as signups FROM users WHERE created_at >= datetime('now', '-7 days') GROUP BY country ORDER BY signups DESC")
- Returns: country-wise signup counts

User: "Create a backup table of inactive accounts"

Agent calls:
- query(sql="CREATE TABLE inactive_backup AS SELECT * FROM users WHERE last_login < datetime('now', '-1 year')")
- Returns: success message with row count
```

---

### 8. Puppeteer

**Package**: `@modelcontextprotocol/server-puppeteer`

**Description**: Browser automation for testing, screenshots, interaction, and dynamic content rendering.

**What it enables**:
- Take screenshots of websites
- Interact with dynamic content (click, type, scroll)
- Test web flows end-to-end
- Extract rendered HTML (not just static)
- PDF generation
- Performance measurement

**Installation**:
```bash
# Requires Node.js installed globally
npm install -g puppeteer
```

**Configuration**:
```json
{
  "mcpServers": {
    "puppeteer": {
      "command": "npx",
      "args": ["-y", "@modelcontextprotocol/server-puppeteer"],
      "timeout_ms": 60000
    }
  }
}
```

**Example usage**:

```
User: "Take a screenshot of the login page at https://app.example.com"

Agent calls Puppeteer MCP:
- screenshot(url="https://app.example.com/login")
- Returns: PNG image base64-encoded

Agent: "The login form is visible with email and password fields"

User: "Test the forgot-password flow"

Agent calls:
- goto(url="https://app.example.com/login")
- click(selector="a[href='/forgot-password']")
- screenshot()
- fill(selector="input[type='email']", text="test@example.com")
- click(selector="button:contains('Reset')")
- waitForNavigation()
- Returns: success message with screenshot showing reset email sent
```

---

### 9. Slack

**Package**: `@modelcontextprotocol/server-slack`

**Description**: Send messages to Slack, read channels, manage threads, and integrate agent actions with Slack workflows.

**What it enables**:
- Send direct messages to users
- Post to channels
- Read channel history
- Manage threads and reactions
- List channels and users
- Upload files

**Installation**:
```bash
# No installation — npx handles it
```

**Configuration**:
```json
{
  "mcpServers": {
    "slack": {
      "command": "npx",
      "args": ["-y", "@modelcontextprotocol/server-slack"],
      "env": {
        "SLACK_BOT_TOKEN": "${SLACK_BOT_TOKEN}",
        "SLACK_APP_TOKEN": "${SLACK_APP_TOKEN}"
      },
      "timeout_ms": 10000
    }
  }
}
```

**Environment setup**:
```bash
# Create a Slack app at https://api.slack.com/apps
# Generate tokens with scopes: chat:write, channels:read, users:read, files:write
export SLACK_BOT_TOKEN="xoxb-..."
export SLACK_APP_TOKEN="xapp-..."
hone
```

**Example usage**:

```
User: "Notify the dev team on Slack that the deployment is ready"

Agent calls Slack MCP:
- send_message(channel="#dev-team", text="Deployment ready for v2.1.0. Rollout starts in 10 minutes.")
- Returns: message timestamp

User: "Read the last 10 messages in #incidents"

Agent calls:
- read_channel(channel="#incidents", limit=10)
- Returns: messages with authors, timestamps

Agent summarizes: "3 open incidents, highest priority: database replication lag"
```

---

### 10. Sequential Thinking

**Package**: `@modelcontextprotocol/server-sequential-thinking`

**Description**: Structured reasoning and multi-step planning for complex problems.

**What it enables**:
- Break complex queries into steps
- Explicit reasoning chains
- Verification and backtracking
- Better error handling
- Auditable decision-making

**Installation**:
```bash
# No installation — npx handles it
```

**Configuration**:
```json
{
  "mcpServers": {
    "thinking": {
      "command": "npx",
      "args": ["-y", "@modelcontextprotocol/server-sequential-thinking"],
      "timeout_ms": 30000
    }
  }
}
```

**Example usage**:

```
User: "Design a database schema for a multi-tenant SaaS app"

Agent calls Sequential Thinking MCP:

thinking_start()

  Step 1: Identify core entities (users, teams, workspaces, data records)
  Step 2: Define relationships (users <-> teams, teams <-> workspaces)
  Step 3: Consider isolation (row-level security for tenants)
  Step 4: Plan for scaling (sharding, archiving strategies)
  Step 5: Design migrations (versioned schema changes)

thinking_end()

Agent outputs: Complete SQL schema with rationale for each design decision

User: "Audit this code for security issues"

Agent uses thinking to:
  1. Identify input vectors (URL params, form data, headers, cookies)
  2. Check for validation (length limits, type checks, encoding)
  3. Check for escaping (SQL injection, XSS, command injection)
  4. Check for auth/authz (permissions, token validation)
  5. Report findings with severity and fixes

Returns: actionable security report with prioritized fixes
```

---

## Finding More Extensions

The MCP ecosystem is rapidly growing. Here's where to discover new extensions:

### Official Registry

- **GitHub**: https://github.com/modelcontextprotocol/servers
  - Monitored list of official and community extensions
  - Installation instructions and examples
  - Feature documentation

### Community Collections

- **MCP.run**: https://mcp.run (community registry with search)
- **Awesome MCP**: https://github.com/pglombardo/awesome-mcp
  - Curated list of extensions
  - Categorized by use case

### Installing Custom MCP Extensions

Most extensions follow the same pattern:

```json
{
  "mcpServers": {
    "my-extension": {
      "command": "npx",
      "args": ["-y", "@scope/server-name", "arg1", "arg2"],
      "env": {
        "API_KEY": "${MY_API_KEY}"
      },
      "timeout_ms": 10000
    }
  }
}
```

For local or custom extensions:

```json
{
  "mcpServers": {
    "custom-tool": {
      "command": "python",
      "args": ["/path/to/server.py"],
      "env": {
        "CONFIG": "/path/to/config.json"
      }
    }
  }
}
```

---

## Configuration Reference

Every MCP server in `.hone/mcp.json` has this structure:

```json
{
  "mcpServers": {
    "server-name": {
      "command": "string (required)",
      "args": ["array", "of", "arguments"],
      "env": {
        "ENV_VAR": "value or ${REFERENCE}",
        "API_KEY": "${MY_API_KEY}"
      },
      "disabled": false,
      "timeout_ms": 10000
    }
  }
}
```

**Fields**:

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `command` | string | (required) | Executable to run (e.g., `npx`, `python`, absolute path) |
| `args` | array | `[]` | Arguments passed to command |
| `env` | object | `{}` | Environment variables. Values can reference `${VAR}` to use shell variables |
| `disabled` | boolean | `false` | Skip this server at startup |
| `timeout_ms` | number | `10000` | Per-request timeout in milliseconds |

**Environment variables**:
- `${VAR}` syntax is expanded from your shell environment
- Only variables set before `hone` starts are available
- Use a `.env` file or shell profile to set them persistently

---

## Security Best Practices

### Sandboxing

MCP servers run in sandboxed subprocesses. Each server:
- Runs with isolated file access (via `args` paths)
- Cannot access Hone's memory or other servers
- Is terminated if it hangs (via `timeout_ms`)

### API Keys

**Do NOT hardcode secrets in `.hone/mcp.json`**. Always use environment variables:

```bash
# Good: uses shell environment variable
export GITHUB_TOKEN="ghp_..."
hone

# Config references it safely:
"env": { "GITHUB_PERSONAL_ACCESS_TOKEN": "${GITHUB_TOKEN}" }
```

**Do NOT commit `.hone/mcp.json` with real keys**:

```bash
# Add to .gitignore
echo ".hone/mcp.json" >> .gitignore

# Create a template for teammates
cat > .hone/mcp.json.example << 'EOF'
{
  "mcpServers": {
    "github": {
      "command": "npx",
      "args": ["-y", "@modelcontextprotocol/server-github"],
      "env": {
        "GITHUB_PERSONAL_ACCESS_TOKEN": "${GITHUB_TOKEN}"
      }
    }
  }
}
EOF
```

### Principle of Least Privilege

- Use dedicated API keys with minimal scopes for each service
- Revoke keys if a machine is compromised
- Rotate keys periodically
- Use read-only credentials when possible

**Example for GitHub**:

```bash
# Create a Personal Access Token with only these scopes:
# - repo (for public repos only, uncheck "Full control")
# - read:org
# - NOT: delete_repo, admin:repo_hook, workflow

export GITHUB_TOKEN="ghp_..."
```

---

## Troubleshooting

### Common Issues

#### `npx: command not found`

**Problem**: Node.js/npx not installed.

**Solution**:
```bash
# Install Node.js from https://nodejs.org
# Or via package manager:
brew install node          # macOS
sudo apt install nodejs    # Ubuntu/Debian
choco install nodejs       # Windows
```

Verify:
```bash
npx --version
```

#### Server crashes or returns errors

**Problem**: MCP server exited unexpectedly.

**Debugging**:
```bash
# Check server output directly
npx -y @modelcontextprotocol/server-filesystem /tmp

# Verify environment variables are set
echo $GITHUB_TOKEN

# Check server logs in Hone (if available)
```

**Common causes**:
- Missing environment variables
- Invalid arguments
- Insufficient permissions (e.g., database user lacks query access)

#### Timeout exceeded

**Problem**: `timeout_ms` expired before server responded.

**Solution**:
```json
{
  "mcpServers": {
    "slow-server": {
      "command": "npx",
      "args": ["-y", "@modelcontextprotocol/server-something"],
      "timeout_ms": 30000
    }
  }
}
```

Increase `timeout_ms` for:
- Database queries on large tables
- Complex web scraping
- Slow network APIs

#### Server not found by Hone

**Problem**: Configuration is correct but agent can't access server.

**Solutions**:
1. Verify `.hone/mcp.json` is valid JSON:
   ```bash
   cat .hone/mcp.json | jq .
   ```

2. Check file location:
   - Project-level: `.hone/mcp.json` (in your project root)
   - Global: `~/.config/hone/mcp.json` (home directory)

3. Restart Hone after adding/changing config:
   ```bash
   hone
   ```

#### Agent can't find the tool

**Problem**: Server is running but agent doesn't know about its tools.

**Solution**: MCP servers expose tools that agents discover automatically. If discovery fails:
- Check server logs for errors
- Verify required environment variables are set
- Increase `timeout_ms` if server is slow to respond

---

## Advanced: Writing Custom MCP Extensions

For team-specific tools, you can write custom MCP servers:

```bash
# Create a Python MCP server
mkdir my-mcp-server
cd my-mcp-server
pip install mcp

# Implement your server.py with resources and tools
# See https://modelcontextprotocol.io/docs/server/sdks for SDK docs

# Use it in Hone
```

Configuration:
```json
{
  "mcpServers": {
    "custom": {
      "command": "python",
      "args": ["./my-mcp-server/server.py"],
      "env": {
        "CUSTOM_CONFIG": "/path/to/config.json"
      }
    }
  }
}
```

For details, see the [MCP specification](https://spec.modelcontextprotocol.io).

---

## Next Steps

1. **Start small**: Add one extension (e.g., Filesystem or GitHub) and test it
2. **Monitor usage**: Watch agent logs to see which tools are called
3. **Expand gradually**: Add more extensions as you discover use cases
4. **Share configs**: Create `.hone/mcp.json.example` templates for your team
5. **Optimize timeouts**: Adjust `timeout_ms` based on real performance

For more on Hone capabilities, see:
- [Tools and MCP Integration](tools-and-mcp.md)
- [Configuration Guide](../configuration.md)
- [Security Best Practices](security.md)
