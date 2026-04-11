# Feature Comparison: Hone vs Competitors

Hone is one of several open-source AI coding agents. This page provides an honest, detailed comparison with other tools to help you choose the right agent for your needs.

---

## Overview Table

Detailed feature matrix across six major coding agents:

| Feature Category | Hone | Goose | Claude Code | Cursor | Codex CLI | Aider |
|---|:---:|:---:|:---:|:---:|:---:|:---:|
| **CORE AGENT** |
| Tool calling | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ |
| Streaming output | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ |
| Error recovery & retry | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ |
| Context compaction | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ |
| Budget enforcement | ✓ | ~ | ✓ | ~ | ~ | ~ |
| Phase detection (code/QA) | ✓ | ✗ | ✗ | ✗ | ✗ | ✗ |
| Complexity routing | ✓ | ✗ | ✗ | ✗ | ✗ | ✗ |
| Mid-session provider swap | ✓ | ✗ | ✗ | ✗ | ✗ | ✗ |
| **MULTI-AGENT** |
| Subagent spawning | ✓ | ~ | ✗ | ✗ | ✗ | ✗ |
| Dependency DAG | ✓ | ✗ | ✗ | ✗ | ✗ | ✗ |
| Ensemble (N candidates) | ✓ | ✗ | ✗ | ✗ | ✗ | ✗ |
| Reviewer agent pattern | ✓ | ✗ | ✗ | ✗ | ✗ | ✗ |
| Self-refine loop | ✓ | ✗ | ✗ | ✗ | ✗ | ✗ |
| Test-verify loop | ✓ | ✗ | ✗ | ✗ | ✗ | ✗ |
| **SECURITY** |
| Kernel sandbox | ✓ | ✗ | ✗ | ✗ | ✗ | ✗ |
| Memory encryption | ✓ | ✗ | ✗ | ✗ | ✗ | ✗ |
| Command validation | ✓ | ✗ | ✗ | ✗ | ✗ | ✗ |
| Prompt injection detection | ✗ | ✓ | ✗ | ✗ | ✗ | ✗ |
| Egress audit log | ✗ | ✓ | ✗ | ✗ | ✗ | ✗ |
| RBAC / team roles | ✓ | ✗ | ✗ | ✗ | ✗ | ✗ |
| **PROVIDERS** |
| Anthropic (Claude) | ✓ | ✓ | ✓ | ✓ | ✗ | ✓ |
| OpenAI | ✓ | ✓ | ✗ | ✓ | ✓ | ✓ |
| Google Gemini | ~ | ✓ | ✗ | ✗ | ✗ | ✓ |
| AWS Bedrock | ✗ | ✓ | ✗ | ✗ | ✗ | ✗ |
| Azure OpenAI | ✓ | ✓ | ✗ | ✓ | ✗ | ✗ |
| Ollama (local) | ✓ | ✓ | ✗ | ✗ | ✗ | ✓ |
| OpenRouter | ✓ | ✓ | ✗ | ✗ | ✗ | ✗ |
| Custom endpoints | ✓ | ✓ | ✗ | ✗ | ✗ | ✗ |
| Provider count | 5+ | 30+ | 1 | ~3 | 1 | 6+ |
| **MCP / EXTENSIONS** |
| MCP client | ✓ | ✓ | ✗ | ✗ | ✗ | ✗ |
| Community extensions | ✗ | 70+ | ✗ | ✗ | ✗ | ✗ |
| Extension marketplace | ✗ | ✓ | ✗ | ✗ | ✗ | ✗ |
| **INTERFACES** |
| Terminal TUI | ✓ | ✓ | ✗ | ✗ | ✓ | ✓ |
| Desktop GUI | ✗ | ✓ | ✗ | ✓ | ✗ | ✗ |
| Web UI | ✗ | ✗ | ✓ | ✓ | ✗ | ✗ |
| VS Code integration | ✗ | ✗ | ✗ | ✓ | ✗ | ✓ |
| Remote daemon | ✓ | ~ | ✗ | ✓ | ✗ | ✗ |
| **SESSION MANAGEMENT** |
| Named sessions | ~ | ✓ | ✓ | ✓ | ✗ | ✗ |
| Resume sessions | ~ | ✓ | ✓ | ✓ | ✗ | ✗ |
| Session export | ✗ | ✓ | ✓ | ✓ | ✗ | ✗ |
| Multi-user sessions | ✓ | ✗ | ✗ | ~ | ✗ | ✗ |
| **TOOLS & CODE UNDERSTANDING** |
| File I/O tools | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ |
| Shell execution | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ |
| Git tools | ✓ | ~ | ✓ | ✓ | ~ | ✓ |
| LSP integration | ✓ | ~ | ✓ | ✓ | ✗ | ✗ |
| Semantic indexing | ✓ | ✗ | ✗ | ✓ | ✗ | ✗ |
| Tree-sitter parsing | ✓ | ✗ | ✗ | ? | ✗ | ✗ |
| Symbol search | ✓ | ✗ | ✗ | ✓ | ✗ | ✗ |
| Screen capture | ✗ | ✓ | ✗ | ✓ | ✗ | ✗ |
| **AUTOMATION & RECIPES** |
| YAML recipes | ✓ | ✓ | ✗ | ✗ | ✗ | ✗ |
| Template variables | ✗ | ✓ | ✗ | ✗ | ✗ | ✗ |
| Built-in recipes | ✓ | ✗ | ✓ | ✓ | ✗ | ✗ |
| Scheduled tasks (cron) | ✗ | ✓ | ✗ | ✗ | ✗ | ✗ |
| **PERFORMANCE** |
| Binary size | 12 MB | 15 MB | ~150 MB | ~450 MB | ~100 MB | ~40 MB |
| Startup time (warm) | <10 ms | ~200 ms | ~100 ms | ~2s | ~50 ms | ~100 ms |
| Memory footprint | ~8 MB | ~50 MB | ~80 MB | ~500 MB | ~30 MB | ~20 MB |
| **BENCHMARKS** |
| SWE-bench Lite | 19.3% | ? | ? | ? | ? | 28% (aider-gpt4) |
| Cost per task | $0.03 | ? | ? | ? | ? | ~$1.00 |
| Published benchmarks | ✓ | ✗ | ✗ | ✗ | ✗ | ✓ |
| **DEPLOYMENT** |
| Open source | ✓ | ✓ | ✗ | ✗ | ✗ | ✓ |
| Self-hosted | ✓ | ✓ | ✗ | ✗ | ✗ | ✓ |
| Free tier | ✓ | ✓ | Free (limited) | Paid only | Paid only | ✓ |
| Commercial support | ~ | ✓ | ✓ | ✓ | ✓ | ~ |

