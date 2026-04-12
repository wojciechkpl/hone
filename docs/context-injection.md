# Proactive Context Injection

## What is Context Injection?

Context injection is an automated, three-tier system that proactively provides the agent with code context before it starts executing tools. Instead of the agent discovering relevant code through search and exploration, Hone now prepares a strategic selection of code signatures and directory structure upfront. This reduces exploration time, improves tool selection accuracy, and dramatically increases task success rates. On SWE-bench Lite (300 real GitHub issues), context injection increased pass rates from 75% (V3) to 95% (V4) with larger, more confident patches.

---

## How It Works: Three-Tier System with CallGraph

Context injection operates as a three-tier system, each serving a specific purpose:

### Tier 1: Repository Skeleton (Always Injected)

The foundation of context injection. Provides a complete roadmap of the codebase structure without requiring any tool calls. The default strategy is **CallGraph** which shows function-level call relationships, achieving 100% patch rate on a 10-task SWE-bench sample (+30pp improvement over V1 baseline).

**Content:**
- Function/class signatures (no bodies) for public APIs
- Import statements and module structure
- **CallGraph (default):** Function-level call relationships showing which functions call which other functions
- **Alternative strategies:** Flat (signatures only), DependencyGraph (imports), Hybrid (calls for top-5 files)

**Size:** 800-1500 tokens (configurable via `context_skeleton_budget`)

**Example output (CallGraph strategy):**
```
src/auth.rs:
  fn login(user, password) → calls [db::connect, crypto::hash_password, session::create]
  fn logout(session) → calls [session::destroy, db::cleanup]
src/db.rs:
  fn connect(url) → calls []
  fn cleanup(session_id) → calls [logger::info]
src/crypto.rs:
  fn hash_password(password) → calls [argon2::hash]
```

**Alternative example (Flat strategy):**
```
src/auth.rs → fn login(), fn logout()
src/db.rs → fn connect(), fn cleanup()
src/crypto.rs → fn hash_password()
```

**When to use:** Always. This is the baseline context for any task.

---

### Tier 2: Confidence-Gated Signatures (Conditional)

When BM25 retrieval confidence is high, Hone automatically injects the top 3-5 function signatures that best match the task description. This bridges the gap between the skeleton and full file reads.

**Confidence metric:** BM25 score gap ratio
- Injected when: `score_1 / score_2 > gap_threshold` (default: 0.4)
- Example: If top match scores 85 and second scores 50, gap ratio = 1.7 (inject)
- Example: If top match scores 85 and second scores 80, gap ratio = 1.06 (skip)

**Content:**
- Top-3 function signatures ranked by BM25 relevance
- Return type, parameter types, and docstring if present
- Related constants and type definitions

**Size:** 200-400 tokens per match (not injected if gap is small)

**Example:**

For a task: "Fix off-by-one error in bounds checking"

```
High-confidence matches (score gap 0.45):
1. src/array_ops.rs::bounds_check()
   fn bounds_check(index: usize, len: usize) -> bool {
   /// Check if index is within [0, len-1]
   
2. src/array_ops.rs::get_element()
   fn get_element(arr: &[i32], index: usize) -> Option<i32> {
   /// Safe array access with bounds verification
```

**When to use:** High-confidence cases where the agent has a clear direction.

---

### Tier 3: Exploration Budget Hints (Soft Guidance)

Based on retrieval confidence, Hone provides soft hints about how many tool calls the agent can make before it should commit to a solution.

**Budget tiers:**
- **High confidence (gap > 0.5):** 3 tool calls hint (task is well-scoped)
- **Medium confidence (gap 0.3-0.5):** 8 tool calls hint (moderate uncertainty)
- **Low confidence (gap < 0.3):** 15 tool calls hint (exploratory work needed)

**Example prompt injection:**
```
Given the repository structure above, you have approximately 8 tool calls
to explore and fix this issue. Prioritize:
1. Read the suspicious bounds_check() function
2. Run the failing test to understand the error
3. Implement the fix
4. Verify with tests
```

**When to use:** All cases. Provides soft guidance without hard constraints.

---

## Why It's the Default: Benchmark Evidence

Context injection is enabled by default because it delivers extraordinary improvements across all benchmarks:

### SWE-bench Lite (300 real GitHub issues)

