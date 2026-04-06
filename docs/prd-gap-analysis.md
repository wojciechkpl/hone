# Hone PRD Gap Analysis

**Date:** 2026-04-05
**Codebase snapshot:** commit 9082a32 (Initial commit), branch `main`
**Total test count:** 403 passing, 0 failing

---

## 1. Executive Summary

The Hone codebase is approximately **25-30% complete** against the PRD. The workspace structure is well-organized across 12 crates and 2 binaries, and foundational types, traits, and module boundaries are in place. However, the project is largely a **scaffolding pass**: most subsystems have trait definitions and in-memory/mock implementations but lack real integration with external systems. The CLI can stream text from Claude CLI or Gemini CLI through a functional TUI, but it cannot execute tools, run recipes, enforce sandboxing, connect to LSP servers, or persist memory. Of the 20 PRD requirements, zero are fully complete, roughly 8 are partial/stub, and 12 are missing or purely planned. The project is pre-alpha and not usable as a daily driver.

---

## 2. Requirements Scorecard

| Req | Description | PRD Priority | Status | Score | Notes |
|-----|------------|-------------|--------|-------|-------|
| R1 | 75+ LLM providers | P0 | 5 adapters exist (Claude CLI, Gemini CLI, Mock, + Anthropic/OpenAI/Ollama as `todo!()` stubs) | **STUB** | Only CLI pass-through providers work. Anthropic HTTP, OpenAI HTTP, and Ollama adapters are `todo!()`. No Bedrock, Azure, vLLM, llama.cpp, OpenRouter, or custom OpenAI-compatible. Router and fallback chain types exist but are untested with real providers. Actual count: 2 working providers, not 75+. |
| R2 | Kernel-enforced sandboxing (Seatbelt/Landlock) | P0 | Seatbelt profile generation works; `apply()` and `restrict_child()` are stubs that log but do nothing | **STUB** | Profile string is generated correctly. No actual `sandbox_init(3)` or `sandbox-exec` call. Landlock module exists on Linux (not verified). Container sandbox and NullSandbox exist as fallbacks. Fail-closed design is partially implemented (errors on empty profile). |
| R3 | LSP integration with 20+ language servers | P0 | `LspBridge` trait defined. `MockLspBridge` exists. No real LSP JSON-RPC client. | **STUB** | Language detection for 8 languages exists. `AutoFixLoop` logic is implemented against the trait. `LspDiagnosticsTool`, `LspHoverTool`, `LspCodeActionTool` wrappers exist. But there is no code that spawns an actual language server or speaks JSON-RPC. Zero real LSP integration. |
| R4 | Layered tool architecture (Native + MCP + Code Mode + Skills) | P0 | Native tools partially implemented. MCP client is trait + mock. Code Mode runner exists. Skills = HONE.md parser. | **PARTIAL** | **Layer 2 (Native):** File tools (read/write/edit/glob), shell tool, git tools (status/diff/commit/branch/checkout/undo/redo/auto-commit) exist as types. `ToolRegistry` and `ToolDispatcher` route calls. **Layer 1 (MCP):** `McpClient` trait, `MockMcpClient`, `SchemaCache`, `McpTool` wrapper -- all plumbing, no real MCP JSON-RPC transport. **Layer 0 (Code Mode):** `CodeModeRunner` can write a script to a temp file and execute it in a subprocess with sandbox policy. **Layer 3 (Skills):** `HoneMd` parser works. The architecture is sound but untested end-to-end because the main binary does not wire tools into the agent loop. |
| R5 | Client-server architecture with HTTP/SSE API | P0 | `honed` binary runs an Axum server with session CRUD, health, auth middleware, SSE encoder, tunnel routes, governance routes, collaborative routes. | **PARTIAL** | Server starts and serves routes. Session management is in-memory. SSE event encoding works. But there is no actual agent execution on the server side -- sessions are just metadata. No model completion flow through the server. The server is a shell waiting for an agent engine to be plugged in. |
| R6 | Local-first adaptive memory system | P0 | `MemoryStore` trait, `InMemoryStore`, `Memory`/`MemoryQuery`/`MemoryFilter` types, decay logic. | **STUB** | Types and in-memory implementation exist with record/recall/forget/decay/export/import. No SQLite persistence (sqlx is a workspace dep but unused in hone-memory). No integration with the agent loop. No pattern learning. No cross-session persistence. |
| R7 | Rust CLI/TUI with sub-100ms startup and streaming | P0 | TUI is functional with ratatui. Streaming tokens from Claude/Gemini CLI works. Conversation history maintained. Slash commands, themes, animations, syntax highlighting modules exist. | **PARTIAL** | This is the most complete feature. The TUI renders, accepts input, streams responses, and handles multi-turn conversations. However: no tool execution in the loop (tool calls from providers are ignored), no model switching at runtime, no cost display, no file tree panel. Startup time is likely within target given it's a compiled Rust binary. |
| R8 | HONE.md hierarchical project constitution | P0 | `HoneMd` parser in hone-config parses `## Heading` sections from markdown. | **STUB** | Parser works. But there is no hierarchical loading (project + directory + user levels), no injection into agent system prompts, no runtime loading from `.hone/` directories. The parser is an isolated utility with no consumers. |
| R9 | Lightweight web UI | P1 | No web UI code exists. | **MISSING** | No SolidJS, no frontend assets, no PWA manifest. Server routes exist but serve JSON API only. |
| R10 | YAML-based recipe system | P1 | `Recipe` types, YAML parser, `RecipeRunner` with step execution and test commands. 4 bundled recipes (bug-fix, code-review, refactor, tdd-feature). | **PARTIAL** | Recipe parsing and sequential execution logic is implemented. Test command validation between steps works. However, the executor is a generic closure -- no actual integration with the agent/provider system. Recipes are not accessible from the CLI (`--recipe` flag is accepted but ignored). |
| R11 | Real-time cost estimation | P1 | `CostAccumulator` tracks token usage and USD cost. `PricingTable` in hone-providers. Budget checking exists. | **STUB** | Cost accumulation math is correct and tested. But cost is not displayed in the TUI, not shown before actions, and not integrated into the agent loop. The "real-time projection before executing" feature is entirely missing. |
| R12 | Persistent incremental semantic index | P1 | `SemanticIndex` with symbol extraction (regex-based via `SymbolExtractor`), file indexing, search, symbol-at-position. `walk_and_index` for directory traversal. | **PARTIAL** | In-memory index works: can index files, search by name (exact/prefix/substring), find enclosing symbol. But: no tree-sitter integration (despite dep), no embeddings, no persistence to disk, no incremental updates via file watcher (notify dep unused), no call graph analysis. It's a basic symbol table, not a semantic index. |
| R13 | Multi-agent orchestration (lead-worker model) | P1 | `AgentRegistry` with 12 bundled agent definitions (markdown-based). `AgentDef` parser. | **STUB** | Agent definitions are loaded and queryable. But there is no orchestration engine, no lead-worker coordination, no parallel subagent execution. The `Agent` type in hone-core exists but is not wired into a multi-agent system. |
| R14 | Git-first workflow with auto-commits | P1 | Git tools: status, diff, commit, branch, checkout, undo, redo. `AutoCommitter` exists. `UndoStack` for tracking. | **PARTIAL** | Git tool types exist with `git2` integration. Auto-committer concept is present. But none of these tools are invoked during the agent loop -- the TUI ignores tool calls. The undo/redo stack is an interesting design but has no UI surface. |
| R15 | Extension marketplace | P2 | No marketplace code. | **MISSING** | No discovery, installation, or registry for MCP servers or recipes. |
| R16 | Team governance: spend caps, model policies, audit logs | P2 | `GovernanceStore` with members, policies (allowed models, spend caps, approved extensions), usage tracking. Governance HTTP routes. `AuditLog` trait with `InMemoryAuditLog`. RBAC with roles and tool access control. | **PARTIAL** | Types and in-memory stores exist. HTTP routes expose CRUD for governance data. Audit log captures entries. RBAC defines roles (Admin, Developer, Viewer) with tool access. But all storage is in-memory (lost on restart), no SSO/SCIM, no real enforcement in the agent loop. |
| R17 | Cross-repository intelligence | P2 | `MultiRepoIndex` can index multiple repos and search across them. | **STUB** | The multi-repo wrapper over `SemanticIndex` exists and works for basic symbol search. But without real semantic analysis, embeddings, or call graphs, this is just multi-directory grep. No impact analysis across repos. |
| R18 | Test-driven editing mode | P2 | `tdd-workflow.md` agent definition bundled. `tdd-feature.yaml` recipe bundled. | **STUB** | An agent definition and recipe YAML exist describing the workflow. No engine to actually iterate code until tests pass. The recipe runner could theoretically drive this but is not connected to anything. |
| R19 | Voice-to-code (local Whisper) | P2 | No voice/audio code. | **MISSING** | No Whisper integration, no audio input handling. |
| R20 | Collaborative multi-user sessions | P2 | `CollaborativeStore` with participants, join/leave, roles (Owner/Editor/Viewer). HTTP routes. | **STUB** | Participant tracking exists in-memory. No real-time sync, no cursor sharing, no conflict resolution, no WebSocket connections. It's a participant roster, not a collaboration system. |

