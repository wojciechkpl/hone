use std::sync::Arc;

use hone_core::CodeModeRunner;
use hone_sandbox::NullSandbox;

fn runner() -> CodeModeRunner {
    let working_dir = std::env::temp_dir();
    CodeModeRunner::new(Arc::new(NullSandbox), working_dir)
}

#[tokio::test]
async fn test_code_mode_script_is_sandboxed() {
    let working_dir = std::env::temp_dir();
    let runner = CodeModeRunner::new(Arc::new(NullSandbox), working_dir.clone());

    let script = format!(
        "touch {}/hone_code_mode_sandbox_test_marker",
        working_dir.display()
    );

    let result = runner.execute(&script, "sh").await;
    assert!(
        result.is_ok(),
        "NullSandbox should allow execution: {result:?}"
    );

    let output = result.unwrap();
    assert_eq!(output.exit_code, 0);
}

#[tokio::test]
async fn test_code_mode_output_is_captured() {
    let output = runner()
        .execute(r#"echo "hello from script""#, "sh")
        .await
        .expect("execution should succeed");

    assert_eq!(output.exit_code, 0);
    assert!(
        output.stdout.contains("hello from script"),
        "stdout was: {:?}",
        output.stdout
    );
}

#[tokio::test]
async fn test_code_mode_stderr_captured() {
    let output = runner()
        .execute(r#"echo "err" >&2"#, "sh")
        .await
        .expect("execution should succeed");

    assert_eq!(output.exit_code, 0);
    assert!(
        output.stderr.contains("err"),
        "stderr was: {:?}",
        output.stderr
    );
}

#[tokio::test]
async fn test_code_mode_nonzero_exit() {
    let output = runner()
        .execute("exit 42", "sh")
        .await
        .expect("execution should succeed");

    assert_eq!(output.exit_code, 42);
}
