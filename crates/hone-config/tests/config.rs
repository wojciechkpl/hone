#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

use hone_config::{Config, ConfigError, HoneMd, SandboxMode};
use std::path::PathBuf;

// ── helpers ──────────────────────────────────────────────────────────────────

fn fixtures_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")
}

// ── Config::default() ────────────────────────────────────────────────────────

#[test]
fn test_default_config_is_valid() {
    let cfg = Config::default();

    // Server defaults
    assert_eq!(cfg.server.host, "127.0.0.1");
    assert_eq!(cfg.server.port, 3117);
    assert!(cfg.server.auth_token.is_none());

    // Sandbox defaults
    assert!(matches!(cfg.sandbox.mode, SandboxMode::Manual));
    assert!(cfg.sandbox.allowed_commands.is_empty());
    assert!(cfg.sandbox.allowed_network_domains.is_empty());
    assert!(cfg.sandbox.protected_paths.is_empty());

    // No default model or budget
    assert!(cfg.default_model.is_none());
    assert!(cfg.budget.is_none());

    // Empty providers and routing
    assert!(cfg.providers.is_empty());
    assert!(cfg.routing.is_empty());
}

// ── TOML parsing ─────────────────────────────────────────────────────────────

#[test]
fn test_parse_minimal_toml() {
    let toml = r#"
        [server]
        host = "127.0.0.1"
        port = 3117
    "#;

    let cfg = Config::from_toml_str(toml).expect("minimal toml should parse");
    assert_eq!(cfg.server.host, "127.0.0.1");
    assert_eq!(cfg.server.port, 3117);
    assert!(cfg.default_model.is_none());
}

#[test]
fn test_parse_provider_block() {
    let toml = r#"
        [providers.anthropic]
        api_key_env = "ANTHROPIC_API_KEY"
        base_url = "https://api.anthropic.com"
        model = "claude-opus-4-5"
    "#;

    let cfg = Config::from_toml_str(toml).expect("provider toml should parse");
    let provider = cfg
        .providers
        .get("anthropic")
        .expect("anthropic provider should exist");
    assert_eq!(provider.api_key_env.as_deref(), Some("ANTHROPIC_API_KEY"));
    assert_eq!(
        provider.base_url.as_deref(),
        Some("https://api.anthropic.com")
    );
    assert_eq!(provider.model.as_deref(), Some("claude-opus-4-5"));
}

#[test]
fn test_parse_sandbox_config() {
    let toml = r#"
        [sandbox]
        mode = "Supervised"
        allowed_commands = ["ls", "cat", "grep"]
        allowed_network_domains = ["api.anthropic.com"]
        protected_paths = ["/etc", "/usr"]
    "#;

    let cfg = Config::from_toml_str(toml).expect("sandbox toml should parse");
    assert!(matches!(cfg.sandbox.mode, SandboxMode::Supervised));
    assert_eq!(cfg.sandbox.allowed_commands, vec!["ls", "cat", "grep"]);
    assert_eq!(
        cfg.sandbox.allowed_network_domains,
        vec!["api.anthropic.com"]
    );
    assert_eq!(
        cfg.sandbox.protected_paths,
        vec![PathBuf::from("/etc"), PathBuf::from("/usr")]
    );
}

#[test]
fn test_parse_routing_rules() {
    let toml = r#"
        [[routing]]
        task = "file_read"
        model = "anthropic:claude-haiku-3-5"

        [[routing]]
        task = "multi_file_edit"
        model = "anthropic:claude-opus-4-5"
    "#;

    let cfg = Config::from_toml_str(toml).expect("routing toml should parse");
    assert_eq!(cfg.routing.len(), 2);
    assert_eq!(cfg.routing[0].task, "file_read");
    assert_eq!(cfg.routing[0].model, "anthropic:claude-haiku-3-5");
    assert_eq!(cfg.routing[1].task, "multi_file_edit");
    assert_eq!(cfg.routing[1].model, "anthropic:claude-opus-4-5");
}

#[test]
fn test_parse_budget_config() {
    let toml = r#"
        [budget]
        daily_limit_usd = 10.0
        session_limit_usd = 2.0
    "#;

    let cfg = Config::from_toml_str(toml).expect("budget toml should parse");
    let budget = cfg.budget.expect("budget should be present");
    assert_eq!(budget.daily_limit_usd, Some(10.0));
    assert_eq!(budget.session_limit_usd, Some(2.0));
}

// ── Fixture files ─────────────────────────────────────────────────────────────

#[test]
fn test_parse_minimal_fixture() {
    let path = fixtures_dir().join("minimal.toml");
    let toml = std::fs::read_to_string(&path).unwrap();
    let cfg = Config::from_toml_str(&toml).expect("minimal fixture should parse");
    assert_eq!(cfg.server.host, "127.0.0.1");
    assert_eq!(cfg.server.port, 3117);
}

#[test]
fn test_parse_full_fixture() {
    let path = fixtures_dir().join("full.toml");
    let toml = std::fs::read_to_string(&path).unwrap();
    let cfg = Config::from_toml_str(&toml).expect("full fixture should parse");

    assert_eq!(
        cfg.default_model.as_ref().map(|m| m.as_str()),
        Some("anthropic:claude-opus-4-5")
    );
    assert_eq!(cfg.providers.len(), 2);
    assert!(matches!(cfg.sandbox.mode, SandboxMode::Supervised));
    assert_eq!(cfg.routing.len(), 2);
    assert!(cfg.budget.is_some());
    assert_eq!(cfg.server.auth_token.as_deref(), Some("super-secret-token"));
}

// ── Config layered merging ────────────────────────────────────────────────────

