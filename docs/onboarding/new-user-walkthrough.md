# New User Walkthrough: AI Coding Agents Explained

Welcome. If you've never used an AI coding agent before, this guide will walk you through the fundamentals, step by step. No assumptions about prior knowledge.

## What is an AI Coding Agent?

An AI coding agent is a conversational AI (like ChatGPT) that can read and modify your code. Instead of you copy-pasting code into a chat window and manually applying suggestions, the agent:

1. Understands what you're asking (natural language)
2. Reads relevant files from your project
3. Writes or modifies code autonomously
4. Runs tests to verify changes
5. Commits changes to git

**Example:**
```
You: Fix the null pointer exception in main.rs
Agent: [reads main.rs, identifies the bug, suggests a fix, shows you a diff, waits for approval]
You: Looks good, apply it
Agent: [applies the fix, runs tests to verify, commits to git]
```

Think of it as a very capable pair programmer who understands your codebase and can execute changes safely.

## Installing Hone

Hone is built in Rust and runs entirely on your machine. Here's how to install it.

### Prerequisites

You need:
- **Rust 1.85+** — the language Hone is written in
- **Git** — for version control
- **A command-line terminal** — bash, zsh, or similar

### Check What You Have

```bash
# Check Rust version
rustc --version
# Expected: rustc 1.85.0 or higher

# Check Cargo (comes with Rust)
cargo --version

# Check Git
git --version
```

If any are missing, install Rust from https://rustup.rs (it installs everything together).

### Build Hone

```bash
# Clone the repository
git clone https://github.com/anthropics/hone.git
cd hone

# Build Hone (this takes ~3-5 minutes)
cargo build --release

# Install the binaries
sudo cp target/release/hone /usr/local/bin/
sudo cp target/release/honed /usr/local/bin/

# Verify installation
hone --version
```

That's it. Hone is a single, self-contained binary. No dependencies to manage.

## Your First Real-World Session

Let's say you're working on a Python project and you have a failing test. Here's what happens step by step.

### Step 1: Start Hone

```bash
cd /path/to/your/python/project
hone
```

You'll see the TUI (terminal UI) load. Hone asks you to choose a model provider.

### Step 2: Choose a Provider

A provider is where the AI comes from. Hone supports many:

```
Select a provider:
1. Anthropic Claude (requires API key)
2. OpenAI GPT (requires API key)
3. Ollama (local, free, no API key)
4. Claude CLI (if you already have it installed)
... (70+ other options)

Choice: 
```

**For your first time:**
- If you have an OpenAI account, use OpenAI (GPT-4o)
- If you have an Anthropic account, use Anthropic (Claude)
- If you want free/private, use Ollama (see setup below)

### Setting Up Ollama (Free Option)

If you choose Ollama (local, free, private):

```bash
# Install Ollama from https://ollama.ai
# Then in a separate terminal:
ollama serve

# In your Hone session:
hone --model ollama:llama2
```

Ollama runs models on your machine. No internet, no API key, no cost. Trade-off: slower than cloud models (depends on your hardware).

### Step 3: In Your First Session

Once you've selected a model, you're in the chat loop. Type naturally:

```
You: There's a failing test in tests/test_auth.py. Fix it.

Hone: I'll help you fix the failing test. Let me start by reading the test file.

[Tool: read_file]
Input: tests/test_auth.py
Output: (file contents shown)

Now let me read the implementation to understand what needs fixing.

[Tool: read_file]
Input: src/auth.py
Output: (file contents shown)

I see the issue. The `authenticate()` function doesn't handle the case where the password is empty. Here's the fix:

[Tool: edit_file]
Input: src/auth.py, line 15-20
Old:
  def authenticate(username, password):
    user = db.find(username)
    return user.check_password(password)

New:
  def authenticate(username, password):
    if not password:
      raise ValueError("Password cannot be empty")
    user = db.find(username)
    return user.check_password(password)

[Show diff? (y/n)] y
```

