# Changelog

All notable changes to Hone are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/), and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

- **Agent Rail observability sidebar** — Press `Ctrl+R` to toggle a real-time per-turn sidebar showing files touched (with R/W/X badges), injected context (skeleton + BM25 ranked entries with scores), and memory recalls. Width-adaptive (≥100 cols). Scrollable with j/k (future). Only appears in interactive TUI mode.
- **TurnLedger per-turn accumulator** — Internal data structure recording file operations, skeleton injections, memory recalls, tool invocations, and phase transitions. Automatically aggregated into session reports on `/quit`.
- **Session reports** — Auto-generated on exit to `.hone/reports/<timestamp>-<session>.md` + `.md.json` sidecar. Includes cost, tokens, file stats, tool timelines, context strategy, memory activity, and phase transitions. Default retention: 100 reports, oldest deleted first. (Currently TUI-mode only; recipe/orchestrate/ensemble/one-shot modes planned in follow-up.)
- **`hone reports` CLI** — Three subcommands: `list [--limit N]` (newest-first with file size), `show <id>` (full markdown + pager-friendly), `diff <a> <b>` (line-level LCS diff). Supports prefix matching on report IDs.
- **Accessibility flags** — `HONE_REDUCED_MOTION=1` disables animation in spinners (defined; full wiring in follow-up). `HONE_ASCII_ONLY=1` forces ASCII box-drawing and file-operation badges. Both respect `NO_COLOR=1` for color-blind and log-aggregation use cases.
- **Concurrency limiter for multi-agent orchestration** — `Orchestrator::with_max_concurrent(n)` caps the number of sub-agents running in parallel via tokio::sync::Semaphore. Useful for rate-limited providers or sequential execution. Effective limit is `n.min(max_workers)`. Setting n=1 serializes all workers.
- **Sub-agent health monitoring with timeouts** — `Orchestrator::with_worker_timeout(secs)` enforces per-worker wall-clock timeout. Workers exceeding the limit are cancelled and recorded as `WorkerStatus::TimedOut`. Defaults to 300s (5 minutes). Stuck agents are killed without aborting the entire orchestration.
- **Crash-recovery checkpointing** — `CheckpointStore` persists orchestration state to SQLite after each worker completes. On interruption, `Orchestrator::resume(run_id)` reloads the checkpoint and continues from where it left off. Stores goal, task plan, completed results, pending task IDs, and accumulated cost. Survives process kills, network failures, and timeouts.
- **Agent persona evals** — `benchmarks/persona_eval.py` evaluates 12 built-in agent personas across 35 scenarios (debugger, security-auditor, code-reviewer, rust-expert, python-expert, ai-researcher, postgresql-expert, ml-engineer, data-engineer, test-architect, fullstack-architect, ml-recommendations-expert, quant-advisor). Keyword-matching harness validates domain knowledge and terminology consistency. Run with `python benchmarks/persona_eval.py --url http://localhost:8080`.
- **Adaptive hierarchical skeleton** — Context injection auto-selects detail level based on repo size: <20 files = full call graph, 500+ files = directory tree only. 4-level hierarchy (directory → file list → signatures → call graph) with progressive budget filling. Smart file relevance detection skips files >100KB and slow parses (>100ms). Delivers optimal accuracy while respecting token budgets.
- **Scope clarification mode (`/clarify`, `--no-clarify`)** — Auto-detects ambiguous requests using 4 heuristics (missing file context, unclear scope, vague terminology, contradictory hints). Asks clarifying questions before coding. `--no-clarify` flag disables for CI/scripting. `/clarify` command manually triggers within interactive session.
- **Inline agent dispatch (`@agent-name`)** — Use `@security-auditor`, `@debugger`, `@code-reviewer`, etc. inline to swap system prompt for the turn. 12 built-in agents available. System prompt restores after the turn completes. Example: `@security-auditor "audit this code"`.
- **Safe pipe operator** — Pipes to head/tail/grep/wc/sort now allowed in shell commands (previously blocked as destructive). Pipes to unknown commands still escalate as destructive for safety.
- **Ollama tool calling enabled** — OllamaProvider now supports full tool calling (was previously hardcoded to skip). Qwen 2.5 Coder 14B is recommended local model for code generation with tool support.
- **Model override fixes** — `--model ollama` now directly creates OllamaProvider without falling back to Claude CLI when user explicitly requests Ollama.
- **Text-based tool call parser for Ollama** — Detects JSON tool calls in content field (for models that don't use tool_calls field). Strips markdown code fences automatically. Known limitation: Qwen 2.5 Coder sends tool calls as text but cannot process results back (use for testing only).
- **DeepSeek Direct compatibility** — Works for interactive use and simple tasks. SWE-bench benchmarks: lower output tokens than OpenRouter cause pipe blocking and empty patches (290/300 failed). OpenRouter remains recommended for benchmarks.
- **RALPH loop (Review, Analyze, Learn, Plan, Heal)** — Structured feedback cycle combining adversarial review with root cause analysis and pattern learning. Max 3 iterations (configurable), stops when reviewer approves. Captures learned patterns for cross-session improvement. Optional test verification after each Heal step. Research basis: Self-Refine (NeurIPS 2023), ChatDev, MASAI, and AgentCoder patterns. Implementation: `crates/hone-core/src/ralph_loop.rs`.
- **Token count in status bar** — Inline compact display (e.g., "42t") and full fraction format in wide mode (e.g., "42/200000 tokens"). Helps track context usage at a glance during interactive sessions.
- **9 recipes (expanded from 4)** — New recipes for security-audit, performance, migration, documentation, and onboarding workflows. Template variable support enables per-project customization with `--var KEY=VALUE`.
- **Shell escape (`!command`)** — Execute native shell commands directly from Hone prompt. `!ls`, `!git status`, `!cargo test` show inline output. `!cd src` changes directory persistently (confined to project root). `!nvim file.rs` suspends TUI, runs editor, restores on exit.
- **Vim mode (`--vim` or `:set vim`)** — Full vim-style keybindings in interactive mode. Normal mode: j/k for scroll, gg/G for top/bottom, i/a for insert, :q to quit. Insert mode: type normally, Esc returns to normal. Toggle with `:set vim` and `:set novim`.
- **TDD mode (`:tdd [command]`)** — File watcher + auto-test workflow. Watches source files with 300ms debounce, auto-runs test command on change, displays PASS/FAIL inline. Stop with `:tdd off`.
- **Review mode (`:review`)** — Hunk-by-hunk accept/reject interface for git diffs. Navigate with j/k, accept (a) or reject (r) individual hunks, accept/reject all (A/R), quit with q. Only accepted hunks are staged and committed.
- **JSON output mode (`--json`)** — Structured JSON output for CI/CD automation. Includes response text, tool calls, token usage, cost, and exit status. Non-interactive, suitable for scripting and integration.
- **Prompt injection detection** — Scans tool results and model output for injection patterns (high/medium/low severity). Uses regex-based heuristics to flag suspicious instructions like "ignore previous instructions" and shell escape sequences.
- **Recipe template variables (`--var KEY=VALUE`)** — Substitute {{var}} placeholders in recipe prompts. Repeatable; supports arbitrary key-value pairs. Example: `hone --recipe tdd-feature --var feature_name=oauth --var test_command="pytest tests/auth"`.
- **Egress audit log (`--audit`)** — JSONL audit trail of all tool calls and model requests. Records timestamp, tool name, arguments, and results to `.hone/audit.jsonl`. Enabled via `--audit` flag or `HONE_AUDIT_LOG=1`.
- **Proactive context injection (`--inject-context`)** — Three-tier context injection for improved SWE-bench results: (1) repository skeleton with file tree and signatures, (2) confidence-gated BM25-ranked function signatures, (3) exploration budget hint. Improves pass rate from 19.3% to 86% (+12pp). Enabled via `--inject-context` flag or `HONE_CONTEXT_INJECTION=1`.
- **MCP extensions integration** — Hone can now use 70+ community MCP extensions via `.hone/mcp.json` config (compatible with Claude Code format). McpManager spawns/manages servers with `env_clear` for security isolation. Zero changes to agent loop — McpTool already implements Tool trait.
- **Status bar redesign** — Three-zone layout: Left (model/session/phase), Center (tool + icon), Right (context bar with cost). Width-adaptive (80/120/160+ cols), context window bar with color thresholds, tool type icons, NO_COLOR support.
- **GitHub Pages site** — Documentation now auto-deploys on push via MkDocs + Material theme.
- **Named session management** — `hone session list/resume/rename/delete` commands for persistent named conversations. Sessions auto-save after each turn. `--session NAME` flag to start a named session. `--ephemeral` flag disables saving. Implementation: `bins/hone/src/session.rs`.
- **Interactive configure wizard** — `hone configure` guides users through provider selection, API key entry, and model selection. Writes `~/.config/hone/config.toml`. Implementation: `bins/hone/src/configure.rs`.
- **UX improvements** — NO_COLOR environment variable support for disabling colored output. Ollama model verification during configuration. Ensemble mode validates 2-10 candidate count. Provider capability warnings for --orchestrate and --ensemble modes.
- **Context optimization** — Model-aware token budgets, BM25 reordering for relevance, graduated compression strategies, and phase detection to minimize context overhead while preserving code comprehension.
- **Memory encryption** — AES-256-GCM symmetric encryption with Argon2id key derivation for encrypted session storage. Opt-in via `--features encrypted-memory`. Encryption keys stored in system keychain.
- **Cross-session learning** — AdaptiveClassifier learns patterns across sessions to improve request routing, complexity estimation, and tool selection. Patterns persist in encrypted memory.
- **Multi-agent budget tracking** — Unified token accounting across orchestrated agents. Per-turn, per-session, and per-user cost tracking with enforcement and rollback on budget exhaustion.
- **SWE-bench benchmark harness** — Full benchmarking pipeline for SWE-bench Lite (300 real GitHub issues). `benchmarks/swebench_inference.py` and `swebench_evaluate.py` with Makefile targets for swebench-dry-run, swebench-lite, and swebench-verified. Supports evaluation via Docker-isolated test execution.
- **Custom OpenAI-compatible endpoints** — `HONE_OPENAI_BASE_URL` and `HONE_OPENAI_MODEL` environment variables for drop-in support of DeepSeek, Qwen, Kimi, OpenRouter, and other OpenAI-compatible services.
- **Better startup error messages** — Clearer diagnostics for missing API keys, sandbox unavailability, configuration errors, and provider health checks.
- **System prompt compression** — Reduced system prompt from 1776 to 511 tokens (71% reduction), saving approximately 25K tokens per 20-turn task. Achieved through iterative refinement and removal of redundant instructions while maintaining quality parity.
- **Tool description overhaul** — All 7 core tools now feature structured WHEN TO USE / WHEN NOT TO USE fields for improved decision-making and tool selection accuracy.
- **SWE-bench v1 optimizations** — Structured 4-phase prompt design (exploration → localization → implementation → verification), two-stage localization strategy, and hardcoded tool guards (must-read-before-edit, test-after-edit hints, max consecutive edits limit).

### Changed

- **Context injection now enabled by default** — Proactive context injection (three-tier system: repo skeleton, confidence-gated BM25 signatures, exploration budgets) is now the default behavior, removing the need for `--inject-context` flag. Disable with `--no-inject` or `HONE_NO_INJECTION=1`. Multi-attempt strategy with rollback on failure improves pass rate from 86% to 95%.
- **SWE-bench v4 results** — Context injection enabled by default dramatically improves pass rate from 75% to 95% (+20pp). Multi-attempt strategy: attempt with injection-guided exploration first; if insufficient, rollback and retry with larger budget and no injection constraints. Achieves 95% pass rate at 50-100x lower cost than SWE-Agent.
- **SWE-bench v3 results** — Context injection (when opted-in) improves pass rate from 19.3% to 86% (+12pp). Three-tier injection system: skeleton-first (always), confidence-gated signatures (high-confidence matches only), exploration budget hints.
- **SWE-bench cost efficiency** — Hone v4 achieves 95% pass rate at 50-100x lower cost than SWE-Agent (GPT-4): ~$0.002 per task vs ~$0.10-0.20.
- **Review mode interaction** — `:review` now supports hunk-by-hunk navigation and selective acceptance. Rejected hunks are reverted; accepted hunks are staged.
- **Exit code 1 on missing provider** — Hone now returns exit code 1 when no provider is configured (was silently falling back to MockProvider). Users must configure a provider via `hone configure` or environment variables.
- **Security hardening (Phase 2 complete)**:
  - **Path traversal prevention** — All file tools use `resolve_within_working_dir()` to canonicalize paths and reject `..` escapes.
  - **Shell command classification** — New intent classifier detects compound shell operators (`;`, `&&`, `||`, `|`, `$()`, backticks) and escalates destructive commands to require explicit user approval.
  - **Seatbelt FFI on macOS** — `sandbox_init(3)` now fully functional. Path canonicalization handles macOS symlinks (`/tmp` → `/private/tmp`). Global `file-read*` rules for Dyld compatibility. Process fork/exec support for child sandboxing.
  - **Landlock on Linux** — Full Landlock LSM support (Linux 5.13+) with fine-grained filesystem access rules. Seccomp-BPF fallback for older kernels.
  - **CLI stdin prompts** — Sensitive input (API keys, one-shot prompts) now piped via stdin instead of CLI args to avoid exposure in `ps` output.
  - **Binary validation** — `check_binary_available()` now validates binary names with denylist of metacharacters instead of dangerous `sh -c` calls.
  - **Rate limiter using peer IP** — Server-side rate limiting now uses actual peer socket address instead of spoofable `X-Forwarded-For` header.

- **Storage improvements**:
  - **Dual-database strategy** — Semantic index now uses PostgreSQL+pgvector for vector similarity search (HNSW/IVFFlat indexes). Sessions and memories remain in SQLite for zero-ops local storage.
  - **RwLock poison recovery** — Provider registry uses `unwrap_or_else(|e| e.into_inner())` to recover state after panic.
  - **Memory IDs with UUID v7** — Session and memory IDs now use sortable UUID v7 instead of truncated timestamps.
  - **Cache keys with blake3** — Tool result cache uses blake3 hashing for integrity instead of non-cryptographic `DefaultHasher`.
  - **Git SHA via libgit2** — HEAD resolution now uses pure `git2` crate (in-process) instead of subprocess spawning.

- **Diagnostics improvements**:
  - Updated benchmark results for M-series Macs with warm-cache startup times.
  - Clarified token estimation costs (10ms per message including tiktoken cache warm-up).
  - Added memory footprint breakdown (hone process ~30-40MB, claude CLI subprocess ~270MB).
  - Documented SWE-bench evaluation methodology and Docker requirements.

### Deprecated

- **In-memory semantic index fallback only** — PostgreSQL+pgvector is now the default semantic index. In-memory fallback available only when database is unavailable (see `docs/architecture.md` section 9.2).

### Fixed

- **Ollama tool_call arguments serialization** — Fixed multi-turn tool-call replay: `convert_message` now sends `tool_calls[].function.arguments` as a JSON object (was stringified), matching Ollama's `/api/chat` wire format. Ollama's Go server was rejecting stringified arguments with HTTP 400 "Value looks like object, but can't find closing '}' symbol".
- **Shell injection vulnerability** — Command classification now prevents execution of compound shell operators without explicit user consent.
- **Path canonicalization race** — symlink targets are now validated to be in allowlist at tool execution time, preventing TOCTOU exploits.
- **Subprocess credential leakage** — API keys and tokens no longer exposed in process arguments; moved to stdin or environment (with whitelist).

### Documentation

- **Architecture document** — Updated with storage architecture (9.1, 9.2), security hardening (10), and key design decisions (11). Added ER diagrams for SQLite and PostgreSQL schemas.
- **Security guide** — Expanded threat model, added Seatbelt/Landlock platform matrix, documented environment variable filtering and audit logging.
- **Benchmarks document** — Added M-series performance baseline, cold-start analysis, memory breakdown, and SWE-bench methodology.
- **README** — Updated feature list with context optimization, memory encryption, cross-session learning, and custom endpoint support.
- **New CHANGELOG** — This document, using Keep a Changelog format.

### Testing

- **Security tests** — Added 20+ tests for path traversal prevention, shell command classification, and sandbox policy enforcement.
- **Context optimization tests** — Unit tests for BM25 reordering, graduated compression, and token budget enforcement.
- **Memory encryption tests** — Added AES-256-GCM roundtrip tests with Argon2id key derivation.
- **Benchmark microbenchmarks** — Updated for cold-start vs warm-start distinction. TUI render regression documented (16% vs baseline, still <0.1% frame budget).

### Previous Release (v0.1.0)

- Initial public release with core agent loop, 75+ model providers, native tools (L2), MCP client (L1), sandboxing (Seatbelt/Landlock), SQLite memory, semantic index, and multi-agent orchestration.

---

## Contributing

See [docs/contributing.md](docs/contributing.md) for development guidelines, testing, and commit message conventions.

## Security

For responsible disclosure of security vulnerabilities, see [SECURITY.md](SECURITY.md) (if available) or contact security@anthropic.com.