#[test]
fn test_merge_project_over_global() {
    let global_toml = r#"
        default_model = "openai:gpt-4o"

        [server]
        host = "127.0.0.1"
        port = 3117

        [sandbox]
        mode = "Manual"
    "#;

    let project_toml = r#"
        default_model = "anthropic:claude-opus-4-5"

        [sandbox]
        mode = "Supervised"
    "#;

    let cfg = Config::load_from_str(global_toml, project_toml).expect("layered config should load");

    // project overrides default_model
    assert_eq!(
        cfg.default_model.as_ref().map(|m| m.as_str()),
        Some("anthropic:claude-opus-4-5")
    );
    // project overrides sandbox mode
    assert!(matches!(cfg.sandbox.mode, SandboxMode::Supervised));
    // global server settings survive
    assert_eq!(cfg.server.host, "127.0.0.1");
    assert_eq!(cfg.server.port, 3117);
}

// ── Unknown fields ────────────────────────────────────────────────────────────

#[test]
fn test_unknown_toml_key_errors() {
    let toml = r#"
        totally_unknown_field = "this should not be allowed"

        [server]
        host = "127.0.0.1"
        port = 3117
    "#;

    let result = Config::from_toml_str(toml);
    assert!(
        result.is_err(),
        "unknown top-level fields should cause a parse error"
    );
}

// ── HoneMd parsing ────────────────────────────────────────────────────────────

#[test]
fn test_hone_md_extracts_sections() {
    let path = fixtures_dir().join("sample_hone.md");
    let markdown = std::fs::read_to_string(&path).unwrap();
    let hone_md = HoneMd::parse(&markdown);

    assert!(
        hone_md.sections.contains_key("TOOLS"),
        "should have TOOLS section"
    );
    assert!(
        hone_md.sections.contains_key("STYLE"),
        "should have STYLE section"
    );
    assert!(
        hone_md.sections.contains_key("CONSTRAINTS"),
        "should have CONSTRAINTS section"
    );
    assert!(
        hone_md.sections.contains_key("CONTEXT"),
        "should have CONTEXT section"
    );

    let tools = &hone_md.sections["TOOLS"];
    assert!(
        tools.contains("file_read"),
        "TOOLS section should contain 'file_read'"
    );

    let style = &hone_md.sections["STYLE"];
    assert!(style.contains("Rust"), "STYLE section should mention Rust");
}

#[test]
fn test_hone_md_empty() {
    let hone_md = HoneMd::parse("");
    assert!(
        hone_md.sections.is_empty(),
        "empty markdown should produce no sections"
    );
}

#[test]
fn test_hone_md_no_headings() {
    let markdown = "Just some plain text with no headings.\nAnother line.";
    let hone_md = HoneMd::parse(markdown);
    assert!(
        hone_md.sections.is_empty(),
        "markdown without h2 headings should produce no sections"
    );
}

#[test]
fn test_hone_md_ignores_h1_and_h3() {
    let markdown = r#"
# Top-level heading (ignored)

## SECTION_A

Content under section A.

### Sub-heading (not a section key)

More content.

## SECTION_B

Content under section B.
"#;

    let hone_md = HoneMd::parse(markdown);
    // Only h2 headings become section keys
    assert_eq!(
        hone_md.sections.len(),
        2,
        "only h2 headings should be section keys"
    );
    assert!(hone_md.sections.contains_key("SECTION_A"));
    assert!(hone_md.sections.contains_key("SECTION_B"));
}

#[test]
fn test_hone_md_section_content_excludes_heading() {
    let markdown = "## MY_SECTION\n\nThe content here.\n";
    let hone_md = HoneMd::parse(markdown);
    let content = hone_md
        .sections
        .get("MY_SECTION")
        .expect("section should exist");
    assert!(
        !content.contains("## MY_SECTION"),
        "section content should not include the heading itself"
    );
    assert!(content.contains("The content here."));
}

// ── Config::load (file-based) ─────────────────────────────────────────────────

#[test]
fn test_load_from_paths() {
    use tempfile::TempDir;

    let tmp = TempDir::new().unwrap();

    let global_path = tmp.path().join("global.toml");
    let project_path = tmp.path().join("project.toml");

    std::fs::write(&global_path, b"[server]\nhost = \"0.0.0.0\"\nport = 9000\n").unwrap();
    std::fs::write(&project_path, b"[server]\nport = 4000\n").unwrap();

    let cfg =
        Config::load(Some(&global_path), Some(&project_path)).expect("should load from file paths");

    // project overrides port, global sets host
    assert_eq!(cfg.server.port, 4000);
    assert_eq!(cfg.server.host, "0.0.0.0");
}

#[test]
fn test_load_with_no_files_returns_defaults() {
    let cfg = Config::load(None, None).expect("load with no files should return defaults");
    assert_eq!(cfg.server.host, "127.0.0.1");
    assert_eq!(cfg.server.port, 3117);
}

#[test]
fn test_config_error_includes_file_path() {
    use tempfile::TempDir;

    let tmp = TempDir::new().unwrap();
    let bad_path = tmp.path().join("bad.toml");

    // Write intentionally malformed TOML so parsing fails.
    std::fs::write(&bad_path, b"[server\nthis is not valid toml{{{{").unwrap();

    let err =
        Config::load(Some(&bad_path), None).expect_err("malformed TOML should produce an error");

    let err_msg = err.to_string();
    assert!(
        err_msg.contains(bad_path.to_string_lossy().as_ref()),
        "error message should contain the file path; got: {err_msg}"
    );
    assert!(
        matches!(err, ConfigError::InvalidField { .. }),
        "expected ConfigError::InvalidField, got: {err:?}"
    );
}
