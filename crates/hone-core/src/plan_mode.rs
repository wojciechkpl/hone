//! Plan mode: read-only investigation with plan capture.
//!
//! See the feature contract in the task specification for full design details.

use std::path::{Path, PathBuf};

use hone_tools::ToolRegistry;

// ---------------------------------------------------------------------------
// AgentMode
// ---------------------------------------------------------------------------

/// Three operational modes the agent can be in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentMode {
    /// Default: all tools available, edits applied immediately.
    Build,
    /// Read-only investigation. Tools are filtered to `is_read_only == true`.
    /// Final assistant response is captured as a plan.
    Plan,
    /// Build mode but with all tool approvals auto-accepted (no prompts).
    AutoApprove,
}

impl AgentMode {
    /// Cycle to the next mode: Build -> Plan -> AutoApprove -> Build.
    pub fn cycle_next(self) -> AgentMode {
        match self {
            AgentMode::Build => AgentMode::Plan,
            AgentMode::Plan => AgentMode::AutoApprove,
            AgentMode::AutoApprove => AgentMode::Build,
        }
    }

    /// Short label for the status bar.
    pub fn label(self) -> &'static str {
        match self {
            AgentMode::Build => "build",
            AgentMode::Plan => "plan",
            AgentMode::AutoApprove => "auto",
        }
    }
}

// ---------------------------------------------------------------------------
// read_only_registry
// ---------------------------------------------------------------------------

/// Filter a `ToolRegistry` down to tools where `Tool::is_read_only()` returns
/// `true`. Reuses the builder pattern from `filtered_registry` in
/// `agent_registry.rs`.
pub fn read_only_registry(base: &ToolRegistry) -> ToolRegistry {
    let mut builder = ToolRegistry::builder();
    for name in base.names() {
        if let Some(tool) = base.get(name) {
            if tool.is_read_only() {
                builder = builder.register_arc(tool.clone());
            }
        }
    }
    builder.build()
}

// ---------------------------------------------------------------------------
// slugify
// ---------------------------------------------------------------------------

/// Derive a URL-safe slug from a user prompt.
///
/// Rules:
/// - Lowercase everything.
/// - Replace any non-alphanumeric character with `-`.
/// - Collapse consecutive `-`s into one.
/// - Trim leading/trailing `-`s.
/// - Truncate to 40 characters (trim trailing `-` after truncation).
/// - If the result is empty, return `"plan"`.
pub fn slugify(s: &str) -> String {
    // Lowercase and map non-alphanumeric chars to '-'.
    let lowered: String = s
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect();

    // Collapse consecutive dashes.
    let mut result = String::with_capacity(lowered.len());
    let mut last_was_dash = false;
    for c in lowered.chars() {
        if c == '-' {
            if !last_was_dash {
                result.push('-');
            }
            last_was_dash = true;
        } else {
            result.push(c);
            last_was_dash = false;
        }
    }

    // Trim leading/trailing dashes.
    let trimmed = result.trim_matches('-').to_string();

    // Truncate to 40 chars, then trim any trailing dash created by truncation.
    let truncated = if trimmed.len() > 40 {
        let cut = &trimmed[..40];
        cut.trim_end_matches('-').to_string()
    } else {
        trimmed
    };

    if truncated.is_empty() {
        "plan".to_string()
    } else {
        truncated
    }
}

// ---------------------------------------------------------------------------
// SavedPlan
// ---------------------------------------------------------------------------

/// One saved plan on disk.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct SavedPlan {
    /// RFC3339 timestamp, also embedded in the filename.
    pub timestamp: String,
    /// 40-char URL-safe slug derived from `user_prompt`.
    pub slug: String,
    /// The original prompt that led to the plan.
    pub user_prompt: String,
    /// The assistant's final text response.
    pub plan_body: String,
}

// ---------------------------------------------------------------------------
// PlanModeStore
// ---------------------------------------------------------------------------

/// Persistence layer for plan-mode outputs.
pub struct PlanModeStore {
    base_dir: PathBuf,
}

impl PlanModeStore {
    /// Create a store rooted at `base_dir`.
    pub fn new(base_dir: impl Into<PathBuf>) -> Self {
        Self {
            base_dir: base_dir.into(),
        }
    }

