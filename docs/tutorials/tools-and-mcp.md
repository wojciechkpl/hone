# Tools and MCP Integration

Hone's tool system is layered: native Rust tools for speed, MCP for extensibility, and Code Mode for custom orchestration. This guide explains each layer and how to use them.

## Layer Architecture

```mermaid
flowchart TB
    Agent["hone-core: Agent"]
    
    subgraph L3["Layer 3: Skills (HONE.md)"]
        HoneMd["HONE.md<br/>Markdown conventions<br/>Parsed at startup<br/>Zero runtime cost"]
    end
    
    subgraph L2["Layer 2: Native Tools (Rust, <1ms)"]
        FileIO["File I/O<br/>read, write, edit, glob"]
        Shell["Shell<br/>execute, capture"]
        Git["Git<br/>commit, branch, diff, undo"]
        LSP["LSP Bridge<br/>hover, diagnostics, actions"]
        Index["Semantic Index<br/>search code by meaning"]
    end
    
    subgraph L1["Layer 1: MCP (JSON-RPC, 10-100ms)"]
        GitHub["GitHub MCP"]
        Jira["Jira MCP"]
        Slack["Slack MCP"]
        DB["Database MCP"]
        Custom["Custom MCP"]
    end
    
    subgraph L0["Layer 0: Code Mode (Orchestration)"]
        CodeMode["Agent-generated<br/>TypeScript/Python<br/>Calls native + MCP via SDK"]
    end
    
    Agent --> L2
    Agent --> L1
    Agent --> L0
    Agent --> L3
```

## Layer 2: Native Tools

Fastest layer. Direct Rust function calls. Built into Hone.

### Available Native Tools

#### File I/O

Read, write, edit files with glob pattern matching.

```bash
# List files matching a pattern
hone -i "Find all Python files in src/"
# Agent calls: glob(pattern="src/**/*.py")

# Read a file
hone -i "Show me main.rs"
# Agent calls: read_file(path="main.rs")

# Write a file
hone -i "Create a README with instructions"
# Agent calls: write_file(path="README.md", content="...")

# Edit a file (precise edits)
hone -i "Add error handling to line 42"
# Agent calls: edit_file(path="main.rs", line=42, replacement="...")
```

**Schema:**

```json
{
  "read_file": {
    "properties": {
      "path": { "type": "string" }
    }
  },
  "write_file": {
    "properties": {
      "path": { "type": "string" },
      "content": { "type": "string" }
    }
  },
  "edit_file": {
    "properties": {
      "path": { "type": "string" },
      "line": { "type": "integer" },
      "replacement": { "type": "string" }
    }
  },
  "glob": {
    "properties": {
      "pattern": { "type": "string" }
    }
  }
}
```

#### Shell Execution

Execute shell commands and capture output.

```bash
hone -i "Run the tests and show me any failures"
# Agent calls: shell(cmd="cargo test", cwd="/project")
```

**Schema:**

```json
{
  "shell": {
    "properties": {
      "cmd": { "type": "string", "description": "bash command" },
      "timeout_secs": { "type": "integer" }
    }
  }
}
```

#### Git Operations

Version control: commit, branch, diff, undo.

```bash
hone -i "Commit my changes with a message"
# Agent calls: git_commit(message="...", files=["main.rs"])

hone -i "Show me what changed"
# Agent calls: git_diff()

hone -i "Switch to feature branch"
# Agent calls: git_checkout(branch="feature/new-tool")
```

**Available git tools:**
- `git_status` — show changed files
- `git_diff` — show diffs
- `git_commit` — create commit
- `git_branch` — create/list branches
- `git_checkout` — switch branches
- `git_undo` — revert last commit
- `git_redo` — reapply undone commit

#### LSP Bridge

Language server integration for code intelligence.