**Legend:** ✓ = full support | ~ = partial | ✗ = not available | ? = unknown

---

## Detailed Competitor Analysis

### Hone

**What Hone is:** A privacy-first, terminal-native AI coding agent built entirely in Rust. Emphasizes security (kernel sandboxing, encrypted memory), multi-agent sophistication (orchestration with dependency DAG, ensemble, reviewer patterns), and cost efficiency.

**Core strengths:**
- **Kernel-enforced sandboxing** — macOS Seatbelt, Linux Landlock. Commands fail closed, not silently degraded.
- **Multi-agent orchestration** — Decompose complex tasks into dependency-aware subtasks. MASAI Ranker ensemble. Reviewer and self-refine patterns.
- **Encrypted memory** — AES-256-GCM + Argon2id. Session patterns are protected at rest.
- **Context optimization** — BM25 retrieval, graduated compression, phase detection, model-aware budgets. Lower token usage than competitors.
- **Lean codebase** — 37K LOC, 15 crates. Fast startup (<10ms), low memory (~8MB).
- **LSP integration** — Real type information via rust-analyzer, pyright, typescript-language-server, gopls. Not pattern-matching.
- **Cost efficiency** — 19.3% on SWE-bench Lite at ~$0.03/task using DeepSeek V3 (50-100x cheaper than Claude Code or Cursor).