    /// `<working_dir>/.hone/plans`.
    pub fn default_for(working_dir: &Path) -> Self {
        Self::new(working_dir.join(".hone/plans"))
    }

    /// Save a plan. Returns the path of the written file.
    ///
    /// File format (frontmatter + body):
    /// ```text
    /// ---
    /// timestamp: 2026-05-02T18:30:00Z
    /// slug: fix-auth-middleware
    /// ---
    /// # User prompt
    /// <user_prompt verbatim>
    ///
    /// # Plan
    /// <plan_body verbatim>
    /// ```
    ///
    /// Filename: `<YYYYMMDD-HHMMSS>-<slug>.md`.
    pub async fn save(&self, plan: &SavedPlan) -> std::io::Result<PathBuf> {
        tokio::fs::create_dir_all(&self.base_dir).await?;

        let prefix = timestamp_to_prefix(&plan.timestamp);
        let filename = format!("{}-{}.md", prefix, plan.slug);
        let path = self.base_dir.join(&filename);

        let content = format!(
            "---\ntimestamp: {}\nslug: {}\n---\n# User prompt\n{}\n\n# Plan\n{}\n",
            plan.timestamp, plan.slug, plan.user_prompt, plan.plan_body
        );

        tokio::fs::write(&path, content).await?;
        Ok(path)
    }

    /// Load a saved plan by path.
    pub async fn load(path: &Path) -> std::io::Result<SavedPlan> {
        let raw = tokio::fs::read_to_string(path).await?;
        parse_plan_file(&raw)
    }

    /// List saved plans in the store, newest first by filename sort.
    pub async fn list(&self) -> std::io::Result<Vec<PathBuf>> {
        if !self.base_dir.exists() {
            return Ok(vec![]);
        }

        let mut entries = tokio::fs::read_dir(&self.base_dir).await?;
        let mut paths: Vec<PathBuf> = Vec::new();

        while let Some(entry) = entries.next_entry().await? {
            let path = entry.path();
            if path.extension().map_or(false, |e| e == "md") {
                paths.push(path);
            }
        }

        // Sort descending by filename (lexicographic; YYYYMMDD-HHMMSS prefix ensures
        // correct chronological order when reversed).
        paths.sort_by(|a, b| {
            let a_name = a.file_name().and_then(|n| n.to_str()).unwrap_or("");
            let b_name = b.file_name().and_then(|n| n.to_str()).unwrap_or("");
            b_name.cmp(a_name)
        });

        Ok(paths)
    }
}

// ---------------------------------------------------------------------------
// Private helpers
// ---------------------------------------------------------------------------

/// Convert an RFC3339-ish timestamp to `YYYYMMDD-HHMMSS`.
///
/// Strategy: collect only digit chars, take first 14, split 8+6 with `-`.
/// Pads with `0`s on the right if fewer than 14 digits are present.
fn timestamp_to_prefix(ts: &str) -> String {
    let digits: String = ts.chars().filter(|c| c.is_ascii_digit()).collect();
    let mut padded = digits;
    while padded.len() < 14 {
        padded.push('0');
    }
    format!("{}-{}", &padded[..8], &padded[8..14])
}

/// Parse a plan file into a [`SavedPlan`].
///
/// Expected format:
/// ```text
/// ---
/// timestamp: <value>
/// slug: <value>
/// ---
/// # User prompt
/// <user_prompt>
///
/// # Plan
/// <plan_body>
/// ```
fn parse_plan_file(raw: &str) -> std::io::Result<SavedPlan> {
    // Strip leading "---\n".
    let rest = raw
        .strip_prefix("---\n")
        .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::InvalidData, "missing frontmatter opening ---"))?;

    // Split frontmatter from body on "\n---\n".
    let (frontmatter_str, body) = rest.split_once("\n---\n").ok_or_else(|| {
        std::io::Error::new(std::io::ErrorKind::InvalidData, "missing frontmatter closing ---")
    })?;

    // Parse frontmatter lines manually (avoid serde_yaml complexity for two fields).
    let mut timestamp = String::new();
    let mut slug = String::new();
    for line in frontmatter_str.lines() {
        if let Some(val) = line.strip_prefix("timestamp:") {
            timestamp = val.trim().to_string();
        } else if let Some(val) = line.strip_prefix("slug:") {
            slug = val.trim().to_string();
        }
    }

    // Parse body sections.
    let user_prompt = extract_section(body, "# User prompt\n", "# Plan\n")?;
    let plan_body = extract_section_tail(body, "# Plan\n")?;

    Ok(SavedPlan {
        timestamp,
        slug,
        user_prompt,
        plan_body,
    })
}

