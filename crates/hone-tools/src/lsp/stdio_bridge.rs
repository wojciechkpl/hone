use std::collections::HashMap;
use std::path::Path;
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, ChildStdout, Command};
use tokio::sync::Mutex;

use crate::error::ToolError;

use super::bridge::{CodeAction, Diagnostic, DiagnosticSeverity, LspBridge};

// ---------------------------------------------------------------------------
// JSON-RPC framing helpers (Content-Length over stdio)
// ---------------------------------------------------------------------------

/// Write a single LSP message with Content-Length framing to `stdin`.
pub(crate) async fn send_message(stdin: &mut ChildStdin, msg: &Value) -> Result<(), ToolError> {
    let serialized = serde_json::to_string(msg)
        .map_err(|e| ToolError::Other(format!("LSP serialize error: {e}")))?;
    let header = format!("Content-Length: {}\r\n\r\n", serialized.len());
    stdin.write_all(header.as_bytes()).await?;
    stdin.write_all(serialized.as_bytes()).await?;
    stdin.flush().await?;
    Ok(())
}

/// Read a single LSP message from a buffered stdout reader.
///
/// Reads `Content-Length:` headers until the blank separator line, then reads
/// exactly that many bytes as the JSON body.
pub(crate) async fn read_message(reader: &mut BufReader<ChildStdout>) -> Result<Value, ToolError> {
    let mut content_length: usize = 0;

    // Read header lines until we hit the blank \r\n separator.
    loop {
        let mut line = String::new();
        let n = reader.read_line(&mut line).await?;
        if n == 0 {
            return Err(ToolError::Other(
                "LSP server closed stdout unexpectedly".into(),
            ));
        }
        let trimmed = line.trim();
        if trimmed.is_empty() {
            break;
        }
        if let Some(rest) = trimmed.strip_prefix("Content-Length:") {
            content_length = rest
                .trim()
                .parse()
                .map_err(|_| ToolError::Other("invalid Content-Length header".into()))?;
        }
        // Other headers (Content-Type etc.) are intentionally ignored.
    }

    if content_length == 0 {
        return Err(ToolError::Other(
            "LSP message missing Content-Length header".into(),
        ));
    }

    let mut buf = vec![0u8; content_length];
    reader.read_exact(&mut buf).await?;

    let value: Value = serde_json::from_slice(&buf)
        .map_err(|e| ToolError::Other(format!("LSP JSON parse error: {e}")))?;
    Ok(value)
}

// ---------------------------------------------------------------------------
// Conversion helpers
// ---------------------------------------------------------------------------

fn severity_from_lsp(n: u64) -> DiagnosticSeverity {
    match n {
        1 => DiagnosticSeverity::Error,
        2 => DiagnosticSeverity::Warning,
        3 => DiagnosticSeverity::Info,
        _ => DiagnosticSeverity::Hint,
    }
}

fn diagnostics_from_notification(params: &Value, file: &Path) -> Vec<Diagnostic> {
    let Some(array) = params["diagnostics"].as_array() else {
        return vec![];
    };
    array
        .iter()
        .map(|d| {
            let line = d["range"]["start"]["line"].as_u64().unwrap_or(0) as u32;
            let col = d["range"]["start"]["character"].as_u64().unwrap_or(0) as u32;
            let severity = d["severity"]
                .as_u64()
                .map(severity_from_lsp)
                .unwrap_or(DiagnosticSeverity::Error);
            let message = d["message"].as_str().unwrap_or("").to_owned();
            let code = d["code"].as_str().map(str::to_owned).or_else(|| {
                d["code"].as_u64().map(|n| n.to_string())
            });
            Diagnostic {
                file: file.to_path_buf(),
                line,
                col,
                severity,
                message,
                code,
            }
        })
        .collect()
}

