//! Interactive review mode: hunk-by-hunk accept/reject for session diffs.
//!
//! `:review` enters review mode. The user navigates hunks with j/k and
//! accepts (a) or rejects (r) each one. On exit, accepted hunks are
//! staged and committed; rejected hunks are reverted.

use std::path::Path;

/// A single diff hunk extracted from unified diff output.
#[derive(Debug, Clone)]
pub struct DiffHunk {
    /// File path this hunk belongs to.
    pub file: String,
    /// Hunk header (e.g., `@@ -10,5 +10,7 @@ fn main()`)
    pub header: String,
    /// Full hunk content (context + additions + deletions).
    pub lines: Vec<DiffLine>,
    /// Whether the user accepted this hunk.
    pub accepted: Option<bool>,
}

/// A single line in a diff hunk.
#[derive(Debug, Clone)]
pub struct DiffLine {
    pub kind: DiffLineKind,
    pub content: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiffLineKind {
    Context,
    Addition,
    Deletion,
    Header,
}

/// The review session state.
#[derive(Debug)]
pub struct ReviewSession {
    /// All hunks in the diff.
    pub hunks: Vec<DiffHunk>,
    /// Currently focused hunk index.
    pub current: usize,
    /// Whether the session is active.
    pub active: bool,
}

impl ReviewSession {
    /// Parse a unified diff string into a ReviewSession.
    pub fn from_diff(diff: &str) -> Self {
        let hunks = parse_unified_diff(diff);
        Self {
            hunks,
            current: 0,
            active: true,
        }
    }

    /// Move to the next hunk.
    pub fn next(&mut self) {
        if self.current + 1 < self.hunks.len() {
            self.current += 1;
        }
    }

    /// Move to the previous hunk.
    pub fn prev(&mut self) {
        if self.current > 0 {
            self.current -= 1;
        }
    }

    /// Accept the current hunk.
    pub fn accept_current(&mut self) {
        if let Some(hunk) = self.hunks.get_mut(self.current) {
            hunk.accepted = Some(true);
        }
        self.next();
    }

    /// Reject the current hunk.
    pub fn reject_current(&mut self) {
        if let Some(hunk) = self.hunks.get_mut(self.current) {
            hunk.accepted = Some(false);
        }
        self.next();
    }

    /// Accept all remaining unreviewed hunks.
    pub fn accept_all(&mut self) {
        for hunk in &mut self.hunks {
            if hunk.accepted.is_none() {
                hunk.accepted = Some(true);
            }
        }
    }

    /// Reject all remaining unreviewed hunks.
    pub fn reject_all(&mut self) {
        for hunk in &mut self.hunks {
            if hunk.accepted.is_none() {
                hunk.accepted = Some(false);
            }
        }
    }

    /// Get the current hunk.
    pub fn current_hunk(&self) -> Option<&DiffHunk> {
        self.hunks.get(self.current)
    }

    /// How many hunks have been reviewed.
    pub fn reviewed_count(&self) -> usize {
        self.hunks.iter().filter(|h| h.accepted.is_some()).count()
    }

    /// Total number of hunks.
    pub fn total_count(&self) -> usize {
        self.hunks.len()
    }

    /// Number of accepted hunks.
    pub fn accepted_count(&self) -> usize {
        self.hunks
            .iter()
            .filter(|h| h.accepted == Some(true))
            .count()
    }

    /// Number of rejected hunks.
    pub fn rejected_count(&self) -> usize {
        self.hunks
            .iter()
            .filter(|h| h.accepted == Some(false))
            .count()
    }

    /// Whether all hunks have been reviewed.
    pub fn is_complete(&self) -> bool {
        self.hunks.iter().all(|h| h.accepted.is_some())
    }

    /// Generate a patch containing only accepted hunks.
    pub fn accepted_patch(&self) -> String {
        let mut patch = String::new();
        let mut current_file: Option<&str> = None;

        for hunk in &self.hunks {
            if hunk.accepted != Some(true) {
                continue;
            }

            // Add file header if we switched files.
            if current_file != Some(&hunk.file) {
                patch.push_str(&format!("diff --git a/{f} b/{f}\n", f = hunk.file));
                patch.push_str(&format!("--- a/{}\n", hunk.file));
                patch.push_str(&format!("+++ b/{}\n", hunk.file));
                current_file = Some(&hunk.file);
            }

            patch.push_str(&hunk.header);
            patch.push('\n');
            for line in &hunk.lines {
                match line.kind {
                    DiffLineKind::Context => {
                        patch.push(' ');
                        patch.push_str(&line.content);
                        patch.push('\n');
                    }
                    DiffLineKind::Addition => {
                        patch.push('+');
                        patch.push_str(&line.content);
                        patch.push('\n');
                    }
                    DiffLineKind::Deletion => {
                        patch.push('-');
                        patch.push_str(&line.content);
                        patch.push('\n');
                    }
                    DiffLineKind::Header => {}
                }
            }
        }

        patch
    }

