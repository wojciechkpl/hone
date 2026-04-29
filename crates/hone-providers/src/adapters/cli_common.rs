use std::pin::Pin;
use std::process::Stdio;

use futures::{Stream, stream};
use hone_proto::{Message, ProviderEvent, ProviderHealth, Role};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;
use tokio::sync::mpsc;

/// Check if a CLI binary is available on PATH.
///
/// Uses `which` directly instead of `sh -c "command -v ..."` to avoid
/// shell metacharacter injection if `binary` ever comes from config.
pub async fn check_binary_available(binary: &str) -> ProviderHealth {
    // Reject names that look like paths or contain shell metacharacters.
    if binary.is_empty() || binary.contains(['/', '\0', ';', '&', '|']) {
        return ProviderHealth::Unavailable {
            reason: format!("invalid binary name: '{binary}'"),
        };
    }

    let result = Command::new("which")
        .arg(binary)
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
/// conversation structure.  Tool calls on assistant messages and tool result
/// IDs are appended so the CLI receives the full turn.
pub fn messages_to_prompt(messages: &[Message]) -> String {
    let mut parts = Vec::with_capacity(messages.len());

    for msg in messages {
        let role_label = match msg.role {
            Role::System => "SYSTEM",
            Role::User => "USER",
            Role::Assistant => "ASSISTANT",
            Role::Tool => "TOOL",
            // Forward-compat: unknown roles are labelled generically.
            _ => "UNKNOWN",
        };
        let mut text = msg.content.as_text().to_owned();
        // Append tool calls for assistant messages so the CLI sees the full turn.
        for tc in &msg.tool_calls {
            let args = serde_json::to_string(&tc.arguments).unwrap_or_default();
            text.push_str(&format!(
                "\n[tool_use id={} name={} input={}]",
                tc.id, tc.name, args
            ));
        }
        // For tool result messages, include the call ID for traceability.
        if let Some(ref id) = msg.tool_call_id {
            text = format!("[tool_result id={id}]\n{text}");
        }
        parts.push(format!("{role_label}:\n{text}"));
    }

    parts.join("\n\n")
}

/// Estimate token count using a 4-chars-per-token heuristic.
pub fn estimate_tokens_heuristic(messages: &[Message]) -> u32 {
    let total_chars: usize = messages.iter().map(|m| m.content.as_text().len()).sum();
    ((total_chars / 4) as u32).max(1)
}

/// Spawn a CLI process, pipe `prompt` via stdin, and return a stream of
/// `ProviderEvent`s parsed from stdout.
///
/// The prompt is written to stdin (then closed) instead of being passed as a
/// CLI argument to avoid OS arg-length limits and exposure in `ps` output.
///
/// Each non-empty line of stdout becomes a `Token` event. When the process
/// exits with a non-zero status an `Error` event is emitted before the final
/// `Done`.
pub fn spawn_cli_stream_with_stdin(
    mut cmd: Command,
    prompt: String,
) -> Pin<Box<dyn Stream<Item = ProviderEvent> + Send>> {
    let (tx, rx) = mpsc::channel::<ProviderEvent>(64);

    tokio::spawn(async move {
        let child_result = cmd
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn();

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

        // Pipe prompt via stdin and close.
        if let Some(mut stdin) = child.stdin.take() {
            use tokio::io::AsyncWriteExt;
            if let Err(e) = stdin.write_all(prompt.as_bytes()).await {
                let _ = tx
                    .send(ProviderEvent::Error {
                        message: format!("failed to write prompt to stdin: {e}"),
                    })
                    .await;
                let _ = tx.send(ProviderEvent::Done).await;
                return;
            }
            drop(stdin);
        }

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
                        return;
                    }
                }
                Ok(None) => break,
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

    Box::pin(stream::unfold(rx, |mut rx| async move {
        let event = rx.recv().await?;
        Some((event, rx))
    }))
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