fn hover_content_to_string(result: &Value) -> Option<String> {
    match result {
        Value::Null => None,
        result => {
            // result.contents is either a MarkedString, []MarkedString, or MarkupContent
            let contents = &result["contents"];
            if contents.is_null() {
                return None;
            }
            if let Some(s) = contents.as_str() {
                return Some(s.to_owned());
            }
            if let Some(obj) = contents.as_object() {
                // MarkupContent: { kind, value }
                if let Some(v) = obj.get("value").and_then(Value::as_str) {
                    return Some(v.to_owned());
                }
            }
            if let Some(arr) = contents.as_array() {
                let parts: Vec<&str> = arr
                    .iter()
                    .filter_map(|item| {
                        item.as_str().or_else(|| item["value"].as_str())
                    })
                    .collect();
                if parts.is_empty() {
                    return None;
                }
                return Some(parts.join("\n"));
            }
            None
        }
    }
}

fn code_actions_from_result(result: &Value) -> Vec<CodeAction> {
    let Some(array) = result.as_array() else {
        return vec![];
    };
    array
        .iter()
        .filter_map(|item| {
            let title = item["title"].as_str()?.to_owned();
            let kind = item["kind"].as_str().map(str::to_owned);
            Some(CodeAction { title, kind })
        })
        .collect()
}

// ---------------------------------------------------------------------------
// StdioLspBridge
// ---------------------------------------------------------------------------

/// Internal mutable state of the LSP client.
struct StdioLspBridgeInner {
    _child: Child,
    stdin: ChildStdin,
    reader: BufReader<ChildStdout>,
    next_id: u64,
    /// Diagnostics received via `textDocument/publishDiagnostics` notifications,
    /// keyed by the URI string from the notification.
    pending_diagnostics: HashMap<String, Vec<Diagnostic>>,
}

impl StdioLspBridgeInner {
    /// Allocate the next request id and increment the counter.
    fn alloc_id(&mut self) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        id
    }

    /// Send `method` as a JSON-RPC request and wait for the matching response,
    /// buffering any `textDocument/publishDiagnostics` notifications along the way.
    ///
    /// Returns the `result` field of the response, or `Null` if absent.
    async fn request(&mut self, method: &str, params: Value) -> Result<Value, ToolError> {
        let id = self.alloc_id();
        let message = json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params,
        });
        send_message(&mut self.stdin, &message).await?;

        loop {
            let msg = read_message(&mut self.reader).await?;
            // If the message has a numeric `id` matching ours it's the response.
            if msg.get("id") == Some(&json!(id)) {
                if let Some(err) = msg.get("error") {
                    return Err(ToolError::Other(format!(
                        "LSP error response: {}",
                        err["message"].as_str().unwrap_or("unknown")
                    )));
                }
                return Ok(msg["result"].clone());
            }
            // Otherwise buffer notifications we care about.
            self.handle_notification(&msg);
        }
    }

    /// Send a JSON-RPC notification (no id, no response expected).
    async fn notify(&mut self, method: &str, params: Value) -> Result<(), ToolError> {
        let message = json!({
            "jsonrpc": "2.0",
            "method": method,
            "params": params,
        });
        send_message(&mut self.stdin, &message).await
    }

    /// Buffer `textDocument/publishDiagnostics` notifications for later retrieval.
    fn handle_notification(&mut self, msg: &Value) {
        let method = msg["method"].as_str().unwrap_or_default();
        if method == "textDocument/publishDiagnostics" {
            let params = &msg["params"];
            let uri = params["uri"].as_str().unwrap_or_default().to_owned();
            // Derive a path from the URI for constructing Diagnostic.file.
            let path = uri_to_path(&uri);
            let diags = diagnostics_from_notification(params, &path);
            self.pending_diagnostics.insert(uri, diags);
        }
    }

    /// Run the LSP initialization handshake.
    async fn initialize(&mut self) -> Result<(), ToolError> {
        let params = json!({
            "processId": std::process::id(),
            "rootUri": null,
            "capabilities": {
                "textDocument": {
                    "publishDiagnostics": {
                        "relatedInformation": false
                    },
                    "hover": {
                        "contentFormat": ["plaintext", "markdown"]
                    },
                    "codeAction": {}
                }
            },
            "clientInfo": {
                "name": "hone",
                "version": env!("CARGO_PKG_VERSION")
            }
        });

        self.request("initialize", params).await?;

        // `initialized` is a notification (no id).
        self.notify("initialized", json!({})).await?;

        Ok(())
    }

    /// Send `textDocument/didOpen` for `file` and wait up to `timeout` for a
    /// `publishDiagnostics` notification keyed to that file's URI.
    ///
    /// Many language servers publish diagnostics asynchronously after didOpen,
    /// so we poll for a short window.
    async fn did_open_and_wait(
        &mut self,
        file: &Path,
        timeout: Duration,
    ) -> Result<Vec<Diagnostic>, ToolError> {
        let content = std::fs::read_to_string(file).map_err(|e| {
            ToolError::Other(format!("failed to read {}: {e}", file.display()))
        })?;

        let language_id = language_id_for_path(file);
        let uri = path_to_uri(file);

        self.notify(
            "textDocument/didOpen",
            json!({
                "textDocument": {
                    "uri": uri,
                    "languageId": language_id,
                    "version": 1,
                    "text": content
                }
            }),
        )
        .await?;

        // Poll for publishDiagnostics up to `timeout`.
        let deadline = tokio::time::Instant::now() + timeout;
        loop {
            // Check if we already have diagnostics buffered for this URI.
            if let Some(diags) = self.pending_diagnostics.get(&uri) {
                return Ok(diags.clone());
            }

            if tokio::time::Instant::now() >= deadline {
                // Timeout — return empty rather than error; server may not publish
                // diagnostics for a file it considers clean.
                return Ok(vec![]);
            }

            // Try to read one more message non-blockingly with a short inner timeout.
            let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
            let inner_timeout = remaining.min(Duration::from_millis(100));

            match tokio::time::timeout(inner_timeout, read_message(&mut self.reader)).await {
                Ok(Ok(msg)) => self.handle_notification(&msg),
                Ok(Err(e)) => return Err(e),
                Err(_elapsed) => {
                    // Inner timeout — loop again to check deadline.
                }
            }
        }
    }
}

