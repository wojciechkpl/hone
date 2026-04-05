use hone_core::{AuditEntry, AuditFilter, AuditKind, AuditLog, InMemoryAuditLog};
use serde_json::json;

fn make_entry(id: &str, session_id: &str, timestamp: u64, kind: AuditKind) -> AuditEntry {
    AuditEntry {
        id: id.to_string(),
        session_id: session_id.to_string(),
        timestamp,
        kind,
        actor: "test-actor".to_string(),
        detail: json!({}),
    }
}

#[tokio::test]
async fn test_audit_log_records_entry() {
    let log = InMemoryAuditLog::new();
    let entry = make_entry("1", "session-a", 1000, AuditKind::ToolCall);

    log.record(entry.clone()).await.unwrap();

    let results = log.query(&AuditFilter::default()).await.unwrap();
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].kind, AuditKind::ToolCall);
}

#[tokio::test]
async fn test_audit_log_records_multiple_kinds() {
    let log = InMemoryAuditLog::new();

    log.record(make_entry("1", "session-a", 1000, AuditKind::ToolCall))
        .await
        .unwrap();
    log.record(make_entry("2", "session-a", 1001, AuditKind::ModelRequest))
        .await
        .unwrap();
    log.record(make_entry("3", "session-a", 1002, AuditKind::SessionStart))
        .await
        .unwrap();

    let results = log.query(&AuditFilter::default()).await.unwrap();
    assert_eq!(results.len(), 3);
}

#[tokio::test]
async fn test_audit_entries_have_timestamps() {
    let log = InMemoryAuditLog::new();
    let entry = make_entry("1", "session-a", 12345, AuditKind::ToolCall);

    log.record(entry).await.unwrap();

    let results = log.query(&AuditFilter::default()).await.unwrap();
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].timestamp, 12345);
}

#[tokio::test]
async fn test_audit_entries_have_session_id() {
    let log = InMemoryAuditLog::new();

    log.record(make_entry("1", "session-a", 1000, AuditKind::ToolCall))
        .await
        .unwrap();
    log.record(make_entry("2", "session-b", 1001, AuditKind::ToolCall))
        .await
        .unwrap();
    log.record(make_entry("3", "session-a", 1002, AuditKind::ToolCall))
        .await
        .unwrap();

    let filter = AuditFilter {
        session_id: Some("session-a".to_string()),
        ..Default::default()
    };
    let results = log.query(&filter).await.unwrap();
    assert_eq!(results.len(), 2);
    assert!(results.iter().all(|e| e.session_id == "session-a"));
}

#[tokio::test]
async fn test_audit_query_by_kind() {
    let log = InMemoryAuditLog::new();

    log.record(make_entry("1", "session-a", 1000, AuditKind::ToolCall))
        .await
        .unwrap();
    log.record(make_entry("2", "session-a", 1001, AuditKind::ToolCall))
        .await
        .unwrap();
    log.record(make_entry("3", "session-a", 1002, AuditKind::ModelRequest))
        .await
        .unwrap();

    let filter = AuditFilter {
        kind: Some(AuditKind::ToolCall),
        ..Default::default()
    };
    let results = log.query(&filter).await.unwrap();
    assert_eq!(results.len(), 2);
    assert!(results.iter().all(|e| e.kind == AuditKind::ToolCall));
}

#[tokio::test]
async fn test_audit_export_produces_json() {
    let log = InMemoryAuditLog::new();

    let entry_a = make_entry("1", "session-a", 1000, AuditKind::ToolCall);
    let entry_b = make_entry("2", "session-a", 1001, AuditKind::ModelRequest);
    log.record(entry_a.clone()).await.unwrap();
    log.record(entry_b.clone()).await.unwrap();

    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("audit.json");
    log.export(&path).await.unwrap();

    let bytes = tokio::fs::read(&path).await.unwrap();
    let deserialized: Vec<hone_core::AuditEntry> = serde_json::from_slice(&bytes).unwrap();

    assert_eq!(deserialized.len(), 2);
    assert_eq!(deserialized[0].id, entry_a.id);
    assert_eq!(deserialized[1].id, entry_b.id);
}

#[tokio::test]
async fn test_audit_log_is_append_only() {
    let log = InMemoryAuditLog::new();

    log.record(make_entry("1", "session-a", 1000, AuditKind::ToolCall))
        .await
        .unwrap();
    log.record(make_entry("2", "session-a", 1001, AuditKind::ToolCall))
        .await
        .unwrap();
    log.record(make_entry("3", "session-a", 1002, AuditKind::ToolCall))
        .await
        .unwrap();

    assert_eq!(log.count().await, 3);

    log.record(make_entry("4", "session-a", 1003, AuditKind::ModelRequest))
        .await
        .unwrap();
    log.record(make_entry("5", "session-a", 1004, AuditKind::ModelRequest))
        .await
        .unwrap();

    assert_eq!(log.count().await, 5);
}