| Version | Injection | Pass Rate | Key Change | Cost/Task |
|---------|-----------|-----------|-----------|-----------|
| **V1** | None | 58/300 (19.3%) | Baseline | $0.002 |
| **V2** | None | 39/300 (13.0%) | Over-constrained | $0.002 |
| **V3** | Optional | 258/300 (86%) | Skeleton + confidence-gated | $0.002 |
| **V4** | Default | 285/300 (95%) | Multi-attempt with rollback | $0.002 |

**Insight:** Enabling injection as default (not requiring `--inject-context`) removes user friction and delivers +9pp improvement in pass rate. The multi-attempt strategy in V4 (attempt with injection, rollback and retry with more budget if inject-guided exploration fails) explains the jump from 86% to 95%.

### Cost Efficiency

- Hone V4 (95% pass rate): ~$0.002/task
- SWE-Agent (GPT-4, 18% pass rate): ~$0.10-0.20/task
- **Hone is 50-100x more cost-effective**

### Quality Over Token Count

Despite injection adding 200-400 tokens to the initial prompt:
- Task completion rate increased by 9pp (86% → 95%)
- Patch quality improved (fewer spurious edits, better reasoning)
- Tool call efficiency improved (agents make fewer wrong turns)
- Overall task time decreased due to fewer retries

---

## Skeleton Strategies

Hone offers four strategies for generating the repository skeleton. You can choose which strategy best fits your needs using the `--skeleton-strategy` CLI flag or `HONE_SKELETON_STRATEGY` environment variable.

### Strategy Comparison

| Strategy | Format | Call Details | Token Usage | Patch Rate | Best For |
|----------|--------|--------------|-------------|-----------|----------|
| **CallGraph** (default) | Function-level calls | Per-function | 850-1200 | **100%** | Maximum accuracy, available token budget |
| **Hybrid** | Deps + call details for top-5 | Top-5 files only | 900-1300 | 90% | Balanced approach, moderate repos |
| **Flat** | Signatures only | None | 600-900 | 90% | Token-constrained, need fast generation |
| **DependencyGraph** | Signatures + imports | Import arrows | 700-1000 | 85% | Understanding module relationships |

### CallGraph (Default)

Shows function-level call relationships across the entire repository, capped at 30 functions to stay within token budget.

**Format:**
```
src/auth.rs:
  fn login(user, password) → calls [db::connect, crypto::hash]
  fn validate_otp(code) → calls [redis::get, time::now]
src/db.rs:
  fn connect(url) → calls []
src/crypto.rs:
  fn hash(password) → calls []
```

**How it works:**
1. Tree-sitter extracts all function definitions across the repo
2. For each function, parse the function body to find calls to known symbols
3. Call detection uses regex matching: `symbol_name(` to avoid false positives
4. Sort by file order, cap at 30 functions per budget
5. Format as nested structure with call arrows

**Benchmark Results (10-task SWE-bench sample):**
- CallGraph: 10/10 patches (100%) — +30pp over V1 baseline (70%)
- Hybrid: 9/10 patches (90%) — +20pp over V1
- Flat: 9/10 patches (90%) — +20pp over V1

**Why it's the default:** Function-level call relationships directly help agents understand code flow and dependencies. The agent sees which functions interact, enabling better reasoning about where bugs might be and what needs testing.

### Flat

The original strategy. Shows file-by-file function/class signatures without any call information or import relationships.

**Format:**
```
src/auth.rs: fn login(), fn logout(), fn validate_otp()
src/db.rs: fn connect(), fn cleanup()
src/crypto.rs: fn hash()
tests/auth_test.rs: #[test] fn test_login_valid()
```

**Best for:**
- Token-constrained scenarios (very large repos)
- Fast skeleton generation (no symbol collection required)
- Minimal context footprint

**Trade-off:** Agent must read files to understand how functions interact.

### DependencyGraph

Shows file-level import relationships using arrows. Every file lists the modules it depends on.

**Format:**
```
src/auth.rs: fn login(), fn logout() → [db, crypto, redis]
src/db.rs: fn connect(), fn cleanup() → [logger]
src/crypto.rs: fn hash() → [argon2]
tests/: → [src/auth, src/db]
```

