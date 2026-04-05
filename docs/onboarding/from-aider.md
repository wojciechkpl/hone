# Migrating from Aider to Hone

You know Aider. You're comfortable with git-centric workflows, repo maps, voice input, and multi-file editing. Here's how Hone extends those ideas—and what's new.

## Concept Mapping

| Aider | Hone Equivalent | Notes |
|-------|-----------------|-------|
| Repo map (code context) | Semantic index (AST + embeddings) | More powerful, adaptive |
| `/add` / `/drop` (context management) | Context window management + agent control | Automatic in Hone |
| Auto-commit | AutoCommitter + git tools | Implemented + git undo/redo |
| Voice input | Voice input (planned) | Not yet, but CLI is stable |
| `/architect` (plan mode) | Model routing (use opus for planning) | Implemented |
| Multi-file editing | Multi-file edits via agent loop | Native support |
| Edit request format | Edit tool + natural language | More flexible, agent chooses strategy |
| Chat history | Memory + encrypted SQLite | More powerful adaptive learning |
| Model switching | `/model <provider:model>` hot-swap | Same command, broader provider support |

## Before and After

### Aider Workflow

```bash
# Start with a project
cd my-project
aider

# In session:
# /add src/main.py tests/test_main.py  (add files to context)
# Implement user login with password hashing
# /drop tests/  (remove from context)
# Now add two-factor auth
# /architect  (switch to planning mode)
# Design the database schema
```

**Strengths:** Git-aware, can modify multiple files in one turn, voice input.

### Hone Workflow

```bash
# Start with a project
cd my-project
hone

# In session:
# Implement user login with password hashing
# (Hone automatically finds relevant files via semantic search)
# Now add two-factor auth
# (Hone adjusts context window as needed)
# /model anthropic:claude-opus-4-5  (switch to most capable for planning)
# Design the database schema
```

**Differences:**
- No `/add` `/drop` — Hone manages context automatically
- No explicit "architect mode" — Use `/model` to route to a capable model
- Voice coming (planned, not yet implemented)

## What You Gain

### 1. Semantic Indexing (Better Than Repo Map)

Aider uses basic lexical search. Hone uses semantic indexing:

**Aider repo map:**
```
src/
  auth/
    - login.py (100 lines)
    - verify.py (50 lines)
  db/
    - schema.py (200 lines)
    - models.py (300 lines)
tests/
  - test_auth.py (100 lines)
  - test_db.py (80 lines)
```

When you ask to "add two-factor auth," Aider searches by keywords ("factor", "auth").

**Hone semantic index:**
```bash
# Understand structure + meaning
hone -i "Find functions that handle authentication"
# Result: Returns auth.login(), auth.verify_token(), auth.setup_2fa()
# (by meaning, not keywords)

hone -i "Where is the user table defined?"
# Result: models.py, schema.py, migrations/ (related files)
```

Hone uses:
- Tree-sitter for AST parsing (language-aware)
- SQLite vector embeddings (semantic similarity)
- Persistent index (learned, cached across sessions)

### 2. Automatic Context Management

Aider requires you to `/add` files manually. Hone does it automatically:

**Aider:**
```bash
aider
# /add src/auth/login.py src/db/models.py tests/
# Now it's in context. When context is full:
# /drop tests/  (drop something to make room)
```

**Hone:**
```bash
hone
# Agent automatically:
# 1. Finds relevant files via semantic search
# 2. Estimates tokens needed
# 3. Adds to context
# 4. If context overflows, removes least-relevant files
# All automatic, visible in debug logs
```

You can still control it:
```
/context show      # See what's loaded
/context add path  # Manually add a file
/context drop path # Manually remove
```

### 3. Git UndoStack (Beyond Auto-Commit)

Aider auto-commits. Hone goes further with a full undo/redo system:

**Aider:**
```bash
# Agent makes changes, auto-commits
# If you don't like it, undo with git
git revert HEAD
```

**Hone:**
```bash
# Agent makes changes, auto-commits (if enabled)
# You can:
git_undo          # Revert last change
git_redo          # Reapply
git_diff          # Review before accepting
/undo             # Undo last agent turn (not just last commit)
/redo             # Redo undone turn
```

Hone tracks agent turns, not just commits.

### 4. Kernel Sandboxing

Aider relies on process isolation. Hone uses kernel-level containment:

- **macOS:** Seatbelt (same as Safari)
- **Linux:** Landlock (same as systemd)

When agent edits files or runs commands, the OS enforces restrictions at kernel level. No privilege escalation possible.

### 5. LSP Integration

Aider has no built-in LSP. Hone bridges language servers:

```bash
hone -i "What's the return type of this function?"
# Agent calls: lsp_hover(file="main.rs", line=42, col=10)
# Returns: type information from Rust analyzer

hone -i "Fix the type errors in this file"
# Agent calls: lsp_diagnostics(file="main.rs")
# Sees all errors, fixes them systematically
```

### 6. Provider Freedom

Aider locks you to OpenAI. Hone supports 75+ providers:

```bash
hone --model openai:gpt-4o              # OpenAI (Aider's default)
hone --model anthropic:claude-opus-4-5  # Anthropic (more capable for planning)
hone --model ollama:llama2              # Local (free, private)
```

Hot-swap mid-session without losing history.

## What's Different

### 1. No Voice (Yet)