At this point, you see a diff. Review it and type `y` to accept or `n` to reject.

### Step 4: Watch the Agent Work

After you approve, the agent continues:

```
[Tool: shell]
Input: pytest tests/test_auth.py -v
Output:
  test_auth.py::test_authenticate_valid ... PASSED
  test_auth.py::test_authenticate_invalid ... PASSED
  test_auth.py::test_authenticate_empty ... PASSED
  ===== 3 passed in 0.15s =====

Great! All tests pass. Let me commit this fix.

[Tool: git_commit]
Input: message="fix: handle empty password in authenticate()"
Output: [main a1b2c3d] fix: handle empty password in authenticate()
  1 file changed, 3 insertions(+)
```

Done. The agent:
1. Read the failing test
2. Read the implementation
3. Identified the bug
4. Showed you a diff
5. Got your approval
6. Ran tests to verify
7. Committed the change

**Your role:** Review and approve important changes. You're always in control.

## Understanding the Output

### Token Streaming

You'll notice text appearing word-by-word:

```
Hone: I'm analyzing... your request... and will...
```

That's "token streaming." The AI generates text one word at a time, and Hone displays each word as it arrives. This makes it feel faster because you see progress immediately, rather than waiting for a full response.

### Tool Calls

Tool calls are actions the agent takes:

```
[Tool: read_file] src/main.py
[Tool: shell] cargo test
[Tool: git_commit] message="..."
```

Each tool call is visible, logged, and auditable. You see exactly what the agent is doing.

**Built-in tools:**
- `read_file` — read a file from disk
- `write_file` — create a new file
- `edit_file` — modify a file (precise edits, not full rewrites)
- `shell` — run a command (bash, python, cargo, etc.)
- `git_commit` — create a commit
- `git_diff` — show changes
- `git_checkout` — switch branches
- `glob` — search for files by pattern (e.g., `*.py`)
- `lsp_hover` — get type information (language-aware)

### Cost Tracking

Each turn shows how much you're spending (if using a paid API):

```
Usage: 4521 input tokens, 312 output tokens
Cost: $0.019 USD (using openai:gpt-4o)
Session total: $0.24 USD
Budget remaining: $9.76 / $10.00
```

Tokens are the unit of text the AI processes. You're billed per token. Hone tracks this transparently.

**If using Ollama (local):** Cost is $0.00 (completely free).

## Key Concepts Explained

### 1. Models

A "model" is the AI engine. Different models have different strengths:

| Model | Cost | Speed | Reasoning | Best For |
|-------|------|-------|-----------|----------|
| Claude Opus (Anthropic) | High | Slow (20-30s) | Excellent | Complex tasks, design |
| GPT-4o (OpenAI) | Medium | Medium (10-20s) | Very good | Balanced tasks |
| Claude Haiku (Anthropic) | Low | Very fast (2-5s) | Good | Simple tasks, fast feedback |
| Llama 2 (Ollama) | Free | Very slow (depends on hardware) | Fair | Testing, no cost |

You can switch models mid-session with `/model`:

```
/model openai:gpt-4o  (switch to GPT-4o)
/model anthropic:claude-haiku  (switch to faster, cheaper Haiku)
```

**Tip:** Use fast/cheap models for simple tasks, powerful models for complex reasoning.

### 2. Tools

Tools are actions the agent can take. Layer 2 (native) tools are built-in and very fast:

- **File I/O** — read, write, edit (< 1ms latency)
- **Shell** — run commands (speed depends on command)
- **Git** — commit, branch, diff (very fast)
- **LSP** — language server (type info, diagnostics)

Layer 1 (MCP) tools extend Hone:
- **GitHub** — create issues, manage PRs
- **Slack** — send messages
- **Databases** — query and insert
- Custom tools (you can build them)

The agent chooses which tools to use based on your request.