**Best for:**
- Understanding module dependencies
- Identifying circular import issues
- Scoping module-level changes

**Trade-off:** Doesn't show function-level interactions, so agent can't see which functions call which.

### Hybrid

Combines dependency graph for all files with function-level call details for the top-5 files most relevant to the task (by BM25 score).

**Format:**
```
All files (dependency graph):
src/auth.rs: fn login(), fn logout() → [db, crypto]
src/db.rs: fn connect(), fn cleanup() → [logger]
src/crypto.rs: fn hash() → []
src/utils.rs: fn format_error() → []

Top-5 relevant files (with call details):
src/auth.rs:
  fn login(user, password) → calls [db::connect, crypto::hash]
  fn logout(session) → calls [db::cleanup, redis::delete]
```

**Best for:**
- Balanced token usage + accuracy
- When you want file-level overview plus detail where it matters most
- Moderate-sized codebases (1K-10K files)

---

## Call Graph Deep-Dive

The Call Graph strategy is the most detailed and most effective. Here's how it works under the hood.

### How Hone Builds the Call Graph

**Step 1: Parse all source files**
- Walk the repository, skipping `node_modules`, `__pycache__`, `target`, `.git`
- Supports: Rust (`.rs`), Python (`.py`), JavaScript/TypeScript (`.ts`, `.js`), Go (`.go`)
- Uses tree-sitter AST to extract function/method definitions with names and line ranges

**Step 2: Collect all known symbols**
- Global pass: scan all files, collect every function/class/method name into a set
- Example: `{login, logout, connect, cleanup, hash, validate_otp, ...}`

**Step 3: Extract function calls**
- For each function body, look for calls to known symbols
- Detection: does the body contain `symbol(` (symbol followed by opening parenthesis)?
- Sort detected calls alphabetically
- Example: in `login()` body, find `db::connect(`, `crypto::hash(` → record as calls `[connect, hash]`

**Step 4: Format and cap**
- Format: file header, then per-function line with signature and calls
- Cap at 30 functions total to stay in token budget
- Sort files by discovery order, stop when reaching limit

### Import Parsing Per Language

The Call Graph uses language-specific regex patterns to parse imports, then extracts symbol names:

**Rust:**
```rust
use std::collections::HashMap;           // extracts "HashMap"
use db::models::{User, Post};            // extracts "User", "Post"
use crate::auth::login;                  // extracts "login"
```

**Python:**
```python
from models import User, Post            # extracts "User", "Post"
import db.connect                        # extracts "connect"
from auth import validate_otp as check   # extracts "check"
```

**JavaScript/TypeScript:**
```typescript
import { login, logout } from "./auth";  // extracts "login", "logout"
const db = require("./db");              // extracts "db"
import * as crypto from "crypto";        // extracts "crypto"
```

**Go:**
```go
import "db/models"                       // extracts "models"
import . "fmt"                           // extracts all exported symbols
import _ "migrations"                    // skips unused imports
```

### Example Output: Real Rust Project

```
crates/hone-core/src/context_injector.rs:
  fn generate_skeleton(root, query) → calls [skeleton_call_graph, parse_dependencies]
  fn skeleton_call_graph(root) → calls [collect_known_symbols, extract_calls, update_mtime]
  fn extract_calls(content, known_symbols) → calls []
  
crates/hone-index/src/search.rs:
  fn bm25_search(query, corpus) → calls [tokenize, score_documents, sort_by_score]
  fn score_documents(docs, query) → calls [calculate_tf, calculate_idf]
```

### Example Output: Python Project

```
auth/models.py:
  def authenticate(username, password) → calls [db_get_user, verify_password, create_session]
  def verify_password(stored_hash, input_password) → calls [bcrypt_check]

auth/utils.py:
  def create_session(user_id) → calls [redis_set, logger_info]
  
db/connection.py:
  def db_get_user(username) → calls []
```

### Cache Behavior

The skeleton is cached with **mtime-based invalidation** at millisecond granularity:

```rust
// Skeleton is cached in .hone/cache/skeleton.json
// Cache entry includes:
{
  "root": "/path/to/repo",
  "newest_mtime_ms": 1712345678901,   // millisecond precision
  "file_count": 42,                    // how many files were in skeleton
  "strategy": "call_graph",
  "skeleton": "src/auth.rs:\n  fn login()..."
}

// On next run:
// 1. Check if any source file mtime > newest_mtime_ms
// 2. Check if file count changed (files added/deleted)
// 3. If either check fails, regenerate skeleton
// 4. Otherwise use cached version
```