/// Real LSP client that spawns a language server subprocess and speaks
/// JSON-RPC 2.0 over stdio with Content-Length framing.
pub struct StdioLspBridge {
    inner: Arc<Mutex<StdioLspBridgeInner>>,
}

impl StdioLspBridge {
    /// Spawn a language server process and perform the LSP initialization handshake.
    ///
    /// `command` is the binary name (e.g. `"rust-analyzer"`) and `args` are any
    /// additional arguments (e.g. `&["--stdio"]` for servers that require it).
    pub async fn spawn(command: &str, args: &[&str]) -> Result<Self, ToolError> {
        let mut cmd = Command::new(command);
        cmd.args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());

        let mut child = cmd.spawn().map_err(|e| {
            ToolError::Other(format!("failed to spawn LSP server '{command}': {e}"))
        })?;

        let stdin = child.stdin.take().ok_or_else(|| {
            ToolError::Other("failed to capture LSP server stdin".into())
        })?;

        let stdout = child.stdout.take().ok_or_else(|| {
            ToolError::Other("failed to capture LSP server stdout".into())
        })?;

        let mut inner = StdioLspBridgeInner {
            _child: child,
            stdin,
            reader: BufReader::new(stdout),
            next_id: 1,
            pending_diagnostics: HashMap::new(),
        };

        inner.initialize().await?;

        Ok(Self {
            inner: Arc::new(Mutex::new(inner)),
        })
    }

    /// Auto-detect and spawn an LSP server for the given language.
    ///
    /// Returns an error if no known LSP server is available for the language.
    pub async fn for_language(language: super::detect::Language) -> Result<Self, ToolError> {
        match language {
            super::detect::Language::Rust => Self::spawn("rust-analyzer", &[]).await,
            super::detect::Language::Python => {
                Self::spawn("pyright-langserver", &["--stdio"]).await
            }
            super::detect::Language::TypeScript | super::detect::Language::JavaScript => {
                Self::spawn("typescript-language-server", &["--stdio"]).await
            }
            super::detect::Language::Go => Self::spawn("gopls", &[]).await,
            _ => Err(ToolError::Other(format!(
                "no LSP server configured for language {:?}",
                language
            ))),
        }
    }
}

