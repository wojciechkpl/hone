use std::path::PathBuf;
use std::sync::Arc;

use hone_core::{Agent, AgentConfig, TestStatus, TestVerificationLoop};
use hone_proto::{ModelId, PricingEntry};
use hone_providers::MockProvider;
use hone_sandbox::NullSandbox;
use hone_tools::ToolRegistry;
use tokio::sync::mpsc;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn base_config(provider: Arc<dyn hone_providers::ModelProvider>) -> AgentConfig {
    AgentConfig {
        provider,
        tools: ToolRegistry::new(),
        sandbox: Arc::new(NullSandbox),
        system_prompt: "You are a helpful coder.".to_owned(),
        model: ModelId::new("mock:test"),
        max_tokens: None,
        temperature: None,
        budget_limit: None,
        pricing: PricingEntry {
            model: ModelId::new("mock:test"),
            input_per_million: 0.0,
            output_per_million: 0.0,
            cache_read_per_million: None,
        },
        working_dir: PathBuf::from("."),
    }
}

fn make_agent(response: &str) -> Agent {
    let provider = Arc::new(MockProvider::new(vec![response.to_owned()]));
    Agent::new(base_config(provider))
}

/// Write a tiny shell script to a temp file and return the path + command.
///
/// `exit_code_sequence` lists the exit codes the script should return on
/// successive invocations.  A counter file in the temp dir tracks calls.
fn script_that_exits(tmp: &tempfile::TempDir, exit_codes: &[i32]) -> (PathBuf, String) {
    // Serialise the codes as a space-separated list embedded in the script.
    let codes_str: String = exit_codes
        .iter()
        .map(|c| c.to_string())
        .collect::<Vec<_>>()
        .join(" ");

    let counter_path = tmp.path().join("counter.txt");
    let script_path = tmp.path().join("test.sh");

    // The script reads a counter from a file, looks up the n-th exit code from
    // the embedded list, increments the counter, then exits with that code.
    let script_body = format!(
        r#"#!/bin/sh
CODES=({codes_str})
COUNTER_FILE="{counter}"
if [ -f "$COUNTER_FILE" ]; then
    IDX=$(cat "$COUNTER_FILE")
else
    IDX=0
fi
# Clamp to the last code if we've run out.
LEN=${{#CODES[@]}}
if [ "$IDX" -ge "$LEN" ]; then
    IDX=$((LEN - 1))
fi
EXIT_CODE=${{CODES[$IDX]}}
NEXT=$((IDX + 1))
echo "$NEXT" > "$COUNTER_FILE"
exit $EXIT_CODE
"#,
        counter = counter_path.display()
    );

    std::fs::write(&script_path, &script_body).expect("write script");

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&script_path, std::fs::Permissions::from_mode(0o755))
            .expect("chmod");
    }

    let cmd = format!("bash {}", script_path.display());
    (script_path, cmd)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

/// If the test command exits 0 on the first attempt the loop returns
/// `TestStatus::Passed` with `iterations = 1` without touching the agent.
#[tokio::test]
async fn test_verification_loop_passes_immediately() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let (_, cmd) = script_that_exits(&tmp, &[0]);

    let mut agent = make_agent("fixed code");
    let (tx, _rx) = mpsc::channel(32);

    let loop_ = TestVerificationLoop::new(cmd, 3, tmp.path().to_owned());
    let result = loop_.run(&mut agent, tx).await.expect("loop should succeed");

    assert_eq!(result.iterations, 1);
    assert!(matches!(result.final_status, TestStatus::Passed));
}

/// If the test command fails on the first call but passes on the second the
/// loop should report `iterations = 2`.
#[tokio::test]
async fn test_verification_loop_retries_on_failure() {
    let tmp = tempfile::tempdir().expect("tempdir");
    // Fail on call 1, pass on call 2.
    let (_, cmd) = script_that_exits(&tmp, &[1, 0]);

    let mut agent = make_agent("fixed code");
    let (tx, _rx) = mpsc::channel(32);

    let loop_ = TestVerificationLoop::new(cmd, 3, tmp.path().to_owned());
    let result = loop_.run(&mut agent, tx).await.expect("loop should succeed");

    assert_eq!(result.iterations, 2);
    assert!(matches!(result.final_status, TestStatus::Passed));
}

/// When the test command always fails the loop exhausts `max_iterations` and
/// returns `TestStatus::FailedAfterMax`.
#[tokio::test]
async fn test_verification_loop_gives_up_after_max() {
    let tmp = tempfile::tempdir().expect("tempdir");
    // Always fail.
    let (_, cmd) = script_that_exits(&tmp, &[1, 1, 1, 1, 1]);

    let mut agent = make_agent("I tried but still broken");
    let (tx, _rx) = mpsc::channel(32);

    let loop_ = TestVerificationLoop::new(cmd, 3, tmp.path().to_owned());
    let result = loop_.run(&mut agent, tx).await.expect("loop should succeed");

    assert_eq!(result.iterations, 3);
    assert!(matches!(result.final_status, TestStatus::FailedAfterMax { .. }));
}