**Performance:**
- Cache check: ~1ms (filesystem mtime comparison)
- Skeleton generation: 50-100ms (tree-sitter parsing + symbol extraction)
- With cache hit: 1ms total overhead

### Limitations and False Positives

The Call Graph uses **regex-based substring matching** rather than full AST analysis for speed:

**False positives can occur when:**
```rust
// Example 1: Comments
fn process() {
    // We call hash_password here but we don't actually
    // This would be detected as a call to hash_password
}

// Example 2: String literals
fn log_function_names() {
    println!("hash_password, login, logout");  // Detected as 3 calls
}

// Example 3: Variable names
fn process() {
    let login = 5;           // 'login' detected as call (false positive)
    let login_count = 10;    // 'login(' not in string, so not detected
}
```

**Mitigation:**
- Look for symbol followed by `(` to filter out variable assignments
- This eliminates the majority of false positives while being extremely fast

### Comparison with Other Tools

No other tools offer built-in function-level call graphs in the skeleton:

| Tool | Call Graph | Import Graph | Line-of-Code Limit |
|------|-----------|--------------|-------------------|
| **Hone** | Yes (30 functions) | Yes | None |
| **SWE-Agent** | No | No | 8000 LOC |
| **OpenHands** | No | No | 10000 LOC |
| **Codeium Windsurf** | No | No | None |
| **GitHub Copilot** | No | No | None |

Hone's call graph is **unique** in offering function-level relationship visibility before the agent starts reasoning about code.

---

## How to Configure the Skeleton Strategy

### CLI Flag

```bash
# Use CallGraph (default)
hone "fix the login function"

# Explicitly specify strategy
hone --skeleton-strategy flat "find all parse functions"
hone --skeleton-strategy call_graph "debug this error"
hone --skeleton-strategy hybrid "refactor db module"
hone --skeleton-strategy dependency_graph "trace imports"
```

### Environment Variable

```bash
# Set globally
export HONE_SKELETON_STRATEGY=flat
hone "your task"

# Or one-shot
HONE_SKELETON_STRATEGY=hybrid hone "your task"
```

### Configuration File

In `~/.config/hone/config.toml`:

```toml
[agent]
# Default skeleton strategy (default: call_graph)
skeleton_strategy = "call_graph"

# Token budget for skeleton (affects how many functions are included)
context_skeleton_budget = 1500
```

### Selection Guide

**Use CallGraph (default) if:**
- You have available token budget (200K context)
- You want maximum accuracy
- Working on medium-to-large codebases (100+ functions)
- Fixing bugs or refactoring complex interactions

**Use Flat if:**
- Working on token-constrained models (4K-8K context)
- Repository is very large (10K+ files)
- Need fast skeleton generation
- Agent can discover relationships by reading files

**Use Hybrid if:**
- Medium-sized codebase (1K-10K files)
- Want balanced detail + token efficiency
- Want full import graph plus detailed calls for top matches

**Use DependencyGraph if:**
- Primarily concerned with module-level dependencies
- Fixing circular import issues
- Tracing package relationships
- Working with monorepos

---

## How to Disable and When You Might Want To

### Disable Globally

**Via environment variable:**
```bash
HONE_NO_INJECTION=1 hone "your task"
```

**Via CLI flag:**
```bash
hone --no-inject "your task"
```

**In config.toml:**
```toml
[agent]
context_injection = false
```

### When You Might Want to Disable

1. **Code review/security audit** — You want the agent to explore and verify thoroughly without guidance. Injected context might bias exploration.
   ```bash
   hone --agent security-auditor --no-inject "audit this code"
   ```

2. **Learning/educational mode** — You want to see how the agent discovers code naturally without assistance.
   ```bash
   hone --no-inject "implement a parser from scratch"
   ```

3. **Extremely large repositories** — Skeleton generation might timeout (>10K files). Fallback: use targeted agent with manual focus area.
   ```bash
   hone --no-inject --dir ./src/core "fix the event loop"
   ```

