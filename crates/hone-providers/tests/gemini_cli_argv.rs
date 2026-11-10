#![cfg(unix)]
#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]
//! R3 (`docs/plans/config-apply-spec.md` section 3b): the Gemini CLI adapter passes the model as
//! ONE token, `--model=<name>`, so a name can never be read as a separate flag.
//!
//! A fake `gemini` script records its argv, one argument per line.

use std::os::unix::fs::PermissionsExt;
use std::time::Duration;

use futures::StreamExt;
use hone_proto::Message;
use hone_providers::{CompletionRequest, GeminiCliProvider, ModelProvider};

async fn argv_for(model: &str) -> Vec<String> {
    let dir = tempfile::tempdir().unwrap();
    let argfile = dir.path().join("argv.txt");
    let script = dir.path().join("fake-gemini");
    std::fs::write(
        &script,
        format!(
            "#!/bin/sh\nfor a in \"$@\"; do printf '%s\\n' \"$a\"; done > '{}'\ncat > /dev/null\necho ok\n",
            argfile.display()
        ),
    )
    .unwrap();
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();

    let provider = GeminiCliProvider::with_binary(script.to_str().unwrap());
    let req = CompletionRequest::new(model, vec![Message::user("hi")]);
    let stream = provider.complete(req).await.expect("spawn");
    let _events: Vec<_> = tokio::time::timeout(Duration::from_secs(20), stream.collect())
        .await
        .expect("fake gemini did not finish");
    std::fs::read_to_string(&argfile)
        .unwrap_or_default()
        .lines()
        .map(str::to_owned)
        .collect()
}

#[tokio::test]
async fn test_r3_model_is_passed_as_a_single_equals_token() {
    let argv = argv_for("gemini-cli:gemini-2.5-pro").await;
    assert!(argv.contains(&"--model=gemini-2.5-pro".to_owned()), "{argv:?}");
    assert!(!argv.contains(&"-m".to_owned()), "no separate -m flag: {argv:?}");
    assert!(!argv.contains(&"--model".to_owned()), "no separate --model flag: {argv:?}");
}

#[tokio::test]
async fn test_r3_a_flag_shaped_model_name_stays_inside_its_token() {
    let argv = argv_for("gemini-cli:--yolo").await;
    assert!(argv.contains(&"--model=--yolo".to_owned()), "{argv:?}");
    assert!(!argv.contains(&"--yolo".to_owned()), "must not become its own flag: {argv:?}");
}