### 3. Sandbox (Why It's Safe)

Hone runs code in a sandbox. If the agent tries to do something unsafe (delete system files, access the network without permission, etc.), the sandbox stops it:

```bash
# Show the sandbox policy
hone --show-sandbox-policy

# Output:
# Sandbox: Seatbelt (macOS)
#   Allow: Read files in /Users/you/project
#   Allow: Execute /bin/bash, /usr/bin/python3
#   Deny: Read /etc/passwd
#   Deny: Access the network (except api.openai.com, github.com)
#   Deny: Access /private/var, /tmp (except /tmp/.hone-*)
```

The sandbox uses your operating system's native security (Seatbelt on macOS, Landlock on Linux). It's the same technology Safari uses.

**In practice:** The agent can't do anything destructive. It can only modify code in your project, run tests, and commit to git.

### 4. Memory

Hone remembers your patterns:

**First session:**
```
You: Run the tests
Agent: [runs pytest]

You: Commit this fix
Agent: [commits]
```

**Second session:**
```
You: Fix this bug
Agent: (learns you typically run pytest after fixing, suggests it)
Agent: Should I run pytest to verify?
```

Memory is encrypted and stored locally on your machine. Hone never uploads your code to the cloud.

## Using Built-in Agents and Recipes

Now that you understand the fundamentals, let's explore Hone's most powerful feature: built-in agents and recipes. These let you delegate specialized tasks to expert personas.

### What's an Agent?

An agent is a specialized AI persona. Instead of saying "fix this code," you can say "I want the security auditor to review this code." Each agent has:

- **A name** (e.g., `code-reviewer`, `debugger`)
- **Specialized tools** (file readers, shell, git, etc.)
- **Expert knowledge** (system prompt encoded with best practices)
- **A preferred model** (usually Sonnet for balanced speed/cost)

### Built-in Agents

Hone ships with 9 pre-trained agents:

| Agent | Use When |
|-------|----------|
| `code-reviewer` | You want a security/quality review |
| `debugger` | You're stuck on a bug |
| `security-auditor` | You need vulnerability scanning |
| `rust-expert` | You're writing Rust |
| `python-expert` | You're writing Python |
| `tdd-workflow` | You want to implement with TDD |
| `api-designer` | You're designing an API |
| `performance-optimizer` | You need to optimize code |
| `documentation-agent` | You need to write docs |

### Using an Agent

```bash
# Use a specific agent
hone --agent code-reviewer -i "Review this code for security issues"

# Find an agent by keyword
hone --agent "test-driven" -i "Implement a new feature"
# This finds "tdd-workflow" automatically

# Use an agent in a recipe (we'll cover recipes next)
hone --recipe tdd-feature --agent rust-expert
```

During the session, you can also switch agents:

```
/agent debugger
```

### What's a Recipe?

A recipe is a structured workflow. Instead of manually running steps one by one, a recipe defines:

1. What to do in each step
2. The order to do them
3. Whether to run tests at the end

Hone ships with 4 built-in recipes:

| Recipe | What It Does |
|--------|-------------|
| `tdd-feature` | Implement a feature with Red-Green-Refactor |
| `bug-fix` | Fix a bug with a regression test |
| `code-review` | Comprehensive code review (5 steps) |
| `refactor` | Safely refactor with test verification |

### Running a Recipe

```bash
# Run a built-in recipe
hone --recipe tdd-feature
hone --recipe bug-fix
```

Here's what a TDD feature run looks like:

```
Recipe: tdd-feature
Step 1/5: Understand
  Prompt: Read the relevant code and understand the current architecture...
  Agent: [reads files, summarizes the codebase]
  Continue? (y/n) y

Step 2/5: Red
  Prompt: Write failing tests that describe the desired behavior...
  Agent: [writes tests, runs them to confirm failure]
  Continue? (y/n) y

Step 3/5: Green
  Prompt: Write the minimum code to make tests pass...
  Agent: [writes implementation]
  Continue? (y/n) y

Step 4/5: Refactor
  Prompt: Refactor for clarity and quality...
  Agent: [improves code]
  Continue? (y/n) y

Step 5/5: Verify
  Prompt: Run all tests...
  Agent: [runs full test suite, reports success]
  
Test command: cargo test
Result: PASSED (all 12 tests)

Recipe completed successfully.
```

