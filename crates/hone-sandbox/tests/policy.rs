#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

use hone_sandbox::{SandboxError, SandboxMode, SandboxPolicyBuilder};
use std::path::PathBuf;

#[test]
fn test_policy_builder_basic() {
    let read_path = PathBuf::from("/tmp/read");
    let write_path = PathBuf::from("/tmp/write");

    let policy = SandboxPolicyBuilder::new(SandboxMode::Manual)
        .allow_read(read_path.clone())
        .allow_write(write_path.clone())
        .build()
        .expect("should build valid policy");

    assert!(policy.allow_read.contains(&read_path));
    assert!(policy.allow_write.contains(&write_path));
}

#[test]
fn test_policy_builder_rejects_empty_read_paths() {
    let result = SandboxPolicyBuilder::new(SandboxMode::Autonomous).build();

    assert!(
        matches!(result, Err(SandboxError::InvalidPolicy { .. })),
        "autonomous mode with no read paths should fail"
    );
}

#[test]
fn test_policy_builder_allows_empty_for_manual() {
    let result = SandboxPolicyBuilder::new(SandboxMode::Manual).build();

    assert!(result.is_ok(), "manual mode with no paths should succeed");
}

#[test]
fn test_policy_builder_network_defaults_off() {
    let policy = SandboxPolicyBuilder::new(SandboxMode::Manual)
        .build()
        .expect("should build");

    assert!(!policy.allow_network, "network should be off by default");
}

#[test]
fn test_policy_builder_add_multiple_paths() {
    let paths = [
        PathBuf::from("/tmp/a"),
        PathBuf::from("/tmp/b"),
        PathBuf::from("/tmp/c"),
    ];

    let mut builder = SandboxPolicyBuilder::new(SandboxMode::Manual);
    for p in &paths {
        builder = builder.allow_read(p.clone());
    }
    let policy = builder.build().expect("should build");

    for p in &paths {
        assert!(
            policy.allow_read.contains(p),
            "missing path: {}",
            p.display()
        );
    }
}

#[test]
fn test_policy_builder_env_allowlist() {
    let policy = SandboxPolicyBuilder::new(SandboxMode::Manual)
        .allow_env("PATH")
        .allow_env("HOME")
        .build()
        .expect("should build");

    assert!(policy.allow_env.contains(&"PATH".to_string()));
    assert!(policy.allow_env.contains(&"HOME".to_string()));
}

#[test]
fn test_policy_builder_supervised_allows_empty_reads() {
    // Supervised mode (like Manual) should not require read paths
    let result = SandboxPolicyBuilder::new(SandboxMode::Supervised).build();
    assert!(result.is_ok());
}

#[test]
fn test_policy_builder_network_and_domains() {
    let policy = SandboxPolicyBuilder::new(SandboxMode::Manual)
        .allow_network(true)
        .allow_domain("api.example.com")
        .build()
        .expect("should build");

    assert!(policy.allow_network);
    assert!(
        policy
            .allowed_domains
            .contains(&"api.example.com".to_string())
    );
}

#[test]
fn test_policy_builder_exec_paths() {
    let exec_path = PathBuf::from("/usr/bin/git");

    let policy = SandboxPolicyBuilder::new(SandboxMode::Manual)
        .allow_exec(exec_path.clone())
        .build()
        .expect("should build");

    assert!(policy.allow_exec.contains(&exec_path));
}
