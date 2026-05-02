use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use thiserror::Error;
use tokio::io::AsyncWriteExt;

// ---------------------------------------------------------------------------
// Error type
// ---------------------------------------------------------------------------

#[derive(Debug, Error)]
pub enum HookError {
    #[error("io error: {0}")]
    Io(#[from] io::Error),
    #[error("toml parse error: {0}")]
    Toml(String),
    #[error("unknown event: {0}")]
    InvalidEvent(String),
}

// ---------------------------------------------------------------------------
// HookEvent
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum HookEvent {
    PreToolUse {
        tool_name: String,
        args: serde_json::Value,
    },
    PostToolUse {
        tool_name: String,
        args: serde_json::Value,
        result: serde_json::Value,
        is_error: bool,
        duration_ms: u64,
    },
    UserPromptSubmit {
        prompt: String,
    },
    SessionStart {
        session_id: String,
    },
    SessionEnd {
        session_id: String,
    },
    Stop {
        turn_summary: String,
    },
}

impl HookEvent {
    /// Return the string name used to match against `HookSpec::event`.
    pub fn event_name(&self) -> &'static str {
        match self {
            HookEvent::PreToolUse { .. } => "PreToolUse",
            HookEvent::PostToolUse { .. } => "PostToolUse",
            HookEvent::UserPromptSubmit { .. } => "UserPromptSubmit",
            HookEvent::SessionStart { .. } => "SessionStart",
            HookEvent::SessionEnd { .. } => "SessionEnd",
            HookEvent::Stop { .. } => "Stop",
        }
    }

    /// For `PreToolUse` / `PostToolUse`, return the tool name; otherwise `None`.
    pub fn tool_name(&self) -> Option<&str> {
        match self {
            HookEvent::PreToolUse { tool_name, .. } => Some(tool_name.as_str()),
            HookEvent::PostToolUse { tool_name, .. } => Some(tool_name.as_str()),
            _ => None,
        }
    }
}

// ---------------------------------------------------------------------------
// HookOutcome
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HookOutcome {
    pub deny: bool,
    pub message: Option<String>,
    pub mutated_input: Option<serde_json::Value>,
}

// ---------------------------------------------------------------------------
// HookSpec
// ---------------------------------------------------------------------------

