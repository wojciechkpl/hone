# Audit-log timestamps use the real calendar: spec

Branch `fix/audit-timestamps`, off `main`. Found while reviewing the context-receipts PRs.

## Problem

`egress_log::now_rfc3339` (`crates/hone-core/src/egress_log.rs`) builds its date with 365-day years and 30-day months. Every `.hone/audit.jsonl` line is stamped with it, and `hone trace` shows it:
- on 2026-11-12 it writes `2026-11-30T…Z`;
- leap days accumulate, so the year and day drift further over time;
- late in the year it writes month `13` (day-of-year 360–364 → `364 / 30 + 1 = 13`).

## Rows

| Row | Behaviour |
|---|---|
| T1 | **Timestamps are correct RFC 3339 UTC.** A pure function `format_rfc3339_utc(secs_since_epoch: u64) -> String` returns `YYYY-MM-DDTHH:MM:SSZ` on the proleptic Gregorian calendar (via `chrono`, already a dependency). Known values: `0` → `1970-01-01T00:00:00Z`; `951782400` → `2000-02-29T00:00:00Z`; `1767225599` → `2025-12-31T23:59:59Z`; `1794441600` → `2026-11-12T00:00:00Z`; `1798761599` → `2026-12-31T23:59:59Z`. `now_rfc3339()` uses it. The output format (no fractional seconds, `Z`) stays the same, so `hone trace` parsing is unchanged. |
| T2 | **Old lines still parse.** `hone trace` keeps reading audit files whose timestamps were written by the old function (including month `13` or day `30` of February); such a line is shown with its stored text and doesn't abort the listing. |

## Out of scope

- Rewriting existing audit files.