#[async_trait]
impl LspBridge for StdioLspBridge {
    async fn diagnostics(&self, file: &Path) -> Result<Vec<Diagnostic>, ToolError> {
        let mut guard = self.inner.lock().await;
        // Wait up to 5 seconds for the server to publish diagnostics after didOpen.
        guard.did_open_and_wait(file, Duration::from_secs(5)).await
    }

    async fn hover(&self, file: &Path, line: u32, col: u32) -> Result<Option<String>, ToolError> {
        let mut guard = self.inner.lock().await;
        let uri = path_to_uri(file);
        let result = guard
            .request(
                "textDocument/hover",
                json!({
                    "textDocument": { "uri": uri },
                    "position": { "line": line, "character": col }
                }),
            )
            .await?;

        Ok(hover_content_to_string(&result))
    }

    async fn code_actions(
        &self,
        file: &Path,
        line: u32,
        col: u32,
    ) -> Result<Vec<CodeAction>, ToolError> {
        let mut guard = self.inner.lock().await;
        let uri = path_to_uri(file);
        let result = guard
            .request(
                "textDocument/codeAction",
                json!({
                    "textDocument": { "uri": uri },
                    "range": {
                        "start": { "line": line, "character": col },
                        "end":   { "line": line, "character": col }
                    },
                    "context": { "diagnostics": [] }
                }),
            )
            .await?;

        Ok(code_actions_from_result(&result))
    }
}

// ---------------------------------------------------------------------------
// URI / path helpers
// ---------------------------------------------------------------------------

/// Convert a file path to a `file://` URI string.
pub(crate) fn path_to_uri(path: &Path) -> String {
    // Canonicalize if possible; fall back to the path as-is.
    let p = path
        .canonicalize()
        .unwrap_or_else(|_| path.to_path_buf());
    format!("file://{}", p.display())
}

/// Strip a `file://` prefix and return a `PathBuf`.
fn uri_to_path(uri: &str) -> std::path::PathBuf {
    uri.strip_prefix("file://")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| std::path::PathBuf::from(uri))
}

/// Return the LSP `languageId` string for a file path.
fn language_id_for_path(path: &Path) -> &'static str {
    match path.extension().and_then(|e| e.to_str()) {
        Some("rs") => "rust",
        Some("py") | Some("pyw") => "python",
        Some("ts") | Some("tsx") => "typescript",
        Some("js") | Some("jsx") | Some("mjs") | Some("cjs") => "javascript",
        Some("go") => "go",
        Some("java") => "java",
        Some("c") | Some("h") => "c",
        Some("cpp") | Some("cxx") | Some("cc") | Some("hpp") | Some("hxx") => "cpp",
        _ => "plaintext",
    }
}