Aider supports voice input. Hone doesn't (planned). For now, type or pipe input:

```bash
# Voice workaround (until Hone adds native support)
hone << 'EOF'
Refactor the authentication module to support JWT tokens
EOF

# Or use a voice-to-text tool
say "Refactor the authentication module" | \
  speech-to-text | \
  hone
```

### 2. No Explicit `/architect` Mode

Aider has `/architect` for planning. Hone routes to capable models instead:

**Aider:**
```
/architect
Design a caching layer for the API
```

**Hone:**
```
/model anthropic:claude-opus-4-5  (switch to most capable model)
Design a caching layer for the API
```

Or let Hone route automatically (planned feature):
```
# Hone detects "design" keywords and routes to opus automatically
Design a caching layer for the API
```

### 3. Multi-File Edits Work Differently

**Aider:** You specify files, agent edits all of them in one turn:
```
/add src/auth.py src/db.py src/tests/test_auth.py
Now add two-factor authentication (modify both auth.py and db.py)
# Agent makes edits to both files
```

**Hone:** Agent loops and makes multiple edits:
```
Add two-factor authentication to the auth module
# Agent turn 1: reads auth.py, designs changes
# Agent turn 2: edits auth.py
# Agent turn 3: reads db.py, designs schema changes
# Agent turn 4: edits db.py
# Agent turn 5: runs tests, verifies
```

Why? Hone's layered tool architecture allows granular, auditable changes.

### 4. Config Format

**Aider:** Command-line flags or `.aider.conf.json`

**Hone:** TOML config + HONE.md conventions

```toml
# ~/.config/hone/config.toml
[agent]
default_model = "openai:gpt-4o"  # match your Aider default
max_iterations = 20
auto_commit = true               # same as Aider

[git]
commit_on_change = true
require_approval = false

[memory]
enabled = true

[sandbox]
strategy = "seatbelt"  # macOS: seatbelt, Linux: landlock
```

```markdown
# HONE.md (in your project)
## Skill: Multi-File Edits

When making related changes across multiple files:
1. Read all affected files first
2. Design changes in natural language
3. Execute edits one file at a time
4. Test changes with unit tests
5. Commit with clear messages
```

## Step-by-Step Migration

### 1. Install Hone

```bash
git clone https://github.com/anthropics/hone.git
cd hone
cargo build --release
sudo cp target/release/hone /usr/local/bin/
```

### 2. Configure Your Default Model

If you've been using OpenAI with Aider:

```bash
export OPENAI_API_KEY="sk-..."

# Create config
mkdir -p ~/.config/hone
cat > ~/.config/hone/config.toml << 'EOF'
[providers.openai]
api_key = "${OPENAI_API_KEY}"

[agent]
default_model = "openai:gpt-4o"
auto_commit = true
EOF
```

### 3. Create HONE.md for Your Project

Copy any Aider system prompts or project guidelines:

```markdown
# Project Skills

## Skill: Multi-File Edits

When making changes across files:
1. Read all affected files
2. Plan changes clearly
3. Edit files one at a time
4. Run tests after each edit
5. Commit with a clear message

## Skill: Testing

Always run pytest after making changes:
  pytest -v tests/

## Skill: Code Review

Before committing:
1. Review changes with `git diff`
2. Check for error handling
3. Verify all unwrap() calls are justified
4. Run clippy/pylint as appropriate
```

### 4. Test Your First Session

```bash
cd /your/aider/project
hone
```

Try a familiar workflow:
```
Refactor the login module to use JWT tokens
```

Hone will:
1. Search for related files (no `/add` needed)
2. Read them automatically
3. Make changes
4. Test (if it finds test files)
5. Commit (if auto_commit is true)

### 5. Set Up Auto-Commit (Optional)

If you liked Aider's auto-commit:

```toml
[git]
auto_commit = true
require_approval = false  # false = commit automatically, true = ask first
```

Or in session:
```
/auto-commit on
```

## FAQ

### Do I need to `/add` files like in Aider?

No. Hone manages context automatically via semantic search. You can still `/context add` if you want to force something.

### Will Hone support voice input?

Yes, it's planned. For now, type or pipe input from a speech-to-text tool.

### How does Hone handle multi-file edits compared to Aider?

Differently. Hone edits one file at a time (more auditable). For large refactors, it may take more turns, but each change is reviewable.

### Can I use my Aider system prompt in Hone?

Yes, move it to HONE.md. Same format, same semantics.

### How do I switch models mid-session like I did with Aider?

Use `/model`:
```
/model anthropic:claude-opus-4-5  # switch to Claude
/model openai:gpt-4o              # switch back to OpenAI
```

Context is preserved across switches.

### What about Aider's chat history?

Hone stores chat history in memory (encrypted SQLite). Sessions are replayable. You can also export:
```
/export session.json
```

## Next Steps

1. **Install:** Follow step 1 above
2. **Config:** Create `~/.config/hone/config.toml` with your model preference
3. **HONE.md:** Create HONE.md in your project with project guidelines
4. **Test:** Run `hone` and try a simple task
5. **Explore:** Read [Semantic Index](../tutorials/tools-and-mcp.md) for how context works
6. **Recipes:** Read [Recipes Guide](../tutorials/recipes.md) for automation

---

Migrating from Aider? Questions? Open an issue on GitHub.
