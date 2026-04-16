# Code Review: Phase 1 / P0 Agent Observability (commits 525cff2..f4156db)

**Date:** 2026-04-13  
**Reviewer:** Code Reviewer  
**Verdict:** PASS WITH FIXES — merge after addressing Major issues; Critical issue requires one targeted fix before merge.

---

## Executive Summary

Phase 1 delivers a functional observability baseline: TurnLedger event wiring, Agent Rail sidebar, accessibility flags, session reports, and the `hone reports` CLI. The architecture is clean, the test count (57+) is non-trivial, and the implementation notes are unusually honest about trade-offs. Three issues require attention before merge. One is Critical (byte-slicing a UTF-8 string in `record_memory_recall`). Three are Major: `cache_hit` is hardwired to `false` producing silently wrong report data; `line_diff` discards all context output by draining `ops` after computing it; and `keep_all` in `hone reports list` is accepted as a flag but never passed to `cmd_list`, making the feature a no-op. Everything else is Minor or a Suggestion. The `reduced_motion` gap in `animation.rs` is acknowledged and deferred, which is reasonable. The UNIX-epoch-only timestamp is a genuine UX regression but not a correctness bug.

---

## Findings Table

| # | Severity | File : Line | Description | Recommended Fix |
|---|---|---|---|---|
| 1 | Critical | `crates/hone-tui/src/turn_ledger.rs:267` | Byte-index slice on UTF-8 string in `record_memory_recall` will panic on any multi-byte snippet | Use `snippet.char_indices().nth(79).map(|(i, _)| &snippet[..i])` |
| 2 | Major | `crates/hone-tui/src/app.rs:1145` | `cache_hit` hardwired to `false` in every `ledger.record_tool_call` call; `ToolStat.cache_hits` is always 0 | Pass the actual cache-hit status; `ToolCallEnd` carries no cache flag so add `cache_hit: bool` to `ToolCallEnd` or derive from a cache-hit event |
| 3 | Major | `bins/hone/src/reports_cmd.rs:196–223` | `line_diff` first tries windowed output then calls `ops.drain(..)` discarding everything; the function always produces no output lines | Remove the dead windowed loop or replace it with the drain-based output; currently `hone reports diff` prints headers but zero diff lines |
| 4 | Major | `bins/hone/src/main.rs:1608` | `keep_all` field is matched with `..` (ignored); `cmd_list` is always called with `*limit`, never with `limit=0`; `--keep-all` is a no-op | Change match arm to `ReportsCmd::List { limit, keep_all }` and pass `if *keep_all { 0 } else { *limit }` |
| 5 | Minor | `bins/hone/src/main.rs:2417–2427` | `chrono_like_timestamp()` returns raw Unix epoch seconds (e.g. `"1744588327"`); filenames are not human-readable and the `list` UI shows opaque numbers | At minimum format as `YYYY-MM-DDTHHMMSS` in pure-Rust arithmetic to match ISO sort order; add `chrono`/`time` at P1 |
| 6 | Minor | `bins/hone/src/report.rs:553–555` | `enforce_retention` uses `with_extension("md.json")` to build the JSON path, but the actual sidecar is `<stem>.md.json` (two-extension compound); `Path::with_extension` replaces the last extension only, so `.json` is appended to `.md` → `.md.json` — this happens to be correct, but only by coincidence when the `.md` extension is treated as the last component | Make the sidecar path construction explicit: `md_path.with_file_name(format!("{}.json", md_path.file_name()...))` |
| 7 | Minor | `crates/hone-tui/src/render/agent_rail.rs:323` | `abbrev_strategy` does `&s[..s.len().min(4)]` — byte slice on a `&str` that may contain multi-byte chars in arbitrary strategy names from custom MCP tools | Use `s.char_indices().nth(4).map(|(i,_)| &s[..i]).unwrap_or(s)` |
| 8 | Minor | `crates/hone-tui/src/theme.rs:383–401` | Tests call `unsafe { std::env::set_var(...) }` without locking, racing with parallel test threads that also `set_var`/`detect()` | Use `std::sync::Mutex` in test helpers, or add `#[serial]` from the `serial_test` crate |
| 9 | Minor | `crates/hone-tui/src/app.rs:1091–1093` | A turn ledger is only started when the first `ToolCallStart` arrives; a pure-text turn (model responds with no tool calls) never creates or seals a ledger, so it is invisible to reports and the rail shows the previous turn's data indefinitely | Create the ledger on `Token` (first token of a new assistant message) and seal it on `Done`, even if no tools fired |
| 10 | Minor | `bins/hone/src/report.rs:376` | `file.path` and `ops` are interpolated unescaped into a markdown table cell — a path containing `|` (pipes) will corrupt the GFM table | Escape `|` in path and field values before interpolation |
| 11 | Minor | `crates/hone-tui/src/app.rs:1139–1144` | `pending_tool_start` is filtered by `id` equality but only one slot is stored; if two `ToolCallStart` events fire before their matching `ToolCallEnd` (parallel tool calls), the second start overwrites the first and the first end gets `tool_name = ""` | Use a `HashMap<String, (String, serde_json::Value)>` keyed by id |
| 12 | Minor | Implementation notes §TurnLedger / §Session Reports | `reduced_motion` is not wired into the existing `animation.rs::spinner_char()` — the status bar spinner still cycles regardless of the flag | Block or document as P0 gap; `status_bar.rs:200` calls `anim.spinner_char()` not `theme.spinner_glyph()` |
| 13 | Suggestion | `bins/hone/src/report.rs:458–459` | `render_json_sidecar` calls `serde_json::to_value(...).unwrap_or(serde_json::Value::Null)` — silently returns `null` on serialization failure, producing an invalid sidecar with no diagnostic | Use `?` or at least `tracing::error!` before the fallback |
| 14 | Suggestion | `bins/hone/src/reports_cmd.rs:83–89` | `find_reports` uses substring matching (`s.contains(prefix)`) not prefix matching; the doc says "prefix matching" but the implementation is substring matching — differs from git-style short-hash lookup | Decide on one and document it; `starts_with` is harder to collide accidentally |
| 15 | Suggestion | `crates/hone-tui/src/turn_ledger.rs:50–53` | `instant_now()` and `#[serde(skip, default = "instant_now")]` on `Instant` fields is a serde workaround that will cause deserialized ledgers to show wrong timestamps (they get `Instant::now()` at deserialization time, not the original time) | Use `Option<Instant>` defaulting to `None` and document the limitation, or store durations instead |
| 16 | Suggestion | `crates/hone-tui/src/render/agent_rail.rs:68–72` | Dead branch: `if nc { "[FILES]" } else { "[FILES]" }` — both arms return the same string; the unicode-vs-ascii header differentiation was presumably intended | Either add a unicode header glyph or collapse to a single string |