fn default_timeout_ms() -> u64 {
    5000
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HookSpec {
    pub name: String,
    pub event: String,
    pub command: String,
    #[serde(default = "default_timeout_ms")]
    pub timeout_ms: u64,
    pub tool_name_pattern: Option<String>,
}

// ---------------------------------------------------------------------------
// Valid event names
// ---------------------------------------------------------------------------

const VALID_EVENTS: &[&str] = &[
    "PreToolUse",
    "PostToolUse",
    "UserPromptSubmit",
    "SessionStart",
    "SessionEnd",
    "Stop",
];

// ---------------------------------------------------------------------------
// TOML file shape
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct HooksFile {
    #[serde(default)]
    hooks: Vec<HookSpec>,
}

// ---------------------------------------------------------------------------
// Pattern matching helper
// ---------------------------------------------------------------------------

fn pattern_matches(pat: &str, name: &str) -> bool {
    pat == "*" || pat == name
}

// ---------------------------------------------------------------------------
// Hook runner
// ---------------------------------------------------------------------------

async fn run_hook(spec: &HookSpec, event_json: &str) -> HookOutcome {
    use tokio::process::Command;

    let timeout_duration = std::time::Duration::from_millis(spec.timeout_ms);

    let mut child = match Command::new("sh")
        .arg("-c")
        .arg(&spec.command)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        // Ensure the child is killed if its handle is dropped (e.g. on timeout).
        // Without this, a misbehaving hook can linger as a zombie after the
        // outer timeout future is cancelled.
        .kill_on_drop(true)
        .spawn()
    {
        Ok(c) => c,
        Err(e) => {
            return HookOutcome {
                deny: false,
                message: Some(format!("hook '{}' failed to spawn: {e}", spec.name)),
                mutated_input: None,
            };
        }
    };

    // Drain stdout/stderr concurrently with the stdin write so a hook that
    // emits output before reading all stdin cannot deadlock on a full pipe
    // buffer. Take the handles up front so we can hold them across the select.
    let mut stdin = child.stdin.take();
    let stdout_handle = child.stdout.take();
    let stderr_handle = child.stderr.take();

    let event_bytes = event_json.as_bytes().to_vec();
    let write_stdin = async move {
        if let Some(mut s) = stdin.take() {
            let _ = s.write_all(&event_bytes).await;
            // Dropping `s` signals EOF on stdin.
        }
    };

    let read_stdout = async move {
        let mut buf = Vec::new();
        if let Some(mut s) = stdout_handle {
            let _ = tokio::io::AsyncReadExt::read_to_end(&mut s, &mut buf).await;
        }
        buf
    };

    let read_stderr = async move {
        let mut buf = Vec::new();
        if let Some(mut s) = stderr_handle {
            let _ = tokio::io::AsyncReadExt::read_to_end(&mut s, &mut buf).await;
        }
        buf
    };

    // Run write+reads concurrently with a timeout that races against process exit.
    // On timeout, kill the child explicitly so it doesn't linger as a zombie.
    let io_and_wait = async {
        let (_, stdout_buf, stderr_buf) =
            tokio::join!(write_stdin, read_stdout, read_stderr);
        let status = child.wait().await?;
        Ok::<(std::process::ExitStatus, Vec<u8>, Vec<u8>), io::Error>((
            status, stdout_buf, stderr_buf,
        ))
    };

    let (status, stdout_buf, stderr_buf) = match tokio::time::timeout(
        timeout_duration,
        io_and_wait,
    )
    .await
    {
        Ok(Ok(triple)) => triple,
        Ok(Err(e)) => {
            return HookOutcome {
                deny: false,
                message: Some(format!("hook '{}' io error: {e}", spec.name)),
                mutated_input: None,
            };
        }
        Err(_) => {
            // Timed out. The child was moved into `io_and_wait`; cancelling
            // that future drops the `Child` handle, and since the command was
            // spawned with `kill_on_drop(true)` the OS process is killed by
            // tokio's reaper rather than leaking as a zombie.
            return HookOutcome {
                deny: false,
                message: Some(format!(
                    "hook '{}' timed out after {}ms",
                    spec.name, spec.timeout_ms
                )),
                mutated_input: None,
            };
        }
    };

    if !stderr_buf.is_empty() {
        let stderr = String::from_utf8_lossy(&stderr_buf);
        tracing::warn!("hook '{}' stderr: {}", spec.name, stderr);
    }

    if !status.success() {
        let code = status.code().unwrap_or(-1);
        return HookOutcome {
            deny: false,
            message: Some(format!(
                "hook '{}' exited with non-zero status: {code}",
                spec.name
            )),
            mutated_input: None,
        };
    }

    let stdout = String::from_utf8_lossy(&stdout_buf);
    match serde_json::from_str::<HookOutcome>(stdout.trim()) {
        Ok(outcome) => outcome,
        Err(e) => HookOutcome {
            deny: false,
            message: Some(format!(
                "hook '{}' output failed to parse as JSON: {e}",
                spec.name
            )),
            mutated_input: None,
        },
    }
}

// ---------------------------------------------------------------------------
// HookRegistry
// ---------------------------------------------------------------------------

#[derive(Default)]
pub struct HookRegistry {
    hooks: Vec<HookSpec>,
}

impl HookRegistry {
    pub fn new() -> Self {
        Self { hooks: Vec::new() }
    }

    pub fn register(&mut self, spec: HookSpec) {
        self.hooks.push(spec);
    }

    pub fn len(&self) -> usize {
        self.hooks.len()
    }

    pub fn is_empty(&self) -> bool {
        self.hooks.is_empty()
    }

    /// Load a `HookRegistry` from a TOML file.
    pub fn load_from_toml(path: &Path) -> Result<Self, HookError> {
        let content = std::fs::read_to_string(path).map_err(HookError::Io)?;
        let file: HooksFile =
            toml::from_str(&content).map_err(|e| HookError::Toml(e.to_string()))?;
        // Validate event names; warn on misconfigurations that would silently
        // mismatch at dispatch time.
        for spec in &file.hooks {
            if !VALID_EVENTS.contains(&spec.event.as_str()) {
                return Err(HookError::InvalidEvent(spec.event.clone()));
            }
            if spec.tool_name_pattern.is_some()
                && !matches!(spec.event.as_str(), "PreToolUse" | "PostToolUse")
            {
                tracing::warn!(
                    hook = %spec.name,
                    event = %spec.event,
                    "tool_name_pattern is only meaningful for PreToolUse/PostToolUse and will be ignored"
                );
            }
        }
        Ok(Self { hooks: file.hooks })
    }

    /// Overlay user-level (`user_dir/hooks.toml`) with project-level
    /// (`project_dir/hooks.toml`). Project-level wins on duplicate `name`.
    /// If either file is absent it is silently skipped.
    pub fn load_hierarchical_with_dirs(user_dir: &Path, project_dir: &Path) -> Self {
        let mut hooks: Vec<HookSpec> = Vec::new();

        // Load user-level hooks
        let user_file = user_dir.join("hooks.toml");
        if user_file.exists() {
            if let Ok(registry) = Self::load_from_toml(&user_file) {
                hooks.extend(registry.hooks);
            }
        }

        // Load project-level hooks, overriding by name
        let project_file = project_dir.join("hooks.toml");
        if project_file.exists() {
            if let Ok(registry) = Self::load_from_toml(&project_file) {
                for spec in registry.hooks {
                    // Remove any existing hook with the same name
                    hooks.retain(|h| h.name != spec.name);
                    hooks.push(spec);
                }
            }
        }

        Self { hooks }
    }

    /// Load from `~/.config/hone/hooks.toml` then `.hone/hooks.toml` relative
    /// to `working_dir`.
    pub fn load_hierarchical(working_dir: &Path) -> Self {
        let user_dir = std::env::var_os("HOME")
            .map(PathBuf::from)
            .map(|h| h.join(".config").join("hone"));

        let project_dir = hone_config::find_project_root(working_dir).map(|r| r.join(".hone"));

        let user_path = user_dir.unwrap_or_else(|| PathBuf::from("/nonexistent"));
        let project_path = project_dir.unwrap_or_else(|| PathBuf::from("/nonexistent"));

        Self::load_hierarchical_with_dirs(&user_path, &project_path)
    }

    /// Dispatch an event to all matching hooks, in registration order.
    /// Returns one `HookOutcome` per hook that matched.
    pub async fn dispatch(&self, event: &HookEvent) -> Vec<HookOutcome> {
        let event_name = event.event_name();
        let event_tool = event.tool_name();
        let event_json = match serde_json::to_string(event) {
            Ok(j) => j,
            Err(e) => {
                tracing::warn!("hooks: failed to serialize event: {e}");
                return Vec::new();
            }
        };

        let mut outcomes = Vec::new();

        for spec in &self.hooks {
            // Event name must match
            if spec.event != event_name {
                continue;
            }

            // Tool name pattern matching for PreToolUse/PostToolUse
            if let Some(tool) = event_tool {
                match &spec.tool_name_pattern {
                    None => {} // None matches all
                    Some(pat) => {
                        if !pattern_matches(pat, tool) {
                            continue;
                        }
                    }
                }
            }

            let outcome = run_hook(spec, &event_json).await;
            outcomes.push(outcome);
        }

        outcomes
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use std::fs;
    use std::time::Instant;

    use serde_json::json;
    use tempfile::TempDir;
    use tokio::time::Duration;

    use super::*;

    // -----------------------------------------------------------------------
    // Helpers
    // -----------------------------------------------------------------------

    fn write_toml(dir: &TempDir, filename: &str, content: &str) -> PathBuf {
        let path = dir.path().join(filename);
        fs::write(&path, content).unwrap();
        path
    }

    fn echo_script(dir: &TempDir, name: &str, body: &str) -> String {
        let path = dir.path().join(name);
        fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
        }
        path.to_string_lossy().into_owned()
    }

    // -----------------------------------------------------------------------
    // Parsing tests
    // -----------------------------------------------------------------------

    #[test]
    fn parse_hooks_toml_with_two_hooks() {
        let dir = TempDir::new().unwrap();
        let toml_content = r#"
[[hooks]]
name = "audit-shell"
event = "PreToolUse"
command = "/usr/local/bin/my-audit.sh"
timeout_ms = 3000
tool_name_pattern = "shell"

[[hooks]]
name = "log-prompt"
event = "UserPromptSubmit"
command = "tee -a /tmp/prompts.log"
"#;
        let path = write_toml(&dir, "hooks.toml", toml_content);
        let registry = HookRegistry::load_from_toml(&path).unwrap();
        assert_eq!(registry.len(), 2);
    }

    #[test]
    fn parse_hooks_toml_missing_event_field_errors() {
        let dir = TempDir::new().unwrap();
        let toml_content = r#"
[[hooks]]
name = "broken"
command = "/bin/true"
"#;
        let path = write_toml(&dir, "hooks.toml", toml_content);
        let result = HookRegistry::load_from_toml(&path);
        assert!(result.is_err(), "expected error for missing event field");
    }

    #[test]
    fn parse_hooks_toml_unknown_event_string_errors() {
        let dir = TempDir::new().unwrap();
        let toml_content = r#"
[[hooks]]
name = "bogus-hook"
event = "Bogus"
command = "/bin/true"
"#;
        let path = write_toml(&dir, "hooks.toml", toml_content);
        let result = HookRegistry::load_from_toml(&path);
        assert!(
            result.is_err(),
            "expected error for unknown event string 'Bogus'"
        );
    }

    #[test]
    fn parse_hooks_toml_default_timeout_5000() {
        let dir = TempDir::new().unwrap();
        let toml_content = r#"
[[hooks]]
name = "no-timeout"
event = "SessionStart"
command = "/bin/true"
"#;
        let path = write_toml(&dir, "hooks.toml", toml_content);
        let registry = HookRegistry::load_from_toml(&path).unwrap();
        assert_eq!(registry.hooks[0].timeout_ms, 5000);
    }

    // -----------------------------------------------------------------------
    // Hierarchical loading tests
    // -----------------------------------------------------------------------

    #[test]
    fn load_hierarchical_overlays_project_over_user() {
        let user_dir = TempDir::new().unwrap();
        let project_dir = TempDir::new().unwrap();

        let user_toml = r#"
[[hooks]]
name = "shared-hook"
event = "SessionStart"
command = "/usr/bin/user-version"
"#;
        let project_toml = r#"
[[hooks]]
name = "shared-hook"
event = "SessionStart"
command = "/usr/bin/project-version"
"#;
        fs::create_dir_all(user_dir.path()).unwrap();
        fs::create_dir_all(project_dir.path()).unwrap();
        fs::write(user_dir.path().join("hooks.toml"), user_toml).unwrap();
        fs::write(project_dir.path().join("hooks.toml"), project_toml).unwrap();

        let registry =
            HookRegistry::load_hierarchical_with_dirs(user_dir.path(), project_dir.path());

        // Exactly one entry — the project version wins.
        assert_eq!(registry.len(), 1);
        assert_eq!(registry.hooks[0].command, "/usr/bin/project-version");
    }

    #[test]
    fn load_hierarchical_missing_dirs_returns_empty() {
        let user_dir = PathBuf::from("/nonexistent/user/config/hone");
        let project_dir = PathBuf::from("/nonexistent/project/.hone");
        let registry = HookRegistry::load_hierarchical_with_dirs(&user_dir, &project_dir);
        assert_eq!(registry.len(), 0);
    }

    // -----------------------------------------------------------------------
    // Dispatch tests
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn dispatch_pre_tool_use_runs_matching_hook() {
        let dir = TempDir::new().unwrap();
        let script = echo_script(
            &dir,
            "ok.sh",
            r#"cat > /dev/null; echo '{"deny":false,"message":null,"mutated_input":null}'"#,
        );

        let mut registry = HookRegistry::new();
        registry.register(HookSpec {
            name: "shell-hook".into(),
            event: "PreToolUse".into(),
            command: script.clone(),
            timeout_ms: 2000,
            tool_name_pattern: Some("shell".into()),
        });

        // Matching tool — should fire.
        let matching_event = HookEvent::PreToolUse {
            tool_name: "shell".into(),
            args: json!({}),
        };
        let outcomes = registry.dispatch(&matching_event).await;
        assert_eq!(outcomes.len(), 1, "hook should fire for matching tool");

        // Non-matching tool — should NOT fire.
        let non_matching_event = HookEvent::PreToolUse {
            tool_name: "read_file".into(),
            args: json!({}),
        };
        let outcomes_no = registry.dispatch(&non_matching_event).await;
        assert_eq!(
            outcomes_no.len(),
            0,
            "hook must not fire for non-matching tool"
        );
    }

    #[tokio::test]
    async fn dispatch_returns_deny_when_hook_emits_deny_true() {
        let dir = TempDir::new().unwrap();
        let script = echo_script(
            &dir,
            "deny.sh",
            r#"cat > /dev/null && echo '{"deny":true,"message":"blocked","mutated_input":null}'"#,
        );

        let mut registry = HookRegistry::new();
        registry.register(HookSpec {
            name: "deny-hook".into(),
            event: "PreToolUse".into(),
            command: script,
            timeout_ms: 2000,
            tool_name_pattern: None,
        });

        let event = HookEvent::PreToolUse {
            tool_name: "any".into(),
            args: json!({}),
        };
        let outcomes = registry.dispatch(&event).await;
        assert_eq!(outcomes.len(), 1);
        assert!(outcomes[0].deny);
        assert_eq!(outcomes[0].message.as_deref(), Some("blocked"));
    }

    #[tokio::test]
    async fn dispatch_passes_event_json_on_stdin() {
        let dir = TempDir::new().unwrap();
        let capture_path = dir.path().join("captured.json");
        let capture_str = capture_path.to_string_lossy().into_owned();
        let script = echo_script(
            &dir,
            "capture.sh",
            &format!(
                r#"cat > "{capture_str}"; echo '{{"deny":false,"message":null,"mutated_input":null}}'"#
            ),
        );

        let mut registry = HookRegistry::new();
        registry.register(HookSpec {
            name: "capture-hook".into(),
            event: "PreToolUse".into(),
            command: script,
            timeout_ms: 2000,
            tool_name_pattern: None,
        });

        let event = HookEvent::PreToolUse {
            tool_name: "shell".into(),
            args: json!({"cmd": "ls"}),
        };
        let _ = registry.dispatch(&event).await;

        let captured = fs::read_to_string(&capture_path).expect("hook did not write stdin");
        let parsed: serde_json::Value =
            serde_json::from_str(&captured).expect("stdin was not valid JSON");
        assert_eq!(parsed["tool_name"], "shell");
    }

    #[tokio::test]
    async fn dispatch_returns_mutated_input_when_provided() {
        let dir = TempDir::new().unwrap();
        let script = echo_script(
            &dir,
            "mutate.sh",
            r#"cat > /dev/null && echo '{"deny":false,"message":null,"mutated_input":{"path":"/safe/path"}}'"#,
        );

        let mut registry = HookRegistry::new();
        registry.register(HookSpec {
            name: "mutate-hook".into(),
            event: "PreToolUse".into(),
            command: script,
            timeout_ms: 2000,
            tool_name_pattern: None,
        });

        let event = HookEvent::PreToolUse {
            tool_name: "any".into(),
            args: json!({}),
        };
        let outcomes = registry.dispatch(&event).await;
        assert_eq!(outcomes.len(), 1);
        assert!(!outcomes[0].deny);
        let mutated = outcomes[0]
            .mutated_input
            .as_ref()
            .expect("expected mutated_input to be Some");
        assert_eq!(mutated["path"], "/safe/path");
    }

    #[tokio::test]
    async fn dispatch_timeout_returns_safe_fallback() {
        let dir = TempDir::new().unwrap();
        let script = echo_script(&dir, "slow.sh", "sleep 5 && echo '{}'");

        let mut registry = HookRegistry::new();
        registry.register(HookSpec {
            name: "slow-hook".into(),
            event: "PreToolUse".into(),
            command: script,
            timeout_ms: 200,
            tool_name_pattern: None,
        });

        let event = HookEvent::PreToolUse {
            tool_name: "shell".into(),
            args: json!({}),
        };
        let start = Instant::now();
        let outcomes = registry.dispatch(&event).await;
        let elapsed = start.elapsed();

        assert!(
            elapsed < Duration::from_secs(2),
            "dispatch must return well before the hook's sleep finishes, took {:?}",
            elapsed
        );
        assert_eq!(outcomes.len(), 1);
        assert!(!outcomes[0].deny);
        let msg = outcomes[0]
            .message
            .as_deref()
            .expect("expected a message on timeout");
        assert!(
            msg.contains("timed out"),
            "message should mention 'timed out', got: {msg}"
        );
    }

    #[tokio::test]
    async fn dispatch_malformed_json_returns_safe_fallback() {
        let dir = TempDir::new().unwrap();
        let script = echo_script(&dir, "bad.sh", "cat > /dev/null; echo 'not json'");

        let mut registry = HookRegistry::new();
        registry.register(HookSpec {
            name: "bad-json-hook".into(),
            event: "PreToolUse".into(),
            command: script,
            timeout_ms: 2000,
            tool_name_pattern: None,
        });

        let event = HookEvent::PreToolUse {
            tool_name: "shell".into(),
            args: json!({}),
        };
        let outcomes = registry.dispatch(&event).await;
        assert_eq!(outcomes.len(), 1);
        assert!(!outcomes[0].deny);
        let msg = outcomes[0]
            .message
            .as_deref()
            .expect("expected a message on malformed JSON");
        // Message should mention parse failure somehow.
        let msg_lower = msg.to_lowercase();
        assert!(
            msg_lower.contains("parse") || msg_lower.contains("json") || msg_lower.contains("invalid"),
            "message should mention parse/json/invalid, got: {msg}"
        );
    }

    #[tokio::test]
    async fn dispatch_no_matching_hooks_returns_empty_vec() {
        let dir = TempDir::new().unwrap();
        let script = echo_script(
            &dir,
            "noop.sh",
            r#"cat > /dev/null; echo '{"deny":false,"message":null,"mutated_input":null}'"#,
        );

        let mut registry = HookRegistry::new();
        // Hook registered for UserPromptSubmit only.
        registry.register(HookSpec {
            name: "prompt-hook".into(),
            event: "UserPromptSubmit".into(),
            command: script,
            timeout_ms: 2000,
            tool_name_pattern: None,
        });

        // Dispatch a different event.
        let event = HookEvent::PreToolUse {
            tool_name: "shell".into(),
            args: json!({}),
        };
        let outcomes = registry.dispatch(&event).await;
        assert_eq!(outcomes.len(), 0);
    }

    #[tokio::test]
    async fn dispatch_runs_multiple_hooks_in_registration_order() {
        let dir = TempDir::new().unwrap();
        let capture_a = dir.path().join("a.txt");
        let capture_b = dir.path().join("b.txt");

        let script_a = echo_script(
            &dir,
            "a.sh",
            &format!(
                r#"cat > /dev/null; echo 'A' >> "{}"; echo '{{"deny":false,"message":"A","mutated_input":null}}'"#,
                capture_a.display()
            ),
        );
        let script_b = echo_script(
            &dir,
            "b.sh",
            &format!(
                r#"cat > /dev/null; echo 'B' >> "{}"; echo '{{"deny":false,"message":"B","mutated_input":null}}'"#,
                capture_b.display()
            ),
        );

        let mut registry = HookRegistry::new();
        registry.register(HookSpec {
            name: "hook-a".into(),
            event: "PreToolUse".into(),
            command: script_a,
            timeout_ms: 2000,
            tool_name_pattern: None,
        });
        registry.register(HookSpec {
            name: "hook-b".into(),
            event: "PreToolUse".into(),
            command: script_b,
            timeout_ms: 2000,
            tool_name_pattern: None,
        });

        let event = HookEvent::PreToolUse {
            tool_name: "shell".into(),
            args: json!({}),
        };
        let outcomes = registry.dispatch(&event).await;

        assert_eq!(outcomes.len(), 2);
        assert_eq!(outcomes[0].message.as_deref(), Some("A"));
        assert_eq!(outcomes[1].message.as_deref(), Some("B"));
    }

    // -----------------------------------------------------------------------
    // Tool-name matching tests
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn tool_name_pattern_glob_star_matches_all() {
        let dir = TempDir::new().unwrap();
        let script = echo_script(
            &dir,
            "glob.sh",
            r#"cat > /dev/null; echo '{"deny":false,"message":null,"mutated_input":null}'"#,
        );

        let mut registry = HookRegistry::new();
        registry.register(HookSpec {
            name: "glob-hook".into(),
            event: "PreToolUse".into(),
            command: script,
            timeout_ms: 2000,
            tool_name_pattern: Some("*".into()),
        });

        for tool in ["shell", "read_file", "list_dir", "write_file"] {
            let event = HookEvent::PreToolUse {
                tool_name: tool.into(),
                args: json!({}),
            };
            let outcomes = registry.dispatch(&event).await;
            assert_eq!(
                outcomes.len(),
                1,
                "glob '*' should match tool '{tool}' but got 0 outcomes"
            );
        }
    }

    #[tokio::test]
    async fn tool_name_pattern_none_matches_all_tools_for_that_event() {
        let dir = TempDir::new().unwrap();
        let script = echo_script(
            &dir,
            "nomatch.sh",
            r#"cat > /dev/null; echo '{"deny":false,"message":null,"mutated_input":null}'"#,
        );

        let mut registry = HookRegistry::new();
        registry.register(HookSpec {
            name: "no-pattern-hook".into(),
            event: "PreToolUse".into(),
            command: script,
            timeout_ms: 2000,
            tool_name_pattern: None,
        });

        for tool in ["shell", "read_file", "anything_at_all"] {
            let event = HookEvent::PreToolUse {
                tool_name: tool.into(),
                args: json!({}),
            };
            let outcomes = registry.dispatch(&event).await;
            assert_eq!(
                outcomes.len(),
                1,
                "None pattern should match any tool; failed for '{tool}'"
            );
        }
    }
}