---

## 3. Competitive Feature Matrix Gap

The PRD's competitive matrix (Section 20) makes many claims. Here is the reality check:

| Claimed Advantage | PRD Claim | Actual Status | Honest Assessment |
|---|---|---|---|
| Model providers: 75+ | Hone supports 75+ providers | 2 working (CLI pass-through), 3 `todo!()` stubs | **FALSE.** Would need to say "2 providers" today. |
| Local models: Full | Full local model support | No working local model adapter (Ollama is `todo!()`) | **FALSE.** Zero local model support. |
| Kernel sandbox: Yes | Kernel-enforced sandboxing | Profile generation only; no enforcement | **FALSE.** Sandbox does not enforce anything. |
| LSP integration: 20+ langs | 20+ language LSP integration | Trait + mock only; 0 real LSP connections | **FALSE.** No LSP integration exists. |
| Tool architecture: Layered | Native + MCP + Code Mode | Types exist but agent loop ignores tool calls | **MISLEADING.** Architecture is designed, not operational. |
| Adaptive memory: Local-first | Local-first adaptive memory | In-memory only, no persistence, no learning | **FALSE.** Memory is volatile and non-adaptive. |
| Remote/mobile: PWA + Web | PWA + Web UI | No web UI of any kind | **FALSE.** |
| Client-server: Yes | Full client-server | Server runs but has no agent engine | **MISLEADING.** Server is a session shell. |
| Cost estimation: Real-time | Real-time cost estimation | CostAccumulator math works, not displayed | **FALSE.** Not user-visible. |
| Recipes/workflows: YAML | YAML recipe system | Parser + runner exist, not connected to CLI | **MISLEADING.** Cannot be used in practice. |
| Semantic index: Persistent | Persistent semantic index | In-memory regex symbol search, no persistence | **FALSE.** Not persistent, not semantic (no embeddings). |
| Git-first: Auto-commit | Automatic commit per edit | Tools exist, not invoked during agent execution | **FALSE.** No auto-committing happens. |
| Multi-agent: Lead-worker | Lead-worker orchestration | Agent registry only, no orchestration | **FALSE.** |
| Voice input: Local Whisper | Local Whisper transcription | No audio code | **FALSE.** |
| Test-driven mode: First-class | First-class TDD mode | Agent def + recipe YAML only | **FALSE.** |
| Startup time: < 100ms | Sub-100ms startup | Likely achievable (compiled Rust, minimal deps) | **PLAUSIBLE** but unmeasured. |
| Memory footprint: < 50MB | Under 50MB idle | Likely achievable | **PLAUSIBLE** but unmeasured. |

