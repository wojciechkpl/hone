# Changelog

All notable changes to Hone are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/), and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

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

- **SWE-bench v1 final results** — Hone achieves 58/300 (19.3%) on SWE-bench Lite with DeepSeek V3, beating SWE-Agent (GPT-4, 18%) at 50x lower cost (~$0.001-0.002 per task vs ~$0.10-0.20).
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