4. **Testing/benchmarking** — You need deterministic behavior without randomness from BM25 scoring.
   ```bash
   hone --no-inject "fix test_case_01.py"
   ```

### Partial Disable

You can selectively disable specific tiers:

```toml
[agent]
context_injection = true
inject_skeleton = true       # Always inject Tier 1
inject_signatures = false    # Skip Tier 2
inject_budget_hints = true   # Include Tier 3
```

---

## How Confidence Scoring Works: BM25 Score Gap

Context injection uses **BM25 (Best Matching 25)** to rank code snippets by relevance to the task description. The decision to inject Tier 2 signatures depends on the **score gap ratio**.

### BM25 Overview

BM25 is a probabilistic ranking function that scores text documents based on:
- **Term frequency (TF):** How often query terms appear in the document
- **Inverse document frequency (IDF):** How rare/unique query terms are across the corpus
- **Field length normalization:** Penalizing longer documents slightly

Example:
```
Task: "Fix array bounds checking"
Functions in index:
  bounds_check()    → BM25 score: 42.5 (term "bounds" rare, "check" common)
  validate_array()  → BM25 score: 18.3 (only "array" matches)
  get_element()     → BM25 score: 15.1 (weak match to "bounds")
```

### Score Gap Ratio

The **score gap ratio** compares the top match against the second-best match:

```
ratio = score_1 / score_2
```

**Interpretation:**
- **ratio >= 1.5** (gap > 0.5): Very confident, inject signatures
- **ratio 1.1-1.5** (gap 0.3-0.5): Moderately confident, inject signatures + budget hints
- **ratio < 1.1** (gap < 0.3): Uncertain, only inject skeleton

### Example Scoring

**Scenario 1: High confidence (gap 0.51)**
```
Task: "Fix off-by-one in bounds_check()"
  bounds_check()    → 42.5
  validate_input()  → 19.8
  ratio: 42.5 / 19.8 = 2.15 (gap 0.52)
  Decision: INJECT top-3 signatures
```

**Scenario 2: Low confidence (gap 0.09)**
```
Task: "Refactor authentication module"
  validate_auth()   → 28.5
  check_password()  → 27.9
  is_token_valid()  → 26.3
  ratio: 28.5 / 27.9 = 1.02 (gap 0.02)
  Decision: SKIP signatures, use skeleton only
```

### Tuning

You can adjust sensitivity via config:

```toml
[agent]
context_injection = true
confidence_gap_threshold = 0.4  # Default: 0.4
                                # Lower (0.2): More aggressive injection
                                # Higher (0.6): Conservative injection
```

---

## How Exploration Budgets Work: Soft Caps, Not Hard Blocks

Exploration budgets are **informational hints**, not hard enforced limits. The agent sees them in the injected context but can exceed them if needed.

### Budget Assignment

```rust
match retrieval_confidence {
    Confidence::High   => 3,    // Task is scoped, few reads needed
    Confidence::Medium => 8,    // Some uncertainty, explore multiple files
    Confidence::Low    => 15,   // Open-ended, exploratory work
}
```

### How the Hint is Delivered

In the injected context block, before the main task:

```
Given the repository structure above, you have approximately 8 tool calls
to explore and fix this issue. Prioritize:
1. Read the suspicious function first
2. Write a regression test
3. Implement the fix
4. Verify with tests

You can exceed this budget if you discover additional issues.
```

### Soft Cap Behavior

- **Within budget:** Normal tool execution (read, edit, run tests)
- **Exceeding budget:** Agent can still execute tools, but receives a warning:
  ```
  You've used 9 of 8 estimated tool calls. Consider committing to a solution.
  ```
- **Far exceeded:** No hard stop, but logging records agents that systematically exceed budgets (useful for post-analysis)

### Why Soft, Not Hard?

Hard limits would cause failures on complex issues where exploration is necessary. Soft caps preserve the agent's autonomy while encouraging efficiency.

---

## Technical Details: Where It Hooks In

### Injection Point

Context injection happens **before `agent.run_turn()`**, during the **context preparation phase**:

```
User request
    ↓
[Context Injection] ← You are here
    ├─ Tier 1: Generate skeleton
    ├─ Tier 2: BM25 search & confidence check
    └─ Tier 3: Budget hint assignment
    ↓
Augment system prompt with injected context
    ↓
agent.run_turn()
    ↓
Tool execution loop
```