```bash
hone -i "What does this function do?"
# Agent calls: lsp_hover(file="main.rs", line=10, col=5)

hone -i "Show me all errors in this file"
# Agent calls: lsp_diagnostics(file="main.rs")

hone -i "Fix the type error on line 42"
# Agent calls: lsp_code_action(file="main.rs", line=42)
```

Supports all languages with LSP servers: Python, Rust, TypeScript, Go, Java, etc.

#### Semantic Index

Search code by meaning (not keywords).

```bash
hone -i "Find functions that handle authentication"
# Agent calls: semantic_search(query="authentication", limit=10)
```

## Layer 1: MCP Integration

Connect external tools via the Model Context Protocol (JSON-RPC over stdio).

### What is MCP?

MCP is a standardized protocol for language models to call external tools. Hone acts as an MCP client, spawning MCP servers as subprocesses and forwarding tool calls.

**Benefits:**
- Plugin architecture — tools in any language
- Lazy schema loading — only fetch tool definitions on first use
- JSON-RPC transport — flexible, testable
- Full bidirectional — tool can push data back to agent

### Configuring MCP Servers

In `~/.config/hone/config.toml`:

```toml
[[mcp_servers]]
name = "github"
command = "npx"
args = ["@modelcontextprotocol/server-github"]
env = { GITHUB_TOKEN = "ghp_..." }

[[mcp_servers]]
name = "postgres"
command = "/usr/local/bin/mcp-postgres"
args = ["--connection-string", "postgresql://user:pass@localhost/db"]

[[mcp_servers]]
name = "slack"
command = "python3"
args = ["/path/to/mcp-slack-server"]
env = { SLACK_BOT_TOKEN = "xoxb-..." }
```

### Using MCP Tools

Once configured, MCP tools appear automatically in the agent's tool registry:

```bash
hone -i "Open a GitHub issue"
# Agent lists available tools:
# - github_create_issue
# - github_list_repos
# - github_add_comment
# ...agent calls: github_create_issue(repo="user/repo", title="...", body="...")

hone -i "Query the database for users"
# Agent calls: postgres_query(sql="SELECT * FROM users WHERE active=true")

hone -i "Post a message to Slack"
# Agent calls: slack_send_message(channel="general", text="...")
```

### Writing Custom MCP Servers

Create your own MCP server in any language. Here's a minimal Node.js example:

```javascript
// server.js — MCP server that provides a custom "translate" tool
const readline = require("readline");

const rl = readline.createInterface({
  input: process.stdin,
  output: process.stdout,
});

const tools = [
  {
    name: "translate",
    description: "Translate text from English to another language",
    inputSchema: {
      type: "object",
      properties: {
        text: { type: "string" },
        language: { type: "string" }
      },
      required: ["text", "language"]
    }
  }
];

let messageId = 0;

rl.on("line", (line) => {
  try {
    const message = JSON.parse(line);
    
    if (message.method === "tools/list") {
      console.log(JSON.stringify({
        jsonrpc: "2.0",
        id: message.id,
        result: { tools }
      }));
    }
    
    if (message.method === "tools/call") {
      const { name, arguments: args } = message.params;
      let result = {};
      
      if (name === "translate") {
        // Dummy translation
        result = {
          translated: `[Translation to ${args.language}]: ${args.text}`
        };
      }
      
      console.log(JSON.stringify({
        jsonrpc: "2.0",
        id: message.id,
        result
      }));
    }
  } catch (e) {
    console.error(e);
  }
});
```

Register in config:

```toml
[[mcp_servers]]
name = "translator"
command = "node"
args = ["/path/to/server.js"]
```

Use it:

```bash
hone -i "Translate 'hello' to Spanish"
# Agent calls: translate(text="hello", language="Spanish")
# Output: "Hola"
```

## Layer 0: Code Mode

Advanced orchestration via agent-generated scripts. The agent generates TypeScript/Python code that calls native tools and MCP tools via SDK imports.