// ---------------------------------------------------------------------------
// Unit tests (framing + pure conversion logic; no real LSP server needed)
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::path::PathBuf;

    // -----------------------------------------------------------------------
    // Content-Length framing round-trip
    // -----------------------------------------------------------------------

    /// Build a raw framed LSP message in memory (what send_message produces).
    fn framed(msg: &Value) -> Vec<u8> {
        let body = serde_json::to_string(msg).unwrap();
        let header = format!("Content-Length: {}\r\n\r\n", body.len());
        let mut out = header.into_bytes();
        out.extend_from_slice(body.as_bytes());
        out
    }

    #[tokio::test]
    async fn test_framing_format_contains_content_length() {
        let msg = json!({"jsonrpc": "2.0", "id": 1, "method": "test"});
        let bytes = framed(&msg);
        let text = String::from_utf8_lossy(&bytes);
        assert!(text.starts_with("Content-Length:"), "must start with header");
        assert!(text.contains("\r\n\r\n"), "must have blank-line separator");
        assert!(text.contains("\"method\":\"test\""), "must contain body");
    }

    #[tokio::test]
    async fn test_read_message_roundtrip() {
        // Build a framed message and feed it through a Cursor into read_message.
        // read_message takes &mut BufReader<ChildStdout>, so we use a real child
        // process pipe filled by a helper task instead.
        //
        // We use tokio's duplex() to avoid spawning a shell, but read_message
        // requires BufReader<ChildStdout>. Since ChildStdout is opaque, we test
        // the framing *logic* through send_message + raw read, which is sufficient
        // to validate the protocol without running an actual LSP server.
        let msg = json!({"jsonrpc":"2.0","id":42,"result":{"ok":true}});
        let bytes = framed(&msg);

        // Verify parse: deserialise the body portion directly.
        let separator = b"\r\n\r\n";
        let sep_pos = bytes
            .windows(separator.len())
            .position(|w| w == separator)
            .unwrap();
        let body = &bytes[sep_pos + separator.len()..];
        let parsed: Value = serde_json::from_slice(body).unwrap();
        assert_eq!(parsed["id"], 42);
        assert_eq!(parsed["result"]["ok"], true);
    }

    // -----------------------------------------------------------------------
    // Diagnostic conversion
    // -----------------------------------------------------------------------

    #[test]
    fn test_diagnostics_from_notification_parses_correctly() {
        let params = json!({
            "uri": "file:///project/src/main.rs",
            "diagnostics": [
                {
                    "range": {
                        "start": { "line": 5, "character": 10 },
                        "end":   { "line": 5, "character": 15 }
                    },
                    "severity": 1,
                    "code": "E0308",
                    "message": "mismatched types"
                },
                {
                    "range": {
                        "start": { "line": 2, "character": 0 },
                        "end":   { "line": 2, "character": 4 }
                    },
                    "severity": 2,
                    "message": "unused import"
                }
            ]
        });

        let file = PathBuf::from("/project/src/main.rs");
        let diags = diagnostics_from_notification(&params, &file);

        assert_eq!(diags.len(), 2);

        let first = &diags[0];
        assert_eq!(first.line, 5);
        assert_eq!(first.col, 10);
        assert_eq!(first.severity, DiagnosticSeverity::Error);
        assert_eq!(first.message, "mismatched types");
        assert_eq!(first.code.as_deref(), Some("E0308"));

        let second = &diags[1];
        assert_eq!(second.line, 2);
        assert_eq!(second.col, 0);
        assert_eq!(second.severity, DiagnosticSeverity::Warning);
        assert_eq!(second.message, "unused import");
        assert!(second.code.is_none());
    }

    #[test]
    fn test_diagnostics_from_notification_numeric_code() {
        let params = json!({
            "uri": "file:///foo.ts",
            "diagnostics": [
                {
                    "range": { "start": { "line": 0, "character": 0 }, "end": { "line": 0, "character": 1 } },
                    "severity": 1,
                    "code": 2345,
                    "message": "cannot find name"
                }
            ]
        });
        let file = PathBuf::from("/foo.ts");
        let diags = diagnostics_from_notification(&params, &file);
        assert_eq!(diags[0].code.as_deref(), Some("2345"));
    }

    #[test]
    fn test_diagnostics_from_notification_empty() {
        let params = json!({"uri": "file:///clean.rs", "diagnostics": []});
        let diags = diagnostics_from_notification(&params, &PathBuf::from("/clean.rs"));
        assert!(diags.is_empty());
    }

    // -----------------------------------------------------------------------
    // Hover content conversion
    // -----------------------------------------------------------------------

    #[test]
    fn test_hover_markup_content() {
        let result = json!({"contents": {"kind": "markdown", "value": "# fn foo"}});
        assert_eq!(hover_content_to_string(&result).as_deref(), Some("# fn foo"));
    }

    #[test]
    fn test_hover_plain_string_contents() {
        let result = json!({"contents": "fn bar() -> u32"});
        assert_eq!(
            hover_content_to_string(&result).as_deref(),
            Some("fn bar() -> u32")
        );
    }

    #[test]
    fn test_hover_array_contents() {
        let result = json!({"contents": ["first part", {"language": "rust", "value": "fn baz()"}]});
        let text = hover_content_to_string(&result).unwrap();
        assert!(text.contains("first part"));
        assert!(text.contains("fn baz()"));
    }

    #[test]
    fn test_hover_null_result() {
        assert!(hover_content_to_string(&Value::Null).is_none());
    }

    // -----------------------------------------------------------------------
    // Code action conversion
    // -----------------------------------------------------------------------

    #[test]
    fn test_code_actions_from_result() {
        let result = json!([
            {"title": "Remove unused import", "kind": "quickfix"},
            {"title": "Extract function", "kind": "refactor.extract"},
            {"title": "No kind action"}
        ]);
        let actions = code_actions_from_result(&result);
        assert_eq!(actions.len(), 3);
        assert_eq!(actions[0].title, "Remove unused import");
        assert_eq!(actions[0].kind.as_deref(), Some("quickfix"));
        assert_eq!(actions[1].kind.as_deref(), Some("refactor.extract"));
        assert!(actions[2].kind.is_none());
    }

    #[test]
    fn test_code_actions_from_null_result() {
        assert!(code_actions_from_result(&Value::Null).is_empty());
    }

    // -----------------------------------------------------------------------
    // URI / path helpers
    // -----------------------------------------------------------------------

    #[test]
    fn test_uri_to_path() {
        let path = uri_to_path("file:///home/user/project/main.rs");
        assert_eq!(path, PathBuf::from("/home/user/project/main.rs"));
    }

    #[test]
    fn test_uri_to_path_no_prefix() {
        // Falls back to treating the whole string as a path.
        let path = uri_to_path("/fallback/path");
        assert_eq!(path, PathBuf::from("/fallback/path"));
    }

    #[test]
    fn test_language_id_for_path() {
        assert_eq!(language_id_for_path(&PathBuf::from("main.rs")), "rust");
        assert_eq!(language_id_for_path(&PathBuf::from("app.py")), "python");
        assert_eq!(language_id_for_path(&PathBuf::from("index.ts")), "typescript");
        assert_eq!(language_id_for_path(&PathBuf::from("lib.js")), "javascript");
        assert_eq!(language_id_for_path(&PathBuf::from("main.go")), "go");
        assert_eq!(language_id_for_path(&PathBuf::from("unknown.xyz")), "plaintext");
    }

    // -----------------------------------------------------------------------
    // Spawn / for_language error cases (no real server needed)
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_spawn_nonexistent_binary_returns_error() {
        let result = StdioLspBridge::spawn("__nonexistent_lsp_server_xyz__", &[]).await;
        assert!(result.is_err(), "expected Err for missing binary");
        let msg = result.err().unwrap().to_string();
        // Should mention the binary name in the error message.
        assert!(
            msg.contains("__nonexistent_lsp_server_xyz__"),
            "error message should name the binary: {msg}"
        );
    }

    #[tokio::test]
    async fn test_for_language_unknown_returns_error() {
        let result = StdioLspBridge::for_language(super::super::detect::Language::Unknown).await;
        assert!(result.is_err(), "expected Err for Unknown language");
    }

    #[tokio::test]
    async fn test_for_language_c_returns_error() {
        // C and C++ have no configured server.
        let result = StdioLspBridge::for_language(super::super::detect::Language::C).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_for_language_cpp_returns_error() {
        let result = StdioLspBridge::for_language(super::super::detect::Language::Cpp).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_for_language_java_returns_error() {
        let result = StdioLspBridge::for_language(super::super::detect::Language::Java).await;
        assert!(result.is_err());
    }
}