---

## Per-Focus-Area Deep Dives

### Focus 1: Event-to-Ledger Wiring

The main wiring is sound: `ToolCallStart` opens a ledger (lazily), `ToolCallEnd` records the tool call, `Done` seals and moves the ledger to `completed_turns`. The id-based `pending_tool_start` correlation is correct for sequential tool calls, but **breaks silently for concurrent tool calls** because only one `(id, name, args)` tuple is stored (finding #11). In practice Anthropic's API sends parallel tool calls with distinct IDs before their matching end events arrive. When this happens the second `ToolCallStart` overwrites the first, and the first `ToolCallEnd` matches by id → `None` → falls through the filter → `tool_name = ""`.

Phase transitions are only recorded once (on the first `ToolCallStart`). Any subsequent phase label changes (from `status.update_phase_from_tool`) within the same turn are silently dropped. The design doc calls for per-transition recording, not just the initial one (finding #9 also touches this).

Pure-text turns (no tools) leave no ledger entry and are therefore invisible in reports. The rail correctly shows the last completed turn's data, but the turn count in `ReportData.turn_count` counts ledgers, not `Done` events, so text-only turns are undercounted (finding #9).

`cache_hit` is always `false` (finding #2). The `ToolCallEnd` proto event has no `cache_hit` field. The `ToolStat.cache_hits` column in reports will always show 0 regardless of actual caching activity.

Memory recalls and skeleton injections are never populated from `AgentEvent` — there are no corresponding event variants, so the CONTEXT and MEMORY rail sections will always show "—". This is a known gap, not a bug introduced here, but the Phase 1 spec implies they should appear. The implementation notes do not call this out explicitly.

### Focus 2: Layout Robustness

`split_with_rail` returns `None` below 100 cols, which is correctly handled in `render_content_area`. The `chunks[0]` and `chunks[1]` indexing is safe because `Layout::split` guarantees a `Vec` of the same length as the constraints slice. The row-level loop in `AgentRailWidget::render` uses `i as u16` without checking overflow — at a visible_height of 65535 lines this wraps, but that cannot happen because `inner.height` is a `u16` and each line is 1 row. No panic risk identified.

`AgentRailState.scroll` is clamped correctly before the skip. The design note about "scroll state is reset on new turn" is implemented by constructing a fresh `AgentRailState { scroll: app.rail_scroll() }` each frame, with `rail_scroll` being reset to 0 in `toggle_agent_rail`. There is no mechanism to scroll the rail with keyboard keys yet — that is correctly deferred.

One cosmetic issue: the rail is 18 cols wide but the `no_color` border is `BorderType::Plain` which uses `|` and `-`, consuming 2 cols, leaving 16 cols of content. At 100 cols terminal width the minimum, this is tight but not broken.

### Focus 3: Report Generation Correctness

**Critical (finding #1):** `record_memory_recall` slices at byte position 79 on a `&str` that may contain multi-byte UTF-8 characters. `&snippet[..79]` will panic with a `byte index N is not a char boundary` error if the snippet contains any non-ASCII character (e.g. Japanese, emoji, or an ellipsis character `…` itself if the caller passes one). The fix is to slice on char indices.

**Markdown injection (finding #10):** `file.path` is written directly into `| file.path | ops |` table cells. A file path containing `|` (legitimately possible on Linux/macOS) would break the GFM table. Tool names from MCP tools could also contain `|`. Escaping is a one-liner.

**Empty ledger:** `render_markdown` with an empty `files` slice emits "No files touched." and empty tool section — handled correctly.

**1000 turns:** `from_ledgers` iterates all ledgers once each, O(n). Fine.

**JSON round-trip:** `render_json_sidecar` uses `serde_json::to_value` on a `#[derive(Serialize, Deserialize)]` struct — this round-trips correctly for all field types present. The `unwrap_or(Null)` fallback is silent but not harmful in tests (finding #13).

**Timestamp sort order (finding #5):** Unix epoch seconds produce lexicographically sortable filenames only if zero-padded to equal length. Current epoch seconds are 10 digits and will remain so until 2286, so the sort is correct in practice but fragile and the `list` output is unreadable. This should be treated as P1.

**Tool call timeline bar:** `(t.total_duration_ms / 1000).min(30) as usize` — integer division truncates. A 999ms tool call shows 0 bars. Acceptable for a first pass.

### Focus 4: CLI Subcommand Robustness

**Critical no-op (finding #4):** `--keep-all` is defined in the `ReportsCmd::List` variant but the match arm in `main.rs:1608` uses `..` to discard all fields except `limit`. The `keep_all` value never reaches `cmd_list`. The flag appears in `hone reports list --help` and silently does nothing.

**Ambiguous prefix (finding #14):** The error message when multiple reports match is good (`"{n} reports match — provide more characters"`). However, the matching is `contains` not `starts_with`, so searching for "session" will match both `2026-04-13-my-session` and `2026-04-13-session-b`. This is more surprising than git-style prefix matching and worth documenting or changing.

**Missing file in diff:** If either report file is deleted between `resolve_one` and `read_to_string`, the error is propagated as an `anyhow` context error — adequate.

**`cmd_list` with `limit=0`:** `let truncated = if limit == 0 { paths.len() } else { paths.len().min(limit) }` — this is the correct "show all" semantic. But since `keep_all` never reaches `cmd_list`, this branch is currently unreachable from the CLI.

### Focus 5: Retention / Deletion Safety

`enforce_retention` enumerates, sorts, and deletes in a single-threaded context. There is no TOCTOU window within the function itself because Rust's `fs::remove_file` is atomic at the syscall level. A concurrent `hone reports show` reading a file while it is being deleted will get an error from `read_to_string`, which the caller propagates as an `anyhow` error. This is acceptable for a local tool.

The JSON sidecar path construction (`md_path.with_extension("md.json")`) is analyzed in finding #6. It works correctly in the current implementation because `Path::with_extension` on a path ending in `.md` replaces `.md` with `md.json` — wait: `with_extension("md.json")` replaces the last extension (`.md`) with `md.json`, producing `stem.md.json`. This is the correct intended filename. But finding #6 is still valid as a clarity concern since the behavior is non-obvious.

No opt-out from retention. This is by spec design. No flag is documented.

### Focus 6: Accessibility Flag Consistency

`reduced_motion` is added to `Theme` and `Theme::spinner_glyph()` is the correct abstraction. However, **`status_bar.rs:200` calls `anim.spinner_char()` directly**, bypassing `theme.spinner_glyph()`. This means `HONE_REDUCED_MOTION=1` has no visible effect in the one place users most expect it — the status bar spinner (finding #12). The implementation notes acknowledge this as a known gap, but because the feature is documented in the release notes of this commit, users will discover the gap immediately and file a bug. It should either be fixed now (a 2-line change: replace `anim.spinner_char()` with `app.theme().spinner_glyph(anim.frame())`) or be called out explicitly in a `// TODO(reduced_motion)` comment in `status_bar.rs`.

`ascii_only` and `no_color` are respected in the Agent Rail widget (border type, badge selection, path coloring). The `AgentRailWidget::render` uses `theme.no_color` to pick `BorderType::Plain` vs `BorderType::Rounded` — but `ascii_only` is not checked for the border type. Since `ascii_only` implies different box chars (via `box_chars()`), and ratatui's `BorderType::Rounded` uses `╭`/`╰` which are not ASCII, a user with `HONE_ASCII_ONLY=1` but `NO_COLOR` unset will get rounded Unicode borders from ratatui despite expecting ASCII. The `box_chars()` abstraction is unused in the widget layer. Minor.

No hardcoded `⠋` spinners outside `animation.rs::SPINNER_FRAMES` and `theme.rs::FRAMES`. The old `SPINNER_FRAMES` in `animation.rs` (`─`, `\`, `│`, `/`) are not Braille, so `spinner_char()` is already ASCII-safe.

### Focus 7: Security & Privacy

**Path traversal:** `write_report` takes `dir: &Path` (caller-supplied working directory from `std::env::current_dir()`) and joins `.hone/reports/<timestamp>-<session_id>`. The session ID is sanitized by replacing non-alphanumeric/dash/underscore chars with `-`. The timestamp is sanitized by replacing `:` and spaces with `-`. Neither allows `..` to escape the reports directory.

**Report content and secrets:** Tool call arguments are stored in `pending_tool_start` as a `serde_json::Value` and used only to extract file paths via `extract_path`. Tool results are stored as `DisplayMessage` content (for the conversation view) and the file preview pane, but **not** in the report data. Report data only contains: file paths, tool names, durations, and aggregate counts. Tool result content does not flow into the report. This is good — a `read_file` result containing `.env` file contents will not appear in `.hone/reports/*.md`.

One minor concern: `ToolCallEnd::result` is stored in `preview_content` (app.rs:1127) for display in the preview pane. This preview is ephemeral (in-memory only) and not written to disk, so it is not a report-privacy issue.

**No network egress** from any of the new code paths.

### Focus 8: Test Quality

57 new tests were claimed; let's assess the real ones:

**`turn_ledger.rs` (18 tests):** All contain meaningful assertions. `record_file_touch_deduplicates_by_path` checks the dedup invariant. `summary_counts_correctly` validates all six counters against known inputs. `integration_fake_agent_event_sequence` verifies the full Read→Write sequence and dedup. `top_skeleton_sorts_by_bm25_descending` checks ordering with three entries. These are genuine unit tests, not just "doesn't panic" guards.

**`agent_rail.rs` (6 tests):** `truncate_path_left_long_path_shows_filename` and `truncate_str_long` have real assertions. `split_with_rail_too_narrow` and `split_with_rail_wide_enough` verify layout arithmetic. The render tests (`rail_renders_without_panic_empty_ledger`, `rail_renders_with_ledger_data`, `rail_renders_no_color_mode`) are smoke tests — they verify no panic but do not assert buffer content. **No test verifies that specific text appears at specific buffer positions.** This is a genuine gap.

**`report.rs` (9 tests):** `render_stdout_no_color_has_no_ansi` is a good negative assertion. `render_json_sidecar_is_valid_json` checks field values, not just type. `fmt_secs_formats_correctly` and `fmt_num_formats_correctly` cover edge values including zero. `enforce_retention_deletes_oldest` has a critical weakness: it acknowledges that mtime resolution may cause test unreliability, and the final assertion (`remaining.len() <= 5`) is vacuously true since the initial count is 5 — the test does not verify that exactly 3 files remain. This is a vanity assertion (finding noted below).

**`reports_cmd.rs` (8 tests):** `cmd_show_ambiguous_prefix_errors` and `cmd_show_no_match_errors` exercise error paths. `list_report_files_sorted_newest_first` uses date-prefixed filenames and verifies ordering by name. `compute_lcs_finds_common_lines` directly exercises the LCS algorithm. These are solid. `cmd_diff_two_reports` calls the function but does not capture or assert stdout output.

**`theme.rs` (9 tests):** The `detect_reads_*` tests use `unsafe { set_var }` without test isolation; if run in parallel with other `detect_*` tests they can race (finding #8).

**Missing test coverage:**
- No test for the `line_diff` output (the bug in finding #3 would have been caught).
- No test for `record_memory_recall` with a multi-byte snippet (finding #1 would have been caught).
- No test for `keep_all` pass-through (finding #4 would have been caught by an integration test).
- No test for markdown table injection via a path containing `|`.

---

## Spot-Check: Deferred Items

| Deferred item | Reasonable? | Latent risk |
|---|---|---|
| Non-TUI mode reports (recipe/orchestrate/ensemble) | Yes — agreed partial; spec allows it | Users who exclusively use `--recipe` get no reports; no error message warns them. Minor UX surprise. Not a bug. |
| `reduced_motion` not wired into `animation.rs` | **Marginal** — the flag is documented on this commit | Users setting `HONE_REDUCED_MOTION=1` will see the spinner still cycling; the feature is broken-from-ship |
| Width scaling (fixed 18 vs spec 16–24) | Yes — trivial follow-up | Not a bug |
| Scroll with j/k in rail | Yes — deferred focus mode is P1 | Not a bug |
| ISO8601 timestamps (epoch only) | Marginal — sortable but unreadable | The `list` command output is unusable for human navigation |
| `tokens_out` / `cache_read_tokens` = 0 | Yes — partial is flagged | Misleading but documented |
| `/share-report` | Yes — clearly P1 | No concern |
| Inspect overlay Ctrl+I | Yes — P1 | No concern |

**Items that should block merge:** `reduced_motion` broken-from-ship is the only one I recommend blocking on if the commit message claims the flag works. Otherwise, patch it as a follow-up tagged explicitly.

---

## Concrete Follow-Up Task List

**P0 (fix before merge):**
1. Fix byte-slice panic in `record_memory_recall` (finding #1).
2. Fix `line_diff` producing zero output lines (finding #3).
3. Fix `keep_all` not being forwarded in the CLI match arm (finding #4).

**P1 (fix in next sprint):**
4. Fix `cache_hit` always `false` — add `cache_hit` field to `ToolCallEnd` or a separate event (finding #2).
5. Fix `reduced_motion` not wired into `status_bar.rs::spinner_char()` — or add a `// TODO` comment to `status_bar.rs` with a link to the tracking issue (finding #12).
6. Fix parallel tool call id correlation (`HashMap` instead of single slot) (finding #11).
7. Add turn ledger entry for pure-text turns / count `Done` events rather than sealed ledgers (finding #9).
8. Escape `|` characters in markdown table cells (finding #10).
9. Human-readable timestamp in filenames (finding #5).

**P2 (clean-up):**
10. Fix `abbrev_strategy` byte-slice for multi-byte strategy names (finding #7).
11. Isolate `detect_reads_*` tests with a mutex or serial annotation (finding #8).
12. Add buffer-content assertions to Agent Rail render tests.
13. Fix `enforce_retention_deletes_oldest` to assert `remaining.len() == 3` after retention enforcement.
14. Add `cmd_diff` stdout capture test that verifies diff lines appear.
15. Remove dead `if nc { "[FILES]" } else { "[FILES]" }` branch (finding #16).

---

## Fixes Applied

The following Critical and Major findings were resolved in a post-review pass.

| Finding | Severity | Resolution | Commit |
|---|---|---|---|
| #1 — Byte-slice panic in `record_memory_recall` | Critical | Replaced `&snippet[..79]` with `truncate_chars` helper that walks `char_indices`; added regression tests for emoji, CJK, short strings, and exact boundary. | `8330b95` |
| #2 — `cache_hit` hardwired to `false` | Major | Added `cache_hit: bool` to `ToolOutput` and `AgentEvent::ToolCallEnd` (serde-defaulted to false for backward compat); `CachingToolWrapper::call` probes the backend before `get_or_compute` to distinguish hit from miss; `agent.rs` propagates the flag; `app.rs` passes it to `ledger.record_tool_call`. Added test that calls a cacheable wrapper twice and asserts `cache_hit` is false on miss, true on hit. | `8b10a14` |
| #3 — `hone reports diff` produces no diff lines | Major | Removed the `ops.drain(..)` stub that discarded all printed output; extracted `line_diff_to_string` for direct testability; fixed backward context window to look back `context` lines instead of 1. Added three tests: non-empty diff, identical-files empty output, and regression guard for the drain bug. | `8518e6d` |
| #4 — `--keep-all` no-op on `hone reports list` | Major | Changed the `ReportsCmd::List` match arm in `main.rs` to destructure `keep_all` and pass `effective_limit = if keep_all { 0 } else { limit }` to `cmd_list`. Added test with 120 fake reports asserting `limit=0` returns all and the default limit bounds correctly. | `8f79c13` |

**Tests added:** 12 new tests across `turn_ledger.rs`, `caching_tool.rs`, and `reports_cmd.rs`.  
**All workspace tests pass** (Ollama provider test was pre-existing failure, unrelated).  
**Clippy:** No new warnings introduced in modified files; pre-existing warnings in unrelated files (`hone-core`, `hone-providers`) are out of scope for this pass.