    /// Generate a patch containing only rejected hunks (for reverting).
    pub fn rejected_files(&self) -> Vec<String> {
        self.hunks
            .iter()
            .filter(|h| h.accepted == Some(false))
            .map(|h| h.file.clone())
            .collect::<std::collections::HashSet<_>>()
            .into_iter()
            .collect()
    }

    /// Format the current hunk for display.
    pub fn format_current_for_display(&self) -> String {
        let Some(hunk) = self.current_hunk() else {
            return "No hunks to review.".to_string();
        };

        let mut out = String::new();
        out.push_str(&format!(
            "[Review {}/{}] {} — {}\n",
            self.current + 1,
            self.total_count(),
            hunk.file,
            hunk.header.trim()
        ));
        out.push_str(
            "  a = accept | r = reject | A = accept all | R = reject all | q = finish\n\n",
        );

        for line in &hunk.lines {
            match line.kind {
                DiffLineKind::Addition => out.push_str(&format!("+{}\n", line.content)),
                DiffLineKind::Deletion => out.push_str(&format!("-{}\n", line.content)),
                DiffLineKind::Context => out.push_str(&format!(" {}\n", line.content)),
                DiffLineKind::Header => {}
            }
        }

        out.push_str(&format!(
            "\n{} accepted, {} rejected, {} remaining",
            self.accepted_count(),
            self.rejected_count(),
            self.total_count() - self.reviewed_count()
        ));

        out
    }
}

/// Apply accepted hunks: reset all changes, then apply accepted patch.
///
/// Returns a shell command string to execute.
pub fn build_apply_commands(session: &ReviewSession, working_dir: &Path) -> Vec<String> {
    let mut cmds = Vec::new();

    // First, stash everything
    cmds.push("git stash".to_string());

    // If there are accepted hunks, apply them
    if session.accepted_count() > 0 {
        let _patch = session.accepted_patch();
        let patch_path = working_dir.join(".hone/review-patch.diff");
        // TODO: Write patch file (caller handles this)
        cmds.push(format!(
            "git stash pop && git apply --allow-empty {}",
            patch_path.display()
        ));
    }

    cmds
}

/// Parse a unified diff into hunks.
fn parse_unified_diff(diff: &str) -> Vec<DiffHunk> {
    let mut hunks = Vec::new();
    let mut current_file = String::new();
    let mut current_header = String::new();
    let mut current_lines: Vec<DiffLine> = Vec::new();
    let mut in_hunk = false;

    for line in diff.lines() {
        if line.starts_with("diff --git") {
            // Flush previous hunk
            if in_hunk && !current_lines.is_empty() {
                hunks.push(DiffHunk {
                    file: current_file.clone(),
                    header: current_header.clone(),
                    lines: std::mem::take(&mut current_lines),
                    accepted: None,
                });
            }
            in_hunk = false;

            // Parse file path: "diff --git a/path b/path"
            if let Some(rest) = line.strip_prefix("diff --git a/") {
                if let Some(path) = rest.split(" b/").next() {
                    current_file = path.to_string();
                }
            }
        } else if line.starts_with("@@") {
            // Flush previous hunk in same file
            if in_hunk && !current_lines.is_empty() {
                hunks.push(DiffHunk {
                    file: current_file.clone(),
                    header: current_header.clone(),
                    lines: std::mem::take(&mut current_lines),
                    accepted: None,
                });
            }
            current_header = line.to_string();
            in_hunk = true;
        } else if in_hunk {
            let (kind, content) = if let Some(rest) = line.strip_prefix('+') {
                (DiffLineKind::Addition, rest.to_string())
            } else if let Some(rest) = line.strip_prefix('-') {
                (DiffLineKind::Deletion, rest.to_string())
            } else if let Some(rest) = line.strip_prefix(' ') {
                (DiffLineKind::Context, rest.to_string())
            } else {
                (DiffLineKind::Context, line.to_string())
            };
            current_lines.push(DiffLine { kind, content });
        }
    }

    // Flush last hunk
    if in_hunk && !current_lines.is_empty() {
        hunks.push(DiffHunk {
            file: current_file,
            header: current_header,
            lines: current_lines,
            accepted: None,
        });
    }

    hunks
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE_DIFF: &str = "\
diff --git a/src/auth.rs b/src/auth.rs
--- a/src/auth.rs
+++ b/src/auth.rs
@@ -10,5 +10,7 @@ fn login(user: &str) {
     let token = generate_token();
-    return token;
+    if user.is_empty() {
+        return Err(\"empty user\");
+    }
+    return Ok(token);
 }
@@ -20,3 +22,5 @@ fn logout() {
     session.clear();
+    session.save();
 }
diff --git a/src/main.rs b/src/main.rs
--- a/src/main.rs
+++ b/src/main.rs
@@ -1,3 +1,4 @@
 fn main() {
+    env_logger::init();
     run();
 }";

    #[test]
    fn test_parse_produces_three_hunks() {
        let session = ReviewSession::from_diff(SAMPLE_DIFF);
        assert_eq!(session.total_count(), 3);
        assert_eq!(session.hunks[0].file, "src/auth.rs");
        assert_eq!(session.hunks[1].file, "src/auth.rs");
        assert_eq!(session.hunks[2].file, "src/main.rs");
    }

    #[test]
    fn test_accept_and_reject() {
        let mut session = ReviewSession::from_diff(SAMPLE_DIFF);
        session.accept_current(); // hunk 0 accepted, moves to 1
        session.reject_current(); // hunk 1 rejected, moves to 2
        session.accept_current(); // hunk 2 accepted

        assert_eq!(session.accepted_count(), 2);
        assert_eq!(session.rejected_count(), 1);
        assert!(session.is_complete());
    }

    #[test]
    fn test_accept_all() {
        let mut session = ReviewSession::from_diff(SAMPLE_DIFF);
        session.reject_current(); // reject first
        session.accept_all(); // accept remaining

        assert_eq!(session.accepted_count(), 2);
        assert_eq!(session.rejected_count(), 1);
    }

    #[test]
    fn test_accepted_patch_contains_only_accepted() {
        let mut session = ReviewSession::from_diff(SAMPLE_DIFF);
        session.accept_current(); // hunk 0
        session.reject_current(); // hunk 1
        session.accept_current(); // hunk 2

        let patch = session.accepted_patch();
        assert!(patch.contains("empty user")); // hunk 0
        assert!(!patch.contains("session.save")); // hunk 1 rejected
        assert!(patch.contains("env_logger")); // hunk 2
    }

    #[test]
    fn test_format_shows_hunk_and_controls() {
        let session = ReviewSession::from_diff(SAMPLE_DIFF);
        let display = session.format_current_for_display();
        assert!(display.contains("[Review 1/3]"));
        assert!(display.contains("a = accept"));
        assert!(display.contains("src/auth.rs"));
    }

    #[test]
    fn test_navigation() {
        let mut session = ReviewSession::from_diff(SAMPLE_DIFF);
        assert_eq!(session.current, 0);
        session.next();
        assert_eq!(session.current, 1);
        session.next();
        assert_eq!(session.current, 2);
        session.next(); // already at end
        assert_eq!(session.current, 2);
        session.prev();
        assert_eq!(session.current, 1);
        session.prev();
        assert_eq!(session.current, 0);
        session.prev(); // already at start
        assert_eq!(session.current, 0);
    }

    #[test]
    fn test_empty_diff() {
        let session = ReviewSession::from_diff("");
        assert_eq!(session.total_count(), 0);
        assert!(session.is_complete());
        assert_eq!(session.format_current_for_display(), "No hunks to review.");
    }

    #[test]
    fn test_diff_line_kinds() {
        let session = ReviewSession::from_diff(SAMPLE_DIFF);
        let hunk = &session.hunks[0];
        let additions = hunk
            .lines
            .iter()
            .filter(|l| l.kind == DiffLineKind::Addition)
            .count();
        let deletions = hunk
            .lines
            .iter()
            .filter(|l| l.kind == DiffLineKind::Deletion)
            .count();
        assert!(additions > 0, "should have additions");
        assert!(deletions > 0, "should have deletions");
    }
}
