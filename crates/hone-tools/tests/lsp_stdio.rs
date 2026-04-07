/// Integration tests for the StdioLspBridge Content-Length framing and
/// error-path behaviour.  These tests do NOT require a real language server.
use hone_tools::lsp::{Language, StdioLspBridge};
use tokio::io::AsyncWriteExt as _;

// ---------------------------------------------------------------------------
// Content-Length framing wire format
// ---------------------------------------------------------------------------

/// Verify that a manually assembled framed message has the correct wire format:
/// `Content-Length: <n>\r\n\r\n<body>`.
#[tokio::test]
async fn test_send_message_format() {
    use serde_json::json;

    let msg = json!({"jsonrpc": "2.0", "id": 1, "method": "test"});
    let body = serde_json::to_string(&msg).unwrap();
    let header = format!("Content-Length: {}\r\n\r\n", body.len());

    let mut wire = header.into_bytes();
    wire.extend_from_slice(body.as_bytes());

    let text = String::from_utf8_lossy(&wire);
    assert!(
        text.starts_with("Content-Length:"),
        "wire message must start with Content-Length header"
    );
    assert!(
        text.contains("\r\n\r\n"),
        "header and body must be separated by \\r\\n\\r\\n"
    );
    assert!(
        text.contains("\"method\":\"test\""),
        "body must appear after the separator"
    );
}

/// Write a framed message into one end of a tokio duplex, read raw bytes from
/// the other end, and verify the Content-Length header value matches the body.
#[tokio::test]
async fn test_framing_length_is_accurate() {
    use serde_json::json;

    let msg = json!({"jsonrpc": "2.0", "id": 99, "result": {"nested": [1, 2, 3]}});
    let body = serde_json::to_string(&msg).unwrap();
    let expected_len = body.len();
    let header = format!("Content-Length: {}\r\n\r\n", expected_len);

    let (mut client_end, mut server_end) = tokio::io::duplex(4096);

    let wire_body = body.clone();
    let wire_header = header.clone();
    tokio::spawn(async move {
        client_end.write_all(wire_header.as_bytes()).await.unwrap();
        client_end.write_all(wire_body.as_bytes()).await.unwrap();
    });

    let mut buf = vec![0u8; header.len() + expected_len];
    let mut read = 0;
    while read < buf.len() {
        use tokio::io::AsyncReadExt as _;
        let n = server_end.read(&mut buf[read..]).await.unwrap();
        assert!(n > 0, "unexpected EOF");
        read += n;
    }

    let text = String::from_utf8_lossy(&buf);
    // Parse the Content-Length value from the header portion.
    let cl_line = text
        .lines()
        .find(|l| l.starts_with("Content-Length:"))
        .expect("Content-Length header missing");
    let reported: usize = cl_line
        .strip_prefix("Content-Length:")
        .unwrap()
        .trim()
        .parse()
        .expect("Content-Length is not a number");

    assert_eq!(
        reported, expected_len,
        "Content-Length must equal actual body length"
    );
}

// ---------------------------------------------------------------------------
// Error-path: spawning a non-existent binary
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_lsp_spawn_nonexistent_binary_errors() {
    let result = StdioLspBridge::spawn("__nonexistent_lsp_xyz_999__", &[]).await;
    assert!(
        result.is_err(),
        "spawning a missing binary must return Err"
    );
    let msg = result.err().unwrap().to_string();
    assert!(
        msg.contains("__nonexistent_lsp_xyz_999__"),
        "error message should mention the binary name, got: {msg}"
    );
}

// ---------------------------------------------------------------------------
// Error-path: unsupported languages
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_lsp_for_unknown_language_errors() {
    let result = StdioLspBridge::for_language(Language::Unknown).await;
    assert!(
        result.is_err(),
        "Unknown language must not resolve to an LSP server"
    );
}

#[tokio::test]
async fn test_lsp_for_c_language_errors() {
    let result = StdioLspBridge::for_language(Language::C).await;
    assert!(
        result.is_err(),
        "C has no configured LSP server, expected Err"
    );
}

#[tokio::test]
async fn test_lsp_for_cpp_language_errors() {
    let result = StdioLspBridge::for_language(Language::Cpp).await;
    assert!(result.is_err());
}

#[tokio::test]
async fn test_lsp_for_java_language_errors() {
    let result = StdioLspBridge::for_language(Language::Java).await;
    assert!(result.is_err());
}
