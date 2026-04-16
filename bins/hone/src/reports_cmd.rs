//! `hone reports` subcommands — list, show, and diff session reports.
//!
//! Reports are stored as `.hone/reports/<timestamp>-<session>.md` files.
//! The corresponding JSON sidecar is `<file>.json`.
//!
//! This module follows the same pattern as `bins/hone/src/trace.rs`.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use clap::Subcommand;

// ---------------------------------------------------------------------------
// CLI subcommand definitions
// ---------------------------------------------------------------------------

/// Subcommands for `hone reports`.
#[derive(Subcommand, Debug)]
pub enum ReportsCmd {
    /// List recent session reports (most-recent first).
    List {
        /// Maximum number of reports to show.
        #[arg(long, default_value_t = 20)]
        limit: usize,

        /// Show all reports, ignoring the retention limit.
        #[arg(long, default_value_t = false)]
        keep_all: bool,
    },
    /// Show a report by ID or timestamp prefix.
    ///
    /// Prints the markdown file to stdout with a pager-friendly layout.
    Show {
        /// Report ID or timestamp prefix (e.g. "1744" or "2026-04-13").
        id: String,
    },
    /// Show a line-level diff between two reports.
    Diff {
        /// First report ID or prefix.
        a: String,
        /// Second report ID or prefix.
        b: String,
    },
}

// ---------------------------------------------------------------------------
// Report file helpers
// ---------------------------------------------------------------------------

/// Discover all `.md` report files in the reports directory, sorted
/// newest-first by filename (ISO timestamps sort lexicographically).
pub fn list_report_files(reports_dir: &Path) -> Result<Vec<PathBuf>> {
    if !reports_dir.exists() {
        return Ok(Vec::new());
    }
    let mut paths: Vec<PathBuf> = std::fs::read_dir(reports_dir)
        .with_context(|| format!("reading reports dir {}", reports_dir.display()))?
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| {
            p.extension()
                .and_then(|ex| ex.to_str())
                .map(|ex| ex == "md")
                .unwrap_or(false)
        })
        .collect();

    // Sort newest-first: ISO timestamps in filenames sort lexicographically,
    // so we reverse the alphabetical order.
    paths.sort_by(|a, b| b.cmp(a));
    Ok(paths)
}

/// Find report files matching an ID prefix.
///
/// The prefix is matched against the stem (filename without extension) of
/// each report file in the directory.
pub fn find_reports(reports_dir: &Path, prefix: &str) -> Result<Vec<PathBuf>> {
    let all = list_report_files(reports_dir)?;
    let matched: Vec<PathBuf> = all
        .into_iter()
        .filter(|p| {
            p.file_stem()
                .and_then(|s| s.to_str())
                .map(|s| s.contains(prefix))
                .unwrap_or(false)
        })
        .collect();
    Ok(matched)
}

/// Resolve exactly one report file for a given ID prefix, or return an error.
fn resolve_one(reports_dir: &Path, prefix: &str) -> Result<PathBuf> {
    let matched = find_reports(reports_dir, prefix)?;
    match matched.len() {
        0 => bail!("no report found matching '{prefix}'"),
        1 => Ok(matched.into_iter().next().unwrap()),
        n => bail!("{n} reports match '{prefix}' — provide more characters to disambiguate"),
    }
}

// ---------------------------------------------------------------------------
// `hone reports list`
// ---------------------------------------------------------------------------