/// Extract text between `start_header` and `end_header` within `text`.
fn extract_section(text: &str, start_header: &str, end_header: &str) -> std::io::Result<String> {
    let after_start = text
        .find(start_header)
        .map(|i| &text[i + start_header.len()..])
        .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::InvalidData, format!("missing header '{}'", start_header.trim())))?;

    let content = if let Some(j) = after_start.find(end_header) {
        &after_start[..j]
    } else {
        after_start
    };

    Ok(content.trim_end_matches('\n').to_string())
}

/// Extract text after `start_header` to end of `text`.
fn extract_section_tail(text: &str, start_header: &str) -> std::io::Result<String> {
    let after_start = text
        .find(start_header)
        .map(|i| &text[i + start_header.len()..])
        .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::InvalidData, format!("missing header '{}'", start_header.trim())))?;

    Ok(after_start.trim_end_matches('\n').to_string())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use hone_proto::ToolLayer;
    use hone_tools::{Tool, ToolContext, ToolError, ToolInput, ToolOutput};
    use serde_json::Value;
    use tempfile::TempDir;

    // -----------------------------------------------------------------------
    // MockTool — two flavours: read-only and write.
    // -----------------------------------------------------------------------

    struct MockTool {
        name: &'static str,
        read_only: bool,
    }

    #[async_trait]
    impl Tool for MockTool {
        fn name(&self) -> &str {
            self.name
        }

        fn description(&self) -> &str {
            "mock tool"
        }

        fn schema(&self) -> Value {
            serde_json::json!({})
        }

        async fn call(
            &self,
            _input: ToolInput,
            _ctx: &ToolContext,
        ) -> Result<ToolOutput, ToolError> {
            Ok(ToolOutput::new(serde_json::json!({"ok": true})))
        }

        fn layer(&self) -> ToolLayer {
            ToolLayer::Native
        }

        fn is_read_only(&self) -> bool {
            self.read_only
        }
    }

    fn make_registry(tools: &[(&'static str, bool)]) -> ToolRegistry {
        let mut builder = ToolRegistry::builder();
        for &(name, read_only) in tools {
            builder = builder.register(Box::new(MockTool { name, read_only }));
        }
        builder.build()
    }

    fn make_plan(prompt: &str, body: &str) -> SavedPlan {
        SavedPlan {
            timestamp: "2026-05-02T18:30:00Z".to_string(),
            slug: slugify(prompt),
            user_prompt: prompt.to_string(),
            plan_body: body.to_string(),
        }
    }

    // -----------------------------------------------------------------------
    // AgentMode tests
    // -----------------------------------------------------------------------

    #[test]
    fn mode_cycle_build_to_plan() {
        assert_eq!(AgentMode::Build.cycle_next(), AgentMode::Plan);
    }

    #[test]
    fn mode_cycle_plan_to_auto_approve() {
        assert_eq!(AgentMode::Plan.cycle_next(), AgentMode::AutoApprove);
    }

    #[test]
    fn mode_cycle_auto_approve_to_build() {
        assert_eq!(AgentMode::AutoApprove.cycle_next(), AgentMode::Build);
    }

    #[test]
    fn mode_label_each_variant() {
        assert_eq!(AgentMode::Build.label(), "build");
        assert_eq!(AgentMode::Plan.label(), "plan");
        assert_eq!(AgentMode::AutoApprove.label(), "auto");
    }

    // -----------------------------------------------------------------------
    // slugify tests
    // -----------------------------------------------------------------------

    #[test]
    fn slugify_simple_phrase() {
        assert_eq!(slugify("Fix the auth bug"), "fix-the-auth-bug");
    }

    #[test]
    fn slugify_strips_punctuation() {
        assert_eq!(slugify("What's wrong with /tmp/x?!"), "what-s-wrong-with-tmp-x");
    }

    #[test]
    fn slugify_collapses_dashes() {
        assert_eq!(slugify("a   b"), "a-b");
    }

    #[test]
    fn slugify_trims_leading_trailing_dashes() {
        assert_eq!(slugify("  hello  "), "hello");
    }

    #[test]
    fn slugify_truncates_at_40_chars() {
        let long_input = "a".repeat(100);
        let result = slugify(&long_input);
        assert!(result.len() <= 40, "slug length {} > 40", result.len());
        assert!(
            !result.ends_with('-'),
            "slug must not end with a trailing dash"
        );
    }

    #[test]
    fn slugify_empty_input_returns_plan() {
        assert_eq!(slugify(""), "plan");
    }

    #[test]
    fn slugify_only_punctuation_returns_plan() {
        assert_eq!(slugify("!!!"), "plan");
    }

    // -----------------------------------------------------------------------
    // read_only_registry tests
    // -----------------------------------------------------------------------

    #[test]
    fn read_only_registry_keeps_only_read_only_tools() {
        let base = make_registry(&[("ro1", true), ("ro2", true), ("write1", false)]);
        let result = read_only_registry(&base);
        assert_eq!(result.len(), 2, "expected 2 read-only tools, got {}", result.len());
        assert!(result.get("ro1").is_some(), "ro1 must be present");
        assert!(result.get("ro2").is_some(), "ro2 must be present");
        assert!(result.get("write1").is_none(), "write1 must be excluded");
    }

    #[test]
    fn read_only_registry_empty_base_returns_empty() {
        let base = make_registry(&[]);
        let result = read_only_registry(&base);
        assert!(result.is_empty());
    }

    #[test]
    fn read_only_registry_all_read_only_passes_through_unchanged() {
        let base = make_registry(&[("a", true), ("b", true)]);
        let result = read_only_registry(&base);
        assert_eq!(result.len(), 2);
        assert!(result.get("a").is_some());
        assert!(result.get("b").is_some());
    }

    // -----------------------------------------------------------------------
    // PlanModeStore async tests
    // -----------------------------------------------------------------------

    fn temp_store() -> (TempDir, PlanModeStore) {
        let dir = TempDir::new().unwrap();
        let store = PlanModeStore::new(dir.path().join("plans"));
        (dir, store)
    }

    #[tokio::test]
    async fn save_writes_file_with_expected_filename_pattern() {
        let (_dir, store) = temp_store();
        let plan = make_plan("Fix the auth bug", "Step 1: look at auth.rs");
        let path = store.save(&plan).await.unwrap();
        let filename = path.file_name().unwrap().to_str().unwrap();
        // Must match YYYYMMDD-HHMMSS-<slug>.md
        let re = regex::Regex::new(r"^\d{8}-\d{6}-[a-z0-9-]+\.md$").unwrap();
        assert!(
            re.is_match(filename),
            "filename '{}' does not match expected pattern",
            filename
        );
    }

    #[tokio::test]
    async fn save_writes_file_under_base_dir() {
        let (dir, store) = temp_store();
        let plan = make_plan("Fix the auth bug", "Step 1: look at auth.rs");
        let path = store.save(&plan).await.unwrap();
        assert!(
            path.starts_with(dir.path()),
            "saved path {:?} must be inside tempdir {:?}",
            path,
            dir.path()
        );
    }

    #[tokio::test]
    async fn save_round_trips_via_load() {
        let (_dir, store) = temp_store();
        let plan = make_plan("Fix the auth bug", "Step 1: look at auth.rs\nStep 2: profit");
        let path = store.save(&plan).await.unwrap();
        let loaded = PlanModeStore::load(&path).await.unwrap();
        assert_eq!(loaded.timestamp, plan.timestamp);
        assert_eq!(loaded.slug, plan.slug);
        assert_eq!(loaded.user_prompt, plan.user_prompt);
        assert_eq!(loaded.plan_body, plan.plan_body);
    }

    #[tokio::test]
    async fn save_uses_slug_in_filename() {
        let (_dir, store) = temp_store();
        let plan = make_plan("Fix the auth bug", "body");
        let path = store.save(&plan).await.unwrap();
        let filename = path.file_name().unwrap().to_str().unwrap();
        assert!(
            filename.contains("fix-the-auth-bug"),
            "filename '{}' must contain slug 'fix-the-auth-bug'",
            filename
        );
    }

    #[tokio::test]
    async fn save_creates_dir_if_missing() {
        let dir = TempDir::new().unwrap();
        // Use a nested path that definitely does not exist yet.
        let store = PlanModeStore::new(dir.path().join("deep/nested/plans"));
        let plan = make_plan("Test plan", "body");
        let result = store.save(&plan).await;
        assert!(result.is_ok(), "save must succeed even when base_dir is missing");
        assert!(result.unwrap().exists());
    }

    #[tokio::test]
    async fn save_handles_special_chars_in_prompt() {
        let (_dir, store) = temp_store();
        let prompt = "Fix \"quoted\" stuff\nwith newlines & Unicode: \u{1F600}";
        let body = "The plan is:\n1. Do X\n2. Do Y";
        let plan = SavedPlan {
            timestamp: "2026-05-02T18:30:00Z".to_string(),
            slug: slugify(prompt),
            user_prompt: prompt.to_string(),
            plan_body: body.to_string(),
        };
        let path = store.save(&plan).await.unwrap();
        let loaded = PlanModeStore::load(&path).await.unwrap();
        assert_eq!(loaded.user_prompt, prompt);
        assert_eq!(loaded.plan_body, body);
    }

    #[tokio::test]
    async fn list_returns_files_newest_first() {
        let (_dir, store) = temp_store();
        // Save three plans with increasing timestamps embedded in the filename.
        // We control the timestamp field in SavedPlan so the implementer uses
        // it when constructing the YYYYMMDD-HHMMSS prefix in the filename.
        let plans = [
            SavedPlan {
                timestamp: "2026-05-01T10:00:00Z".to_string(),
                slug: "alpha".to_string(),
                user_prompt: "alpha".to_string(),
                plan_body: "body alpha".to_string(),
            },
            SavedPlan {
                timestamp: "2026-05-02T10:00:00Z".to_string(),
                slug: "beta".to_string(),
                user_prompt: "beta".to_string(),
                plan_body: "body beta".to_string(),
            },
            SavedPlan {
                timestamp: "2026-05-03T10:00:00Z".to_string(),
                slug: "gamma".to_string(),
                user_prompt: "gamma".to_string(),
                plan_body: "body gamma".to_string(),
            },
        ];
        for p in &plans {
            store.save(p).await.unwrap();
        }

        let listed = store.list().await.unwrap();
        assert_eq!(listed.len(), 3);
        // Filenames encode date prefix; descending sort => gamma, beta, alpha.
        let names: Vec<&str> = listed
            .iter()
            .map(|p| p.file_name().unwrap().to_str().unwrap())
            .collect();
        assert!(
            names[0].contains("gamma"),
            "first entry must be newest (gamma), got {:?}",
            names
        );
        assert!(
            names[2].contains("alpha"),
            "last entry must be oldest (alpha), got {:?}",
            names
        );
    }

    #[tokio::test]
    async fn list_skips_non_md_files() {
        let (_dir, store) = temp_store();
        let plan = make_plan("Some plan", "body");
        store.save(&plan).await.unwrap();

        // Drop a non-.md file in the plans dir.
        let junk = store.base_dir.join("junk.txt");
        tokio::fs::write(&junk, "ignore me").await.unwrap();

        let listed = store.list().await.unwrap();
        for p in &listed {
            assert!(
                p.extension().map_or(false, |e| e == "md"),
                "list must only contain .md files, found {:?}",
                p
            );
        }
    }

    #[tokio::test]
    async fn list_missing_dir_returns_empty_vec() {
        let dir = TempDir::new().unwrap();
        let store = PlanModeStore::new(dir.path().join("nonexistent"));
        let listed = store.list().await.unwrap();
        assert!(listed.is_empty());
    }

    // -----------------------------------------------------------------------
    // SavedPlan serde round-trip
    // -----------------------------------------------------------------------

    #[test]
    fn saved_plan_serde_round_trip() {
        let plan = SavedPlan {
            timestamp: "2026-05-02T18:30:00Z".to_string(),
            slug: "fix-auth-bug".to_string(),
            user_prompt: "Fix the auth bug".to_string(),
            plan_body: "Step 1: investigate\nStep 2: fix".to_string(),
        };
        let json = serde_json::to_string(&plan).expect("serialization must succeed");
        let restored: SavedPlan =
            serde_json::from_str(&json).expect("deserialization must succeed");
        assert_eq!(restored, plan);
    }
}