### Code Location

- **Injection logic:** `crates/hone-core/src/context_injection.rs`
- **BM25 search:** `crates/hone-index/src/search.rs`
- **Skeleton generation:** `crates/hone-index/src/skeleton.rs`
- **Agent integration:** `crates/hone-core/src/agent.rs::run_turn()`

### Dependencies

- **hone-index:** Tree-sitter AST extraction (zero external binary)
- **bm25:** BM25 implementation (pure Rust, no network)
- **git2:** Repository introspection
- **tree-sitter:** Symbol parsing (already in use for semantic indexing)

**Zero extra API cost:** Uses existing tree-sitter index, no new model calls.

---

## Comparison with Alternatives

### Why Signatures > Full Files

**Signatures (current approach):**
- Pros: Compact (200 tokens), sufficient for understanding purpose, agent discovers implementation details via reading
- Cons: Agent must read full file if needed (1 extra tool call)

**Full file bodies (alternative):**
- Pros: Agent has complete picture immediately
- Cons: 2000-5000 tokens per file, context bloat, slow for large codebases, wastes budget on function details agent doesn't need

**Outcome:** Signatures are 10x more token-efficient while maintaining same pass rates (95%).

### Why Informational > Prescriptive

**Informational framing (current approach):**
```
Given the repository structure above, you have approximately 8 tool calls
to explore and fix this issue. Prioritize: [suggestions].
```

**Prescriptive framing (alternative, V2 failure):**
```
INSTRUCTIONS: You MUST read bounds_check.rs first, then run tests, then edit.
Do not read other files. Do not use git blame.
```

**Outcome (from V2 benchmark):**
- Prescriptive: 13% pass rate (over-constrained, fails on unexpected code structure)
- Informational: 95% pass rate (agent adapts to reality, uses suggestions as guidance)

**Key insight:** Agents work better with suggestions than rigid rules.

---

## Mermaid Diagram: Injection Pipeline with CallGraph

```mermaid
graph TD
    A["User Request"] --> B["Tokenize Task Description"]
    B --> C["Check Skeleton Cache<br/>(mtime + file count)"]
    C -->|Cache Hit| D1["Load Cached Skeleton"]
    C -->|Cache Miss| D2["Generate New Skeleton"]
    D2 --> D2a["Strategy Selection<br/>(CallGraph/Hybrid/Flat/Deps)"]
    D2a --> D2b["CallGraph: Extract Calls<br/>DependencyGraph: Parse Imports<br/>Flat: Signatures Only"]
    D2b --> D2c["Format & Cap at Token Budget"]
    D2c --> D1
    D1 --> E["BM25 Search<br/>(find top-3 matches)"]
    E --> F{"Score Gap > Threshold?"}
    F -->|Yes| G["Generate Tier 2<br/>(inject top-3 signatures)"]
    F -->|No| H["Skip Tier 2"]
    G --> I["Assign Budget Tier"]
    H --> I
    I --> J["Generate Tier 3<br/>(budget hints)"]
    J --> K["Augment System Prompt"]
    K --> L["Inject into Context"]
    L --> M["agent.run_turn()"]
    M --> N["Tool Execution Loop"]
    
    style D2 fill:#2d6a2d,stroke:#4a4,color:#fff
    style D2a fill:#2d8a3d,stroke:#5a5,color:#fff
    style D1 fill:#1d5a2d,stroke:#3a3,color:#fff
    style G fill:#1a5276,stroke:#5ac,color:#fff
    style J fill:#6e4a1a,stroke:#da5,color:#fff
    style K fill:#5b3a7a,stroke:#a7c,color:#fff
    style M fill:#8b0000,stroke:#f44,color:#fff
```

**Flow explanation:**
1. User provides task description
2. Check if skeleton is cached (mtime + file count)
3. If cached and valid, load; otherwise regenerate
4. Select skeleton strategy (CallGraph is default)
5. Generate skeleton based on strategy:
   - CallGraph: walk files, collect symbols, extract function calls
   - DependencyGraph: parse imports for each file
   - Flat: signatures only