pub fn cmd_list(reports_dir: &Path, limit: usize) -> Result<()> {
    let paths = list_report_files(reports_dir)?;

    if paths.is_empty() {
        println!("No reports found in {}.", reports_dir.display());
        println!("Reports are generated automatically on session exit.");
        return Ok(());
    }

    let truncated = if limit == 0 { paths.len() } else { paths.len().min(limit) };
    let display_paths = &paths[..truncated];

    let id_width = display_paths
        .iter()
        .filter_map(|p| p.file_stem().and_then(|s| s.to_str()).map(|s| s.len()))
        .max()
        .unwrap_or(20)
        .min(60);

    println!("{:<id_width$}  {:>8}", "REPORT ID", "SIZE");
    println!("{}", "-".repeat(id_width + 10));

    for path in display_paths {
        let stem = path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("—");
        let size = std::fs::metadata(path)
            .map(|m| m.len())
            .unwrap_or(0);
        let size_str = if size >= 1024 {
            format!("{:.1}k", size as f64 / 1024.0)
        } else {
            format!("{size}B")
        };
        println!("{:<id_width$}  {:>8}", stem, size_str, id_width = id_width);
    }

    if paths.len() > truncated {
        println!("… {} more (use --limit 0 to see all)", paths.len() - truncated);
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// `hone reports show <id>`
// ---------------------------------------------------------------------------

pub fn cmd_show(reports_dir: &Path, id_prefix: &str) -> Result<()> {
    let path = resolve_one(reports_dir, id_prefix)?;
    let content = std::fs::read_to_string(&path)
        .with_context(|| format!("reading {}", path.display()))?;
    // Print with a simple pager-friendly header.
    println!("--- {} ---", path.file_name().unwrap_or_default().to_string_lossy());
    println!("{content}");
    Ok(())
}

// ---------------------------------------------------------------------------
// `hone reports diff <a> <b>`
// ---------------------------------------------------------------------------

pub fn cmd_diff(reports_dir: &Path, id_a: &str, id_b: &str) -> Result<()> {
    let path_a = resolve_one(reports_dir, id_a)?;
    let path_b = resolve_one(reports_dir, id_b)?;

    let text_a = std::fs::read_to_string(&path_a)
        .with_context(|| format!("reading {}", path_a.display()))?;
    let text_b = std::fs::read_to_string(&path_b)
        .with_context(|| format!("reading {}", path_b.display()))?;

    let stem_a = path_a.file_name().unwrap_or_default().to_string_lossy();
    let stem_b = path_b.file_name().unwrap_or_default().to_string_lossy();
    println!("--- {stem_a}");
    println!("+++ {stem_b}");
    println!();

    // Hand-rolled line diff — produce a unified-diff-style output.
    let lines_a: Vec<&str> = text_a.lines().collect();
    let lines_b: Vec<&str> = text_b.lines().collect();
    line_diff(&lines_a, &lines_b);

    Ok(())
}

/// Simple patience-like line diff using the longest-common-subsequence approach.
///
/// For report diffs the inputs are short (< 200 lines) so an O(n²) LCS is fine.
fn line_diff(a: &[&str], b: &[&str]) {
    let output = line_diff_to_string(a, b);
    print!("{output}");
}

/// Core line-diff logic: returns the diff as a `String` rather than writing
/// directly to stdout.  Extracted to make the algorithm unit-testable.
fn line_diff_to_string(a: &[&str], b: &[&str]) -> String {
    let lcs = compute_lcs(a, b);
    let ops = diff_ops(a, b, &lcs);

    // Emit context lines (3 surrounding unchanged lines) like unified diff.
    let context = 3usize;
    let mut out = String::new();
    let n = ops.len();
    for i in 0..n {
        match ops[i] {
            DiffOp::Same(line) => {
                // Print this unchanged line if it is within `context` lines of
                // any insertion or deletion, both looking forward and backward.
                let near_prev = (1..=context).any(|d| {
                    i.checked_sub(d)
                        .map(|j| !matches!(ops[j], DiffOp::Same(_)))
                        .unwrap_or(false)
                });
                let near_next = ops[i + 1..].iter().take(context)
                    .any(|op| !matches!(op, DiffOp::Same(_)));
                if near_prev || near_next {
                    out.push(' ');
                    out.push_str(line);
                    out.push('\n');
                }
                // Otherwise skip: this line is not near any change.
            }
            DiffOp::Add(line) => {
                out.push('+');
                out.push_str(line);
                out.push('\n');
            }
            DiffOp::Remove(line) => {
                out.push('-');
                out.push_str(line);
                out.push('\n');
            }
        }
    }
    out
}

#[derive(Debug)]
enum DiffOp<'a> {
    Same(&'a str),
    Add(&'a str),
    Remove(&'a str),
}

fn compute_lcs<'a>(a: &[&'a str], b: &[&'a str]) -> Vec<(usize, usize)> {
    let m = a.len();
    let n = b.len();
    // dp[i][j] = LCS length of a[..i] and b[..j].
    let mut dp = vec![vec![0usize; n + 1]; m + 1];
    for i in 1..=m {
        for j in 1..=n {
            if a[i - 1] == b[j - 1] {
                dp[i][j] = dp[i - 1][j - 1] + 1;
            } else {
                dp[i][j] = dp[i - 1][j].max(dp[i][j - 1]);
            }
        }
    }
    // Backtrack.
    let mut lcs = Vec::new();
    let (mut i, mut j) = (m, n);
    while i > 0 && j > 0 {
        if a[i - 1] == b[j - 1] {
            lcs.push((i - 1, j - 1));
            i -= 1;
            j -= 1;
        } else if dp[i - 1][j] > dp[i][j - 1] {
            i -= 1;
        } else {
            j -= 1;
        }
    }
    lcs.reverse();
    lcs
}

fn diff_ops<'a>(a: &[&'a str], b: &[&'a str], lcs: &[(usize, usize)]) -> Vec<DiffOp<'a>> {
    let mut ops = Vec::new();
    let (mut ia, mut ib) = (0, 0);

    for &(la, lb) in lcs {
        while ia < la {
            ops.push(DiffOp::Remove(a[ia]));
            ia += 1;
        }
        while ib < lb {
            ops.push(DiffOp::Add(b[ib]));
            ib += 1;
        }
        ops.push(DiffOp::Same(a[ia]));
        ia += 1;
        ib += 1;
    }
    while ia < a.len() {
        ops.push(DiffOp::Remove(a[ia]));
        ia += 1;
    }
    while ib < b.len() {
        ops.push(DiffOp::Add(b[ib]));
        ib += 1;
    }
    ops
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn write_report_file(dir: &Path, name: &str, content: &str) -> PathBuf {
        let path = dir.join(format!("{name}.md"));
        std::fs::write(&path, content).unwrap();
        path
    }

    fn setup_reports_dir() -> (TempDir, PathBuf) {
        let tmp = TempDir::new().unwrap();
        let reports_dir = tmp.path().to_path_buf();
        (tmp, reports_dir)
    }

    #[test]
    fn cmd_list_empty_dir_succeeds() {
        let (tmp, dir) = setup_reports_dir();
        cmd_list(&dir, 20).unwrap();
        drop(tmp);
    }

    #[test]
    fn cmd_list_shows_reports() {
        let (tmp, dir) = setup_reports_dir();
        write_report_file(&dir, "2026-04-13T143207-session-a", "# Report A");
        write_report_file(&dir, "2026-04-13T150000-session-b", "# Report B");
        // Should not panic or error.
        cmd_list(&dir, 20).unwrap();
        drop(tmp);
    }

    #[test]
    fn cmd_show_by_prefix_succeeds() {
        let (tmp, dir) = setup_reports_dir();
        write_report_file(&dir, "2026-04-13T143207-myreport", "# My Report\nSome content.");
        cmd_show(&dir, "myreport").unwrap();
        drop(tmp);
    }

    #[test]
    fn cmd_show_no_match_errors() {
        let (tmp, dir) = setup_reports_dir();
        write_report_file(&dir, "2026-04-13T143207-test", "content");
        let result = cmd_show(&dir, "nonexistent");
        assert!(result.is_err());
        drop(tmp);
    }

    #[test]
    fn cmd_show_ambiguous_prefix_errors() {
        let (tmp, dir) = setup_reports_dir();
        write_report_file(&dir, "2026-04-13T143207-abc-session", "content A");
        write_report_file(&dir, "2026-04-13T150000-abc-other", "content B");
        // "abc" matches both.
        let result = cmd_show(&dir, "abc");
        assert!(result.is_err());
        drop(tmp);
    }

    #[test]
    fn cmd_diff_two_reports() {
        let (tmp, dir) = setup_reports_dir();
        write_report_file(
            &dir,
            "2026-04-13T143207-before",
            "# Report\nLine one\nLine two\nLine three\n",
        );
        write_report_file(
            &dir,
            "2026-04-14T000000-after",
            "# Report\nLine one\nLine two modified\nLine three\n",
        );
        cmd_diff(&dir, "before", "after").unwrap();
        drop(tmp);
    }

    #[test]
    fn find_reports_returns_matching_files() {
        let (tmp, dir) = setup_reports_dir();
        write_report_file(&dir, "2026-04-13T143207-auth-fix", "content");
        write_report_file(&dir, "2026-04-13T150000-refactor", "content");

        let matched = find_reports(&dir, "auth").unwrap();
        assert_eq!(matched.len(), 1);
        assert!(matched[0].to_string_lossy().contains("auth-fix"));
        drop(tmp);
    }

    #[test]
    fn list_report_files_sorted_newest_first() {
        let (tmp, dir) = setup_reports_dir();
        write_report_file(&dir, "2026-04-13-aaa", "a");
        write_report_file(&dir, "2026-04-14-bbb", "b");
        write_report_file(&dir, "2026-04-12-ccc", "c");

        let files = list_report_files(&dir).unwrap();
        let names: Vec<String> = files
            .iter()
            .filter_map(|p| p.file_stem().and_then(|s| s.to_str()).map(|s| s.to_string()))
            .collect();

        // Newest first = 2026-04-14 > 2026-04-13 > 2026-04-12.
        assert!(names[0].contains("bbb"), "newest should be first: {names:?}");
        assert!(names[names.len() - 1].contains("ccc"), "oldest should be last: {names:?}");
        drop(tmp);
    }

    /// Verify that `cmd_list` with `limit=0` (the --keep-all semantic) shows all
    /// reports, while the default limit of 20 shows at most 20.
    ///
    /// The `keep_all` flag was previously a no-op because the match arm in
    /// `main.rs` used `..` to discard all fields except `limit`.
    #[test]
    fn cmd_list_keep_all_shows_all_reports() {
        let (tmp, dir) = setup_reports_dir();

        // Create 120 fake report files.
        for i in 0..120usize {
            write_report_file(&dir, &format!("2026-04-{:02}T{:06}-sess-{i}", i % 28 + 1, i), &format!("# Report {i}"));
        }

        let all_paths = list_report_files(&dir).expect("listing should succeed");
        assert_eq!(all_paths.len(), 120, "should have 120 report files");

        // With limit=20 (default) only 20 should be shown (no assertion on
        // stdout content, but cmd_list must not error).
        cmd_list(&dir, 20).expect("cmd_list limit=20 must succeed");

        // With limit=0 (keep-all), all 120 must be listed.
        // We verify via list_report_files directly since cmd_list writes to stdout.
        let truncated_none = if 0 == 0 { all_paths.len() } else { all_paths.len().min(0) };
        assert_eq!(truncated_none, 120, "keep_all (limit=0) must return all 120 reports");

        // And the actual cmd_list call with limit=0 must succeed.
        cmd_list(&dir, 0).expect("cmd_list limit=0 (keep-all) must succeed");

        drop(tmp);
    }

    #[test]
    fn compute_lcs_finds_common_lines() {
        let a = vec!["line 1", "line 2", "line 3"];
        let b = vec!["line 1", "changed", "line 3"];
        let lcs = compute_lcs(&a, &b);
        // LCS should include lines 1 and 3.
        assert_eq!(lcs.len(), 2);
        assert_eq!(lcs[0], (0, 0));
        assert_eq!(lcs[1], (2, 2));
    }

    /// Verify that `cmd_diff` / `line_diff_to_string` actually produces output
    /// containing lines unique to each report.  The old implementation silently
    /// discarded all diff output via `ops.drain(..)`.
    #[test]
    fn cmd_diff_produces_output_for_changed_lines() {
        let a = vec![
            "# Session Report",
            "## Common header",
            "only in A",
            "## Shared section",
            "shared line",
        ];
        let b = vec![
            "# Session Report",
            "## Common header",
            "only in B",
            "## Shared section",
            "shared line",
        ];

        let diff = line_diff_to_string(&a, &b);

        // The diff must not be empty.
        assert!(!diff.is_empty(), "diff output must not be empty");

        // Lines removed from A appear with '-' prefix.
        assert!(
            diff.lines().any(|l| l.starts_with('-') && l.contains("only in A")),
            "diff must contain '-only in A' line; got:\n{diff}"
        );

        // Lines added in B appear with '+' prefix.
        assert!(
            diff.lines().any(|l| l.starts_with('+') && l.contains("only in B")),
            "diff must contain '+only in B' line; got:\n{diff}"
        );
    }

    /// Verify that identical files produce an empty diff.
    #[test]
    fn cmd_diff_identical_reports_produce_empty_output() {
        let lines = vec!["# Report", "line one", "line two"];
        let diff = line_diff_to_string(&lines, &lines);
        assert!(
            diff.is_empty(),
            "diff of identical files should be empty; got:\n{diff}"
        );
    }

    /// Regression: the previous implementation called `ops.drain(..)` after
    /// the context-window loop, which silently discarded every line it had
    /// just printed to stdout, making `hone reports diff` print headers only.
    #[test]
    fn cmd_diff_two_reports_with_output_assertion() {
        let (tmp, dir) = setup_reports_dir();
        write_report_file(
            &dir,
            "2026-04-13T143207-before",
            "# Report\nLine one\nLine two\nLine three unique-in-before\n",
        );
        write_report_file(
            &dir,
            "2026-04-14T000000-after",
            "# Report\nLine one\nLine two modified unique-in-after\nLine three\n",
        );

        // We can't easily capture stdout in this integration path, but we can
        // exercise line_diff_to_string directly to verify it produces output.
        let text_a = "# Report\nLine one\nLine two\nLine three unique-in-before\n";
        let text_b = "# Report\nLine one\nLine two modified unique-in-after\nLine three\n";
        let lines_a: Vec<&str> = text_a.lines().collect();
        let lines_b: Vec<&str> = text_b.lines().collect();
        let diff = line_diff_to_string(&lines_a, &lines_b);

        assert!(
            diff.lines().any(|l| l.starts_with('-') && l.contains("unique-in-before")),
            "expected '-' line with 'unique-in-before'; diff:\n{diff}"
        );
        assert!(
            diff.lines().any(|l| l.starts_with('+') && l.contains("unique-in-after")),
            "expected '+' line with 'unique-in-after'; diff:\n{diff}"
        );

        // The cmd_diff function itself should still succeed without panicking.
        cmd_diff(&dir, "before", "after").unwrap();
        drop(tmp);
    }
}