### Combining Agents and Recipes

You can use an agent with a recipe. For example, TDD in Rust:

```bash
hone --recipe tdd-feature --agent rust-expert
```

The `rust-expert` agent will apply Rust best practices (Arc, ownership, etc.) while following the TDD workflow.

### Creating a Custom Agent

If you find yourself doing the same type of work repeatedly, create a custom agent.

Create `.hone/agents/my-agent.md`:

```markdown
---
name: my-agent
description: My custom agent for X
tools: [read, write, shell, grep, glob]
model: sonnet
---

You are a specialized agent for [your domain].

## Process

1. Understand the task
2. Take action
3. Verify results

[Your system prompt...]
```

Then use it:

```bash
hone --agent my-agent -i "Your task"
```

See [Tools and MCP Guide](../tutorials/tools-and-mcp.md#agents) for detailed examples.

---

## Common Workflows

### Bug Fixing with the Debugger

```bash
hone --agent debugger -i "The login form throws 'TypeError: cannot read password'. Fix it."
```

The debugger agent uses a systematic approach:
1. Reproduce the bug
2. Form hypotheses
3. Test hypotheses
4. Write the minimal fix
5. Verify with a regression test

You review and approve the fix.

### Code Review with Security Focus

```bash
hone --agent security-auditor -i "Review this code for vulnerabilities"
```

The security auditor scans for:
- SQL injection, XSS, command injection
- Hardcoded secrets
- Authentication/authorization flaws
- Cryptography issues
- Known CVEs in dependencies

### Implement a Feature with TDD

```bash
hone --recipe tdd-feature --agent rust-expert
```

This combines the TDD workflow with Rust expertise. The agent:
1. Understands the architecture
2. Writes failing tests
3. Implements the minimum code to pass
4. Refactors for clarity
5. Verifies all tests still pass

### Refactor Code Safely

```bash
hone --recipe refactor
```

The refactor recipe enforces test verification at each step:
1. Establish a green baseline
2. Plan the refactoring
3. Execute changes one at a time
4. Run tests after each change
5. Report before/after metrics

You approve each step incrementally.

### Code Review (General)

```bash
hone --agent code-reviewer -i "Review this PR"
```

The code reviewer performs a systematic review:
1. Understand the scope
2. Check for correctness
3. Check for security issues
4. Check for performance issues
5. Report findings in a table

### Add Documentation

```bash
hone --agent documentation-agent -i "Generate API documentation"
```

The documentation agent can:
- Generate API docs from code
- Create architecture diagrams (Mermaid)
- Write READMEs
- Generate changelogs from git history

## Tips for Effective Prompting

### 1. Be Specific About Paths

**Bad:**
```
Fix the bug in the main file.
```

**Good:**
```
Fix the null pointer exception in src/main.rs, around line 42.
```

Hone can search, but it's faster if you point it to the right file.

### 2. Describe the "Why" Not Just "What"

**Bad:**
```
Add logging.
```

**Good:**
```
Add structured logging to database query functions so we can debug slow queries.
```

With context about the why, the agent makes better decisions.

### 3. Use `/model` for Different Tasks

**For complex reasoning (designing architecture, planning):**
```
/model anthropic:claude-opus-4-5
Design a caching layer for the API.
```

**For simple, quick tasks:**
```
/model anthropic:claude-haiku
Add a comment to line 10 of main.rs.
```

This saves money and speeds up simple tasks.

### 4. Use `/budget` to Avoid Surprises

If you're trying a new model or expensive operation:

```
/budget 1.00  (stop after spending $1.00)
```

When you hit the budget, the session stops.

### 5. Request Incremental Changes

**Bad:**
```
Refactor the entire database module.
```

**Good:**
```
Refactor the database module incrementally. First, move the User model to a separate file. Then we'll handle relations.
```

Incremental changes are easier to review and revert if needed.

### 6. Use Slash Commands

Type `/help` anytime to see available commands:

```
/model <provider:model>   — switch models
/budget <usd>             — set spending limit
/compact                  — reduce output verbosity
/clear                    — clear conversation history
/export <path>            — save session to JSON
/context show             — show what files are loaded
/context add <path>       — add a file to context
/context drop <path>      — remove a file from context
/undo                     — undo last agent turn
/redo                     — redo last undo
```

## Next Steps: Learning More

### 1. Try Your First Session

Pick a real project. Start small:
```bash
cd /your/project
hone
```

Ask it to fix a simple bug or add a small feature. See how it works.

### 2. Read the Tutorials

Now that you understand the fundamentals:
- **[Getting Started](../tutorials/getting-started.md)** — More detailed setup guide
- **[Model Providers](../tutorials/providers.md)** — Deep dive into provider selection and cost
- **[Tools and MCP](../tutorials/tools-and-mcp.md)** — How tools work, agents, how to extend them
- **[Recipes Guide](../tutorials/recipes.md)** — Full recipe syntax and writing custom recipes

### 3. Set Up a Configuration

Create `~/.config/hone/config.toml` to customize defaults:

```toml
[agent]
default_model = "anthropic:claude-opus-4-5"
context_window_tokens = 16000
max_spend_usd = 10.00

[memory]
enabled = true
```

See [Configuration](../configuration.md) for all options.

### 4. Create Project Conventions (HONE.md)

In your project root, create `HONE.md`:

```markdown
# Project Skills

## Skill: Testing

Always run pytest after making changes:
  pytest -v tests/

## Skill: Commit Messages

Use conventional commits:
  fix: resolve race condition
  feat: add user authentication
  docs: update README
```

The agent learns these conventions (zero token cost).

### 5. Explore Recipes

When you find yourself doing the same workflow repeatedly, automate it with a recipe.

See [Recipes Guide](../tutorials/recipes.md).

### 6. Join the Community

- Report bugs or ask questions on GitHub
- Read the [Architecture](../architecture.md) guide for deep dives
- Check out [Contributing](../contributing.md) if you want to extend Hone

## Troubleshooting Your First Session

### "API key not found"

Set your API key as an environment variable:

```bash
export ANTHROPIC_API_KEY="sk-ant-..."
# or
export OPENAI_API_KEY="sk-..."

hone
```

Or add it to config:

```toml
# ~/.config/hone/config.toml
[providers.anthropic]
api_key = "sk-ant-..."
```

### "Model not supported"

List available models:

```bash
hone --list-models
```

Use one from the list.

### "Network error"

Check your internet connection and firewall. If behind a corporate proxy, configure it:

```toml
[http]
proxy = "http://proxy.company.com:8080"
```

### "Sandbox error"

If you see a sandbox error, Hone is being extra safe. Check System Preferences > Security & Privacy on macOS. For Linux, ensure Landlock is available (kernel 5.13+).

### "Out of memory"

Your conversation got too long. Clear it:

```
/clear
```

Or reduce context size:

```toml
[agent]
context_window_tokens = 8000
```

## You're Ready

That's the fundamentals. You now understand:
- What an AI coding agent is
- How to install and run Hone
- What happens in a real session
- Key concepts (models, tools, sandbox, memory)
- How to prompt effectively

**Next:** Pick a real task and give it a try. Hone is safest to learn by doing.

---

Questions? Open an issue on GitHub or read the full [Getting Started](../tutorials/getting-started.md) guide.