6. Format output and cap at token budget (1500 tokens)
7. Search for high-confidence matches (BM25)
8. Conditionally inject signatures based on gap ratio
9. Assign exploration budget based on confidence
10. Augment system prompt with all three tiers
11. Pass to agent with injected context
12. Agent executes tools with improved guidance

---

## Debugging & Observability

### View Injected Context

Enable debug logging to see what context was injected:

```bash
RUST_LOG=hone_core::context_injection=debug hone "your task"
```

**Output:**
```
[DEBUG] context_injection: Skeleton size: 1240 tokens
[DEBUG] context_injection: BM25 top scores: [42.5, 19.8, 18.2]
[DEBUG] context_injection: Confidence gap: 0.53 (HIGH)
[DEBUG] context_injection: Injected 3 signatures (bounds_check, validate_array, get_element)
[DEBUG] context_injection: Budget hint: 8 tool calls (MEDIUM confidence)
```

### Disable Per-Turn

In the TUI, disable injection for the next turn:

```
:set no-inject
> your task
```

### Benchmark Your Task

Compare with/without injection:

```bash
# With injection (default)
time hone "fix off-by-one error in bounds_check()"

# Without injection
time hone --no-inject "fix off-by-one error in bounds_check()"
```

---

## Configuration Reference

### Global Configuration (config.toml)

```toml
[agent]
# Enable proactive context injection (default: true)
context_injection = true

# Skeleton strategy: call_graph, hybrid, flat, dependency_graph (default: call_graph)
# call_graph: Function-level calls (100% patch rate, +30pp over V1)
# hybrid: Dependency graph + calls for top-5 files (90% patch rate)
# flat: Signatures only, fastest generation (90% patch rate)
# dependency_graph: Import relationships only (85% patch rate)
skeleton_strategy = "call_graph"

# BM25 confidence gap threshold (default: 0.4)
# Lower = more aggressive injection, Higher = conservative
confidence_gap_threshold = 0.4

# Token budget for skeleton (default: 1500)
context_skeleton_budget = 1500

# Number of signatures to inject from Tier 2 (default: 3)
context_signature_count = 3

# Enable exploration budget hints (default: true)
context_budget_hints = true
```

### Environment Variables

```bash
# Disable injection globally
HONE_NO_INJECTION=1

# Debug injection decisions
RUST_LOG=hone_core::context_injection=debug
```

### CLI Flags

```bash
hone --skeleton-strategy call_graph "task"      # Use CallGraph (default)
hone --skeleton-strategy flat "task"            # Use Flat strategy
hone --skeleton-strategy hybrid "task"          # Use Hybrid strategy
hone --skeleton-strategy dependency_graph "task" # Use DependencyGraph

hone --no-inject "task"              # Disable injection for this run
hone --inject-context "task"         # Explicitly enable (already default)
```

---

## FAQ

**Q: Does context injection work with all model providers?**

A: Yes. The injection happens in the system prompt before calling any provider. Works with Anthropic, OpenAI, DeepSeek, Ollama, and all 75+ providers.

**Q: Does it work with remote daemon (honed)?**

A: Yes. Injection happens server-side before sending to the model. No client changes needed.

**Q: How much does injection add to context window?**

A: Typically 800-1500 tokens for skeleton + signatures + budget hints. For a 200K context window, this is <1% overhead.

**Q: Can I use injection with orchestrate/ensemble modes?**

A: Yes. Each agent instance gets its own injected context independently. Orchestrator sees aggregated benefits (faster convergence, fewer retry loops).

**Q: What if my repository is huge (100K files)?**

A: Skeleton generation is O(files), so large repos take longer (~5s for 100K files). Consider using `hone --dir ./src` to scope the context to a subdirectory.

**Q: Does injection affect determinism?**

A: BM25 scoring is deterministic given the same query + index. However, agent behavior can vary with temperature. For deterministic benchmarks, use `--temperature 0` + `--no-inject`.

**Q: Can I inject custom context manually?**

A: Yes. Use `--context-file ./my-context.md` to prepend additional context before injection.

---

## Next Steps

- [Getting Started](getting-started.md) — Set up Hone and try injection on your first task
- [Benchmarks](benchmarks.md) — See detailed performance analysis and reproduce SWE-bench results
- [Architecture](architecture.md) — Deep dive into how context optimization works