**When to use Code Mode:**
- Complex multi-step workflows
- Conditional logic based on tool outputs
- Loops over collections
- Custom aggregation of results

### Example

```bash
hone -i "Find all Python files, count lines of code, and report top 10 longest files"
```

Agent generates (pseudocode):

```typescript
import { glob, readFile } from "@hone/native";
import { slack } from "@hone/mcp";

const files = await glob("**/*.py");
const results = [];

for (const file of files) {
  const content = await readFile(file);
  const lines = content.split("\n").length;
  results.push({ file, lines });
}

results.sort((a, b) => b.lines - a.lines);
const top10 = results.slice(0, 10);

await slack.send_message({
  channel: "general",
  text: `Top 10 longest Python files:\n${top10.map(r => `${r.file}: ${r.lines} lines`).join("\n")}`
});
```

Hone executes this generated code in a sandboxed subprocess, streaming results back.

## Layer 3: Skills (HONE.md)

Markdown conventions parsed at startup. Zero runtime cost. Use this for system prompts, example workflows, and reusable prompts.

Create `HONE.md` in your project root:

```markdown
# Project Skills

## Skill: Run Tests

When asked to run tests, always use `cargo test` and capture both stdout and stderr.
Explain any failures.

## Skill: Commit with Care

Before committing:
1. Run `cargo test`
2. Run `cargo clippy`
3. Review the diff with `git diff`
4. Ask for confirmation

## Skill: Code Review

When reviewing code:
1. Check for proper error handling
2. Verify all unwrap() calls are justified
3. Suggest better variable names
4. Test changes locally
```

The agent learns these conventions without token overhead — they're parsed once at startup and become part of the system prompt.

## Tool Registry and Dispatch

Hone maintains a registry of all available tools (L0, L1, L2, L3) and dispatches calls based on availability.

Access programmatically:

```rust
use hone_tools::ToolRegistry;

let registry = ToolRegistry::default();

// List all tools
for tool in registry.all() {
    println!("{}: {}", tool.name(), tool.description());
}

// Call a tool
let input = ToolInput::new(serde_json::json!({
    "path": "main.rs"
}));
let output = registry.get("read_file").unwrap().call(input, &ctx).await?;
println!("{}", output.into_value());
```

## Writing Custom Tools

Create a custom native tool by implementing the `Tool` trait:

```rust
use hone_tools::{Tool, ToolContext, ToolInput, ToolOutput, ToolError};
use async_trait::async_trait;

struct MyTool;

#[async_trait]
impl Tool for MyTool {
    fn name(&self) -> &str {
        "my_custom_tool"
    }
    
    fn description(&self) -> &str {
        "A custom tool that does something special"
    }
    
    fn schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "input": { "type": "string" }
            },
            "required": ["input"]
        })
    }
    
    async fn call(
        &self,
        input: ToolInput,
        ctx: &ToolContext,
    ) -> Result<ToolOutput, ToolError> {
        let value = input.into_value();
        let text = value["input"].as_str().ok_or_else(|| {
            ToolError::InvalidInput("expected 'input' string".into())
        })?;
        
        // Your logic here
        let result = format!("Processed: {}", text);
        
        Ok(ToolOutput::new(serde_json::json!({ "result": result })))
    }
}
```

Add to the registry:

```rust
let mut registry = ToolRegistry::default();
registry.register(Box::new(MyTool));
```

## Sandboxing Tool Execution

All tool calls are sandboxed for security. See [Security Guide](security.md) for details on:
- Path restrictions (allow-list / deny-list)
- Network policies (deny-by-default)
- Environment variable filtering
- Process resource limits

## Next Steps

- **Getting started:** See [Getting Started](getting-started.md) to use built-in tools.
- **Recipes:** Combine tools in [Recipes](recipes.md) for multi-step workflows.
- **Security:** Learn sandbox policies in [Security Guide](security.md).
- **Server:** Use tools remotely via [Server](server-and-remote.md).