**Bottom line:** Of 17 competitive claims, approximately 2 are plausible, 3 are misleading (structure exists but doesn't function), and 12 are outright false given current implementation.

---

## 4. Performance Requirements Gap

| Metric | Target | Current Reality | Gap |
|---|---|---|---|
| CLI startup time | < 100ms | Likely met (compiled binary, no heavy init) | **Low risk** -- needs benchmarking to confirm |
| First token latency (local) | < 500ms | No local model support at all | **Blocked** -- Ollama adapter is `todo!()` |
| First token latency (cloud) | < 1.5s | Depends on Claude/Gemini CLI subprocess spawn time; likely 2-4s including process start | **At risk** -- subprocess model adds overhead vs direct HTTP |
| Memory footprint (idle) | < 50MB | Likely met (no heavy allocations) | **Low risk** -- needs profiling |
| Memory footprint (active) | < 200MB | Unknown; no real workload possible yet | **Unknown** |
| Semantic index build (50K LOC) | < 30s | Regex-based extraction is fast but untested at scale; no tree-sitter | **Unknown** -- likely met for regex, but feature is too basic |
| Semantic index incremental | < 2s | No file watcher, no incremental pipeline | **Not applicable** -- feature not built |
| LSP diagnostic round-trip | < 500ms | No LSP connection | **Not applicable** -- feature not built |
| Session reconnect | < 1s | No reconnection logic | **Not applicable** -- feature not built |
| Web UI initial load | < 500KB | No web UI | **Not applicable** -- feature not built |

**Summary:** 2 of 10 targets are likely met (startup, idle memory). 1 is at risk (cloud latency due to subprocess model). 7 are not applicable because the features don't exist.

---

## 5. Architecture Gaps

### Present and sound
- **Workspace structure:** Clean crate separation with clear dependency graph
- **Protocol types (hone-proto):** Message, Role, Content, AgentEvent, ToolCallRequest/Result, TokenUsage, PricingEntry -- well-defined
- **Provider trait abstraction:** `ModelProvider` trait with streaming, health checks, token estimation
- **Tool trait abstraction:** `Tool` trait with input/output/context, `ToolRegistry`, `ToolDispatcher`
- **Sandbox trait abstraction:** `Sandbox` trait with `apply()` and `restrict_child()`, multiple backend types
- **Config system:** `figment`-based config loading, `HoneMd` parser, typed config structs

### Missing or critically incomplete

1. **Agent execution loop:** The biggest gap. The TUI streams tokens but completely ignores tool calls (`ProviderEvent::ToolCall(_) => {}`). There is no plan-execute-observe cycle. The agent cannot act on the codebase.

2. **Tool-calling protocol:** The provider adapters (Claude CLI, Gemini CLI) do not support tool calling. The `supports_tools: false` flag is honest. Without tool-calling support from at least one provider, the entire tool architecture is inert.

3. **MCP transport:** The `McpClient` trait exists but there is no JSON-RPC client implementation. No stdio or HTTP+SSE transport for MCP servers. The `SchemaCache` and `McpTool` are plumbing waiting for a transport.

4. **LSP transport:** No JSON-RPC client for LSP. The entire `LspBridge` hierarchy operates on mocks.

5. **Persistence layer:** SQLite dep is declared but unused. Memory, index, sessions, governance, audit -- all in-memory. Nothing survives a process restart.

6. **HONE.md integration:** Parser exists but nothing loads HONE.md from disk, merges layers, or injects it into prompts.

7. **hone.toml routing:** Config types for routing rules exist but are not connected to the `ModelRouter` in the provider crate.

8. **Web frontend:** Entirely absent. No HTML, CSS, JS, SolidJS, or static file serving.

---

## 6. Critical Path to MVP

Ranked by user impact, with effort estimates and dependencies.

| Rank | Item | User Impact | Effort | Dependencies | Description |
|------|------|-------------|--------|-------------|-------------|
| 1 | **Agent execution loop with tool calling** | Blocks all agent functionality | L | Provider with tool support | Implement the plan-execute-observe cycle: model returns tool calls, dispatcher executes them, results fed back. Without this, Hone is just a chat wrapper. |
| 2 | **Anthropic HTTP provider with tool use** | Enables tool calling for the most capable model | M | None | Implement the `todo!()` in `anthropic.rs`. Must support `tools` parameter and `tool_use` content blocks. Claude CLI pass-through cannot do tool calling. |
| 3 | **Wire native tools into agent loop** | Users can edit files, run commands, do git ops | M | #1, #2 | Register file/shell/git tools in the main binary, feed tool schemas to the provider, dispatch calls. |
| 4 | **Sandbox enforcement (Seatbelt apply)** | Users can trust autonomous mode | M | None | Call `sandbox-exec` or `sandbox_init(3)` in `SeatbeltSandbox::restrict_child`. Profile generation already works. |
| 5 | **HONE.md loading and prompt injection** | Users can customize agent behavior per-project | S | #1 | Load HONE.md from project root, merge with user-level, inject relevant sections into system prompt. Parser already works. |
| 6 | **hone.toml config loading** | Users can configure models, routing, sandbox | S | None | Wire `figment`-based config into main binary startup. Types exist in hone-config. |
| 7 | **Cost tracking in TUI** | Users see what they're spending | S | #1, #2 | `CostAccumulator` works. Display total cost in TUI status bar. Wire `PricingTable` to active model. |
| 8 | **OpenAI HTTP provider** | Users can choose between Anthropic and OpenAI | M | None | Implement the `todo!()` in `openai.rs`. |
| 9 | **Ollama provider** | Users can run local models | M | None | Implement the `todo!()` in `ollama.rs`. Critical for the "local-first" value proposition. |
| 10 | **Recipe CLI integration** | Users can run declarative workflows | M | #1, #3 | Connect `--recipe` flag to `RecipeRunner`. Provide an executor closure that uses the agent. |

---

## 7. Technical Debt

1. **CLI argument parsing is hand-rolled.** The `parse_args()` function in `main.rs` manually iterates `std::env::args()`. Should use `clap` for proper arg parsing, help generation, and shell completions.

2. **DetectedProvider enum in main.rs duplicates trait dispatch.** The `DetectedProvider` enum with manual `match` arms reimplements dynamic dispatch that the `ModelProvider` trait already provides. Should use `Box<dyn ModelProvider>`.

3. **No error types in main binary.** Main uses `anyhow::Result` everywhere with no structured error handling. Provider detection, TUI setup, and agent turns should have typed errors.

4. **History accumulation is unbounded.** `history: Vec<Message>` in the TUI loop grows without limit. No context window management, no summarization, no sliding window.

5. **In-memory everything.** Every store (sessions, governance, collaborative, memory, index) is in-memory. The `sqlx` dependency is unused. This means zero persistence, zero crash recovery.

6. **No integration tests.** All 403 tests are unit tests on isolated modules. No test starts the TUI, connects to a provider, or exercises the end-to-end flow. No integration test for the server.

7. **Provider subprocess model is fragile.** Spawning `claude` and `gemini` as subprocesses and parsing their JSON output is brittle and adds latency. Should be a fallback, not the primary path.

8. **Unused dependencies.** `tree-sitter`, `sqlx`, `notify`, `keyring`, `argon2`, `tiktoken-rs`, `proptest`, `criterion` are in workspace deps but appear unused or minimally used. This inflates compile times.

9. **No CI/CD pipeline visible.** No GitHub Actions, no Makefile, no justfile. No automated test runs, no release builds.

10. **Seatbelt profile is never actually applied.** The stub logs a debug message and returns `Ok(SandboxHandle::new())`. In production this would silently run without sandboxing, violating the "fail-closed" design promise.

---

## 8. Recommendations

### P0 -- Do now (blocks MVP)

1. **Implement Anthropic HTTP provider with tool use.** This unblocks the entire tool architecture. The Claude CLI pass-through cannot support tool calling, which makes every other feature dead code.

2. **Build the agent execution loop.** Implement a turn-based loop: send messages + tool schemas to provider, receive tool calls, dispatch via `ToolDispatcher`, feed results back, repeat until `Done`. This is the heart of Hone.

3. **Wire native tools into the main binary.** Register `ReadFileTool`, `WriteFileTool`, `EditFileTool`, `GlobTool`, `ShellTool`, `GitStatusTool`, etc. in the CLI and provide their schemas to the provider.

4. **Make Seatbelt enforcement real.** Call `sandbox-exec -p <profile>` in `restrict_child()`. The profile generation already works. Without this, any tool execution in autonomous mode is unsafe.

5. **Load HONE.md and hone.toml at startup.** Wire config loading so users can configure model, routing, and project conventions without editing code.

### P1 -- Do next (needed for beta)

6. **Implement Ollama and OpenAI providers.** Replace the `todo!()` stubs. Ollama is critical for the local-first narrative.

7. **Add SQLite persistence for memory and index.** The `sqlx` dep is already declared. Store memories and symbol indices so they survive restarts.

8. **Build a real LSP JSON-RPC client.** Start with rust-analyzer and tsserver. The trait, tools, and auto-fix loop are ready -- they just need a real transport.

9. **Display cost in TUI.** The math works. Add a status bar widget showing cumulative cost and token usage.

10. **Connect recipe system to CLI.** Make `--recipe bug-fix` actually parse the YAML, create an executor using the agent, and run the steps.

11. **Replace hand-rolled arg parsing with clap.** Add subcommands: `hone chat`, `hone recipe`, `hone agent`, `hone ext`.

12. **Add context window management.** Implement sliding window or summarization for conversation history before it blows the context limit.

### P2 -- Do later (nice to have)

13. **Build web UI with SolidJS.** Server routes are partially ready. Add a frontend.

14. **Implement MCP JSON-RPC transport.** Enable external tool integrations (GitHub, Jira, etc.).

15. **Add multi-agent orchestration.** Agent definitions are loaded. Build a lead-worker coordinator.

16. **Implement WireGuard tunnel integration.** Trait and mock exist. Add a real tunnel provider.

17. **Add tree-sitter to semantic index.** Replace regex symbol extraction with proper AST parsing.

18. **Set up CI/CD.** GitHub Actions for test, lint, build, release.

19. **Benchmark startup time and memory.** Validate the two performance claims that are plausible.

20. **SQLite-backed governance and audit.** Move team governance and audit logs to persistent storage for enterprise use.
