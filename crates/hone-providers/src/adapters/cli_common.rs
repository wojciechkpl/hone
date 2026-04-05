use std::pin::Pin;
use std::process::Stdio;

use futures::{Stream, stream};
use hone_proto::{Message, ProviderEvent, ProviderHealth, Role};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;
use tokio::sync::mpsc;

/// Check if a CLI binary is available on PATH.
pub async fn check_binary_available(binary: &str) -> ProviderHealth {
    let result = Command::new("sh")
        .arg("-c")
        .arg(format!("command -v {binary}"))
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .await;

    match result {
        Ok(status) if status.success() => ProviderHealth::Healthy,
        Ok(_) => ProviderHealth::Unavailable {
            reason: format!("binary '{binary}' not found on PATH"),
        },
        Err(e) => ProviderHealth::Unavailable {
            reason: format!("failed to check binary '{binary}': {e}"),
        },
    }
}

/// Build a prompt string from a list of `Message`s.
///
/// Each message is prefixed with its role label so the CLI sees a clear
/// conversation structure.
pub fn messages_to_prompt(messages: &[Message]) -> String {
    let mut parts = Vec::with_capacity(messages.len());

    for msg in messages {
        let role_label = match msg.role {
            Role::System => "SYSTEM",
            Role::User => "USER",
            Role::Assistant => "ASSISTANT",
            Role::Tool => "TOOL",
        };
        let text = msg.content.as_text();
        parts.push(format!("{role_label}:\n{text}"));
    }

    parts.join("\n\n")
}

/// Estimate token count using a 4-chars-per-token heuristic.
pub fn estimate_tokens_heuristic(messages: &[Message]) -> u32 {
    let total_chars: usize = messages.iter().map(|m| m.content.as_text().len()).sum();
    ((total_chars / 4) as u32).max(1)
}

/// Spawn a CLI process and return a stream of `ProviderEvent`s parsed from stdout.
///
/// Each non-empty line of stdout becomes a `Token` event. When the process
/// exits with a non-zero status an `Error` event is emitted before the final
/// `Done`.
pub fn spawn_cli_stream(mut cmd: Command) -> Pin<Box<dyn Stream<Item = ProviderEvent> + Send>> {
    let (tx, rx) = mpsc::channel::<ProviderEvent>(64);

    tokio::spawn(async move {
        let child_result = cmd.stdout(Stdio::piped()).stderr(Stdio::null()).spawn();

        let mut child = match child_result {
            Ok(c) => c,
            Err(e) => {
                let _ = tx
                    .send(ProviderEvent::Error {
                        message: format!("failed to spawn process: {e}"),
                    })
                    .await;
                let _ = tx.send(ProviderEvent::Done).await;
                return;
            }
        };

        let stdout = match child.stdout.take() {
            Some(s) => s,
            None => {
                let _ = tx
                    .send(ProviderEvent::Error {
                        message: "failed to capture process stdout".to_owned(),
                    })
                    .await;
                let _ = tx.send(ProviderEvent::Done).await;
                return;
            }
        };

        let mut lines = BufReader::new(stdout).lines();

        loop {
            match lines.next_line().await {
                Ok(Some(line)) => {
                    let trimmed = line.trim().to_owned();
                    if !trimmed.is_empty()
                        && tx
                            .send(ProviderEvent::Token { text: trimmed })
                            .await
                            .is_err()
                        {
                            // Receiver dropped — stop reading.
                            return;
                        }
                }
                Ok(None) => break, // EOF
                Err(e) => {
                    let _ = tx
                        .send(ProviderEvent::Error {
                            message: format!("error reading process output: {e}"),
                        })
                        .await;
                    break;
                }
            }
        }

        // Wait for the process to exit and report non-zero status.
        match child.wait().await {
            Ok(status) if !status.success() => {
                let code = status.code().unwrap_or(-1);
                let _ = tx
                    .send(ProviderEvent::Error {
                        message: format!("process exited with status {code}"),
                    })
                    .await;
            }
            _ => {}
        }

        let _ = tx.send(ProviderEvent::Done).await;
    });

    // Convert the tokio mpsc::Receiver into a Stream using futures::stream::unfold.
    Box::pin(stream::unfold(rx, |mut rx| async move {
        let event = rx.recv().await?;
        Some((event, rx))
    }))
}