**Gaps:**
- No desktop GUI (terminal only).
- Smaller extension ecosystem (supports MCP but lacks the marketplace of 70+ extensions that Goose has).
- Named session resumption not yet implemented (in-memory sessions only; daemon can persist while running).
- No prompt injection detection UI (kernel sandbox provides hard boundary but no audit trail).

**Best for:**
- Developers who value privacy and security over GUI polish.
- Teams that need multi-user sessions, RBAC, and auditability.
- Cost-conscious deployments; self-hosted infrastructure.
- Rust/Python/TypeScript specialists who want deep language tooling (LSP, tree-sitter).

**Pricing:** Open source, MIT or Apache 2.0. Free to self-host.

---

### Goose (AAIF / Linux Foundation)

**What Goose is:** A Rust+Electron agent backed by Block (Square/Cash) and transferred to the Agentic AI Foundation. Emphasizes ecosystem breadth (70+ MCP extensions) and desktop accessibility.

**Core strengths:**
- **Extension marketplace** — 70+ community MCP servers. Auto-discovery, enable/disable at runtime. This is Goose's primary competitive moat.
- **Desktop app** — Electron-based GUI for macOS, Linux, Windows. Accessible to non-terminal users.
- **Provider breadth** — 30+ providers including Anthropic, OpenAI, Google, Ollama, AWS Bedrock, Azure, Databricks, Snowflake, xAI.
- **Mature recipe system** — YAML recipes with template variables, sub-recipe composition, GitHub import.
- **Session management** — Named sessions, resume, list, export to JSON/YAML/Markdown.
- **Telemetry** — Opt-in telemetry for usage insights (vs Hone's no-telemetry stance).
- **Prompt injection detection** — ML-based scanner for adversarial content in tool outputs.

**Gaps:**
- No kernel-level sandboxing (relies on application-level permission prompts).
- No multi-agent orchestration or ensemble (subagents run flat and independently).
- No encrypted memory (plaintext file-based storage).
- No semantic code indexing or tree-sitter integration.
- No RBAC or team features.
- No published cost-per-task benchmarks.

**Best for:**
- Teams that want a rich desktop experience and broad provider support.
- Developers who rely on third-party integrations (GitHub, Jira, Slack) via MCP extensions.
- Organizations with Linux Foundation governance preference.
- Non-terminal users or teams with mixed technical backgrounds.

**Pricing:** Open source (MIT). Desktop and server components may have commercial offerings.

**Community:** ~20K GitHub stars, funded, active releases.

---

### Claude Code (Anthropic)

**What Claude Code is:** Anthropic's official CLI tool for Claude. TypeScript-based. Tightly integrated with Anthropic's ecosystem.

**Core strengths:**
- **Anthropic native** — Built by Anthropic, uses Claude exclusively. Optimized for Claude's capabilities.
- **Web interface** — Browser-based chat; easy onboarding for non-technical users.
- **Session management** — Named sessions, history, resume.
- **Official support** — Backed by Anthropic with official documentation and support.

**Gaps:**
- **Vendor lock-in** — Claude only. Cannot swap to GPT-4, DeepSeek, local models.
- No multi-agent orchestration.
- No sandboxing.
- No team features or RBAC.
- No published benchmarks on standard datasets.
- Limited tool ecosystem (no MCP).

**Best for:**
- Organizations standardized on Anthropic / Claude.
- Users who want a simple, officially-supported Claude chat interface.
- Budget-conscious deployments (Claude is cost-competitive but locked to one vendor).

**Pricing:** Paid (Anthropic subscription based on token usage). Typically $0.10-$1.00 per complex task.

---

### Cursor

**What Cursor is:** A VS Code fork with integrated AI, designed for IDE-native development. Cloud-based with optional self-hosted option.

**Core strengths:**
- **IDE integration** — Deep VS Code integration. Inline suggestions, refactoring, preview diffs side-by-side.
- **Multi-provider support** — Claude, GPT-4, local models.
- **GUI polish** — Designed by experienced developers. Smooth UX.
- **Large community** — ~10K+ stars, well-funded.

**Gaps:**
- **Cloud dependency** — Primarily cloud-based; self-hosted option is limited.
- Not a terminal agent (requires VS Code).
- No multi-agent orchestration.
- No sandboxing.
- No semantic indexing.
- Proprietary (not open source).

**Best for:**
- Developers who already use VS Code and want AI co-editing.
- Teams comfortable with cloud-based AI infrastructure.
- Visual developers who prefer GUI interaction over terminal.

**Pricing:** Paid subscription (~$20/month). Per-token rates vary by provider.

---

### Codex CLI (OpenAI)

**What Codex CLI is:** OpenAI's TypeScript-based CLI for Codex/GPT-3.5/GPT-4. Minimal, command-line focused.

**Core strengths:**
- **OpenAI native** — Direct access to GPT-4, latest models.
- **Lightweight** — Small binary, no complex dependencies.

**Gaps:**
- **Vendor lock-in** — OpenAI only.
- No session management.
- No GUI.
- No MCP or extensions.
- No multi-agent features.
- No sandboxing.
- Minimal documentation and community support.

**Best for:**
- OpenAI-only deployments.
- Scripts and CI/CD pipelines.
- Cost-sensitive workloads using cheaper models.

**Pricing:** Paid (OpenAI API tokens). Typically $0.10-$1.00 per task depending on model.

---

### Aider

**What Aider is:** A Python-based AI pair programmer focused on edit workflows. Multi-provider support, vim-like editor integration.

**Core strengths:**
- **Edit-focused** — Specializes in code edits, diffs, and refactoring. Excellent at applying changes cleanly.
- **Multi-provider** — Supports Anthropic, OpenAI, Ollama, Gemini, Groq, Azure, etc.
- **Published benchmarks** — Aider-GPT4 achieves 28% on SWE-bench Lite.
- **Vim integration** — Modal editor with vim keybindings.
- **Active development** — Well-maintained, responsive community.

**Gaps:**
- **Token expensive** — SWE-bench Lite at 28% pass rate costs ~$1.00/task (aider-gpt4), vs Hone at $0.03.
- No kernel sandboxing.
- No multi-agent orchestration.
- No semantic indexing.
- No team features.

**Best for:**
- Developers who want a pure edit-focused workflow.
- Teams using various LLM providers.
- Projects where edit quality matters more than task complexity.

**Pricing:** Open source (MIT). Uses provider tokens (OpenAI ~$0.50-$2.00/task).

---

## Cost Comparison

Rough cost per 20-turn task on SWE-bench Lite (actual costs vary by model choice):

| Agent | Model | Pass Rate | Cost/Task |
|---|---|---|---|
| **Hone** | DeepSeek V3 (OpenRouter) | 19.3% | $0.03 |
| **Aider** | GPT-4 | 28% | $1.00 |
| **Claude Code** | Claude 3.5 Sonnet | ? | $0.50-$1.00 |
| **Cursor** | GPT-4 | ? | $0.50-$1.00 |
| **Goose** | (no published benchmark) | ? | ? |
| **Codex CLI** | GPT-4 | ? | $0.50-$1.00 |

**Key insight:** Hone achieves competitive accuracy (19.3%) at radically lower cost ($0.03 vs $0.50-$1.00) by leveraging cheaper but capable models (DeepSeek V3, Qwen) and aggressive context optimization. Aider's 28% is higher, but at 33x the cost per task.

---

## Architecture Comparison

### Technology Stack

| Dimension | Hone | Goose | Claude Code | Cursor | Codex CLI | Aider |
|---|---|---|---|---|---|---|
| Language | Rust (100%) | Rust (58%) + TS (34%) | TypeScript | Electron + Python | TypeScript | Python |
| Desktop GUI | ✗ | Electron | Web | Electron (VS Code) | ✗ | ✗ |
| Crate/Module count | 15 | 9 | ~20 | ~30 | ~15 | ~10 |
| Dependencies | ~100 | ~150 | ~200+ | ~500+ | ~100 | ~150 |

### Execution Model

**Hone:**
- Agent loop → native Layer 2 tools (file I/O, shell, git) → MCP client (Layer 1) → 75+ provider adapters
- Kernel sandbox wraps all tool execution (Seatbelt/Landlock)
- Daemon mode for remote/team access

**Goose:**
- Agent loop → 30+ provider adapters → extension manager with 70+ MCP servers
- Application-level permission prompts (no kernel sandbox)
- Desktop app or CLI

**Claude Code:**
- Chat loop → Anthropic API (Claude only)
- Minimal tool ecosystem
- Web interface

**Cursor:**
- VS Code agent → Claude/GPT-4 → inline edits
- IDE-native execution
- Cloud or self-hosted backend

**Codex CLI:**
- Simple CLI loop → OpenAI API (GPT-3.5/GPT-4 only)
- Stateless (no sessions)

**Aider:**
- REPL editor → Multi-provider API calls
- Edit-focused tool pipeline
- Vim-like modal editor

---

## When to Choose What

### Choose Hone if you:
- Value privacy and security. Kernel sandboxing + encrypted memory.
- Need multi-agent orchestration. Decompose complex tasks, use ensemble voting, reviewer patterns.
- Work in cost-conscious environments. 50-100x cheaper per task than competitors.
- Deploy to teams. Multi-user sessions, RBAC, audit logging.
- Use multiple LLM providers. 75+ adapters, mid-session provider swap.
- Prefer terminal-native workflows.
- Want open-source, self-hosted infrastructure.

**Trade-off:** No desktop GUI, smaller extension ecosystem, younger project.

---

### Choose Goose if you:
- Want a desktop app with a rich UI.
- Need broad provider support (30+ services including Bedrock, Databricks, Snowflake).
- Rely on third-party integrations. 70+ MCP extensions (GitHub, Slack, Jira, etc.).
- Prefer Linux Foundation governance and ecosystem.
- Need mature session management (resume, export, search).
- Have non-terminal users on the team.

**Trade-off:** No sandboxing, no multi-agent features, steeper memory footprint.

---

### Choose Claude Code if you:
- Use Claude exclusively. It's optimized for Anthropic's models.
- Want official Anthropic support.
- Prefer web-based chat interface.
- Have a straightforward single-agent workflow.

**Trade-off:** Vendor lock-in, no sandboxing, no team features, limited tools.

---

### Choose Cursor if you:
- Already use VS Code and want AI co-editing.
- Prefer visual, IDE-native workflows over terminal.
- Want commercial support from a well-funded company.
- Like side-by-side diff preview and inline refactoring.

**Trade-off:** Proprietary (not open source), cloud-dependent, no sandboxing, not a pure agent.

---

### Choose Codex CLI if you:
- Use OpenAI exclusively.
- Need minimal, lightweight tooling for scripts or CI/CD.
- Don't need session management or complex workflows.

**Trade-off:** Vendor lock-in, no features, minimal community, high per-task cost.

---

### Choose Aider if you:
- Focus on code edits and diffs.
- Want a proven, edit-centric workflow.
- Published benchmarks matter to you (28% on SWE-bench).
- Prefer multi-provider support.

**Trade-off:** High cost per task ($1.00 vs $0.03), no sandboxing, no team features.

---

## Feature Depth: Hone's Unique Capabilities

These are areas where Hone has genuine technical depth that competitors would need significant effort to match:

### 1. Multi-Agent Orchestration with Dependency DAG
Only Hone implements full dependency-aware task graphs. A lead agent decomposes requests into subtasks with `depends_on` relationships, scope restrictions, and acceptance criteria. Workers run in parallel batches respecting the DAG, with per-worker and total budget enforcement. This enables sophisticated decomposition patterns (MapReduce, hierarchical refinement) that flat subagent systems cannot support.

### 2. Kernel-Enforced Sandboxing
Hone uses macOS Seatbelt profiles and Linux Landlock rulesets at the kernel level. Shell commands are validated before execution (Allow/Block/RequireApproval classification). Sandbox failure is fail-closed — the operation aborts, never silently degrades. This is a genuine security advantage over application-level permission prompts.

### 3. Encrypted Memory
Hone's memory store uses AES-256-GCM encryption with Argon2id key derivation. Keys are stored in the system keychain. Session patterns and learned behaviors are protected at rest. Competitors store memory as plaintext or not at all.

### 4. Context Optimization Sophistication
Hone's `ContextManager` implements model-aware token budgets, BM25 message reordering (relevance stays in context), graduated compression (summarize before evict), phase detection (coding vs QA vs planning), and learned compression patterns. This results in 71% lower prompt tokens while maintaining quality parity.

### 5. LSP Integration Depth
Hone bridges stdio JSON-RPC to rust-analyzer, pyright, typescript-language-server, and gopls. Tools include `lsp_diagnostics`, `lsp_hover`, and `lsp_code_actions`. The agent queries real type information and compiler diagnostics, not pattern-matched file text. This is particularly powerful for Rust and Python workflows.

### 6. Semantic Code Index with Tree-Sitter
`hone-index` uses tree-sitter to parse ASTs for multiple languages, stores symbol embeddings in SQLite vectors, and updates incrementally via a file watcher. Symbol search runs in under 1 microsecond in-memory. This gives the agent structural code understanding that grep-based tools cannot provide.

### 7. Cost Efficiency at Scale
Published SWE-bench results demonstrate Hone achieves 19.3% pass rate at $0.03/task using DeepSeek V3. This is 50-100x cheaper than Claude Code or Cursor, while beating SWE-Agent (GPT-4 at $0.10-$0.20/task) at similar accuracy. The cost advantage compounds over thousands of tasks.

---

## Feature Gaps: What Hone is Missing

Honest gaps where competitors are better:

### 1. Desktop GUI
Goose's Electron app is a major adoption driver for non-terminal users. Hone is terminal-only. A web-based TUI or lightweight desktop wrapper would lower the barrier.

### 2. Extension Ecosystem Scale
Goose has 70+ community MCP extensions. Hone supports MCP but lacks the marketplace and community volume. Building ecosystem effects takes time and funding.

### 3. Named Session Resumption
Goose and Claude Code have mature session resume (list, resume by name, delete). Hone has in-memory sessions and can persist via daemon, but no `hone session resume` command yet. This is a planned P1 feature.

### 4. Provider Breadth
Goose supports 30+ providers (including Bedrock, Databricks, Snowflake, xAI). Hone covers the essentials (Anthropic, OpenAI, Ollama) and OpenAI-compatible endpoints but misses enterprise providers.

### 5. Prompt Injection Detection UI
Goose has ML-based scanning of tool outputs for adversarial patterns. Hone's kernel sandbox provides hard boundary, but no visible audit trail. This is a planned P2 feature.

### 6. Maturity
Hone is younger (2025) than Goose (2024) or Claude Code (2023). The codebase is mature and tested, but the feature set is still evolving.

---

## Competitive Matrix Summary

```
Dimension                       | Hone's Position
--------------------------------|---------------------------
Privacy & Security              | Best-in-class (sandbox, encryption)
Cost Efficiency                  | Best-in-class ($0.03/task)
Multi-Agent Sophistication       | Best-in-class (DAG orchestration)
Code Understanding (LSP/AST)     | Best-in-class (real type info)
Desktop/GUI Polish               | Weakest (terminal only)
Extension Ecosystem              | Weakest (no marketplace)
Provider Breadth                 | Middle (5+ vs 30+ Goose)
Session Management               | Catching up (resume planned)
Benchmarks / Validation          | Strong (published SWE-bench)
Community Size                   | Smaller (but growing)
```

---

## Migration Guides

- [From Goose to Hone](../onboarding/from-goose.md)
- [From Claude Code to Hone](../onboarding/from-claude-code.md)
- [From Aider to Hone](../onboarding/from-aider.md)
- [From Codex CLI to Hone](../onboarding/from-codex-cli.md)
- [From OpenCode to Hone](../onboarding/from-opencode.md)

---

## Further Reading

- [Architecture: Why Hone is Built the Way It Is](architecture.md)
- [Security: Sandboxing, Encryption, and RBAC](tutorials/security.md)
- [Benchmarks: SWE-bench Results and Cost Analysis](benchmarks.md)
- [Multi-Agent Orchestration Patterns](tutorials/tools-and-mcp.md#multi-agent-orchestration)

---

*Last updated: April 2026. Comparison based on public source code and documentation. If you spot an inaccuracy, please open an issue or PR.*
