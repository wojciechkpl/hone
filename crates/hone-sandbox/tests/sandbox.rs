use hone_sandbox::{NullSandbox, Sandbox, SandboxBuilder, SandboxMode, SandboxPlatform, SandboxPolicyBuilder};
use std::process::Command;

fn minimal_policy() -> hone_sandbox::SandboxPolicy {
    SandboxPolicyBuilder::new(SandboxMode::Manual)
        .build()
        .expect("minimal policy")
}

#[test]
fn test_null_sandbox_always_succeeds() {
    let sandbox = NullSandbox;
    let policy = minimal_policy();

    let result = sandbox.apply(&policy);
    assert!(result.is_ok(), "NullSandbox::apply should always succeed");
}

#[test]
fn test_null_sandbox_restrict_child_succeeds() {
    let sandbox = NullSandbox;
    let policy = minimal_policy();
    let mut cmd = Command::new("echo");

    let result = sandbox.restrict_child(&mut cmd, &policy);
    assert!(
        result.is_ok(),
        "NullSandbox::restrict_child should always succeed"
    );
}

#[test]
fn test_null_sandbox_platform_is_null() {
    let sandbox = NullSandbox;
    assert_eq!(
        sandbox.platform(),
        SandboxPlatform::Null,
        "NullSandbox platform must be Null"
    );
}

#[test]
fn test_sandbox_detect_returns_something() {
    let sandbox = SandboxBuilder::detect();
    // Just assert it doesn't panic and returns a valid platform
    let platform = sandbox.platform();
    // All valid variants are acceptable
    let valid = matches!(
        platform,
        SandboxPlatform::Null
            | SandboxPlatform::Seatbelt
            | SandboxPlatform::Landlock
            | SandboxPlatform::Container
    );
    assert!(valid, "detect() must return a known platform");
}
