use std::path::PathBuf;
use std::sync::Arc;

use hone_tools::lsp::{
    AutoFixLoop, CodeAction, Diagnostic, DiagnosticSeverity, Language,
    LspDiagnosticsTool, LspHoverTool, MockLspBridge, detect_language,
};
use hone_tools::{Tool, ToolContext, ToolInput};
use hone_sandbox::NullSandbox;
use serde_json::json;

fn make_ctx() -> ToolContext {
    ToolContext {
        sandbox: Arc::new(NullSandbox),
        working_dir: PathBuf::from("/"),
    }
}

fn make_diagnostic(file: &str, msg: &str) -> Diagnostic {
    Diagnostic {
        file: PathBuf::from(file),
        line: 0,
        col: 0,
        severity: DiagnosticSeverity::Error,
        message: msg.to_string(),
        code: None,
    }
}

// ---------------------------------------------------------------------------
// Language detection
// ---------------------------------------------------------------------------

#[test]
fn test_language_detection_from_file_extensions() {
    assert_eq!(detect_language(&PathBuf::from("main.rs")), Language::Rust);
    assert_eq!(detect_language(&PathBuf::from("script.py")), Language::Python);
    assert_eq!(detect_language(&PathBuf::from("app.ts")), Language::TypeScript);
    assert_eq!(detect_language(&PathBuf::from("index.js")), Language::JavaScript);
    assert_eq!(detect_language(&PathBuf::from("main.go")), Language::Go);
    assert_eq!(detect_language(&PathBuf::from("data.xyz")), Language::Unknown);
}

// ---------------------------------------------------------------------------
// LspDiagnosticsTool
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_lsp_diagnostics_returns_mock_results() {
    let diag1 = make_diagnostic("foo.rs", "unused variable");
    let diag2 = make_diagnostic("foo.rs", "type mismatch");

    let bridge = MockLspBridge::builder()
        .diagnostics(vec![diag1.clone(), diag2.clone()])
        .build();

    let tool = LspDiagnosticsTool::new(Arc::new(bridge));
    let input = ToolInput::new(json!({ "file": "foo.rs" }));
    let ctx = make_ctx();

    let output = tool.call(input, &ctx).await.expect("tool call failed");
    let value = output.into_value();

    let array = value.as_array().expect("expected JSON array");
    assert_eq!(array.len(), 2, "expected exactly 2 diagnostics");

    assert_eq!(array[0]["message"], "unused variable");
    assert_eq!(array[1]["message"], "type mismatch");
}

// ---------------------------------------------------------------------------
// LspHoverTool
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_lsp_hover_returns_text() {
    let expected_hover = "fn process(data: &[u8]) -> Result<Output>";

    let bridge = MockLspBridge::builder()
        .hover_text(expected_hover)
        .build();

    let tool = LspHoverTool::new(Arc::new(bridge));
    let input = ToolInput::new(json!({ "file": "lib.rs", "line": 10, "col": 4 }));
    let ctx = make_ctx();

    let output = tool.call(input, &ctx).await.expect("tool call failed");
    let value = output.into_value();

    let hover_text = value["hover"]
        .as_str()
        .expect("expected hover to be a string");
    assert_eq!(hover_text, expected_hover);
}

// ---------------------------------------------------------------------------
// AutoFixLoop
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_auto_fix_loop_applies_fixes() {
    // First call: one error, second call: no errors.
    let diag = make_diagnostic("main.rs", "unused import");

    let bridge = MockLspBridge::builder()
        .diagnostics_sequence(vec![
            vec![diag],
            vec![],
        ])
        .code_actions(vec![CodeAction {
            title: "Remove unused import".to_string(),
            kind: Some("quickfix".to_string()),
        }])
        .build();

    let loop_ = AutoFixLoop::new(Box::new(bridge), 3);
    let result = loop_
        .run(&PathBuf::from("main.rs"))
        .await
        .expect("auto-fix failed");

    assert_eq!(result.iterations, 1, "expected exactly 1 iteration");
    assert!(
        result.remaining_errors.is_empty(),
        "expected no remaining errors"
    );
    assert_eq!(result.applied_fixes, vec!["Remove unused import"]);
}

#[tokio::test]
async fn test_auto_fix_stops_after_max_retries() {
    // The bridge always returns the same error — fixes never actually work.
    let diag = make_diagnostic("main.rs", "type mismatch");

    let bridge = MockLspBridge::builder()
        .diagnostics(vec![diag])
        .code_actions(vec![CodeAction {
            title: "Change type".to_string(),
            kind: Some("quickfix".to_string()),
        }])
        .build();

    let loop_ = AutoFixLoop::new(Box::new(bridge), 3);
    let result = loop_
        .run(&PathBuf::from("main.rs"))
        .await
        .expect("auto-fix failed");

    assert_eq!(result.iterations, 3, "expected exactly 3 iterations");
    assert_eq!(
        result.remaining_errors.len(),
        1,
        "expected 1 remaining error"
    );
}
