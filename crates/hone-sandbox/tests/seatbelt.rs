#[cfg(target_os = "macos")]
mod macos_tests {
    use hone_sandbox::{Sandbox, SandboxMode, SandboxPolicyBuilder, SeatbeltProfile, SeatbeltSandbox};
    use std::path::PathBuf;

    #[test]
    fn test_seatbelt_profile_renders_correctly() {
        let policy = SandboxPolicyBuilder::new(SandboxMode::Manual)
            .allow_read(PathBuf::from("/tmp/workspace"))
            .allow_write(PathBuf::from("/tmp/output"))
            .build()
            .expect("policy");

        let profile = SeatbeltProfile::from_policy(&policy);

        // Seatbelt profiles start with version directive
        assert!(
            profile.contains("(version 1)"),
            "profile must contain version directive"
        );
        // Should have a default deny
        assert!(
            profile.contains("(deny default)"),
            "profile must deny by default"
        );
        // Should allow the read path
        assert!(
            profile.contains("/tmp/workspace"),
            "profile must reference allowed read path"
        );
        // Should allow the write path
        assert!(
            profile.contains("/tmp/output"),
            "profile must reference allowed write path"
        );
    }

    #[test]
    fn test_seatbelt_profile_denies_network_by_default() {
        let policy = SandboxPolicyBuilder::new(SandboxMode::Manual)
            .build()
            .expect("policy");

        let profile = SeatbeltProfile::from_policy(&policy);

        // With allow_network = false, network should be denied
        assert!(
            profile.contains("(deny network*)") || profile.contains("(deny network"),
            "profile must deny network when allow_network is false; got:\n{profile}"
        );
    }

    #[test]
    fn test_seatbelt_profile_allows_network_when_requested() {
        let policy = SandboxPolicyBuilder::new(SandboxMode::Manual)
            .allow_network(true)
            .build()
            .expect("policy");

        let profile = SeatbeltProfile::from_policy(&policy);

        assert!(
            profile.contains("(allow network*)") || profile.contains("(allow network"),
            "profile must allow network when allow_network is true; got:\n{profile}"
        );
    }

    #[test]
    fn test_restrict_child_wraps_with_sandbox_exec() {
        use std::ffi::OsStr;
        use std::process::Command;

        let policy = SandboxPolicyBuilder::new(SandboxMode::Manual)
            .allow_read(PathBuf::from("/tmp"))
            .build()
            .expect("policy");

        let sandbox = SeatbeltSandbox;
        let mut cmd = Command::new("sh");
        cmd.arg("-c").arg("echo hello");

        sandbox
            .restrict_child(&mut cmd, &policy)
            .expect("restrict_child should succeed");

        // The command should now start with sandbox-exec.
        assert_eq!(
            cmd.get_program(),
            OsStr::new("sandbox-exec"),
            "program must be sandbox-exec after restrict_child"
        );

        // Args must include "-p" followed by the profile, then the original program.
        let args: Vec<&OsStr> = cmd.get_args().collect();
        assert!(
            args.contains(&OsStr::new("-p")),
            "args must contain -p flag; got: {args:?}"
        );

        // The original program ("sh") must appear in the args.
        assert!(
            args.contains(&OsStr::new("sh")),
            "original program must be forwarded as an arg; got: {args:?}"
        );
    }

    #[test]
    fn test_sandboxed_command_wraps_with_sandbox_exec() {
        use std::ffi::OsStr;

        let policy = SandboxPolicyBuilder::new(SandboxMode::Manual)
            .allow_read(PathBuf::from("/tmp"))
            .build()
            .expect("policy");

        let cmd = SeatbeltSandbox::sandboxed_command("ls", &policy)
            .expect("sandboxed_command should succeed");

        assert_eq!(
            cmd.get_program(),
            OsStr::new("sandbox-exec"),
            "program must be sandbox-exec"
        );

        let args: Vec<&OsStr> = cmd.get_args().collect();
        assert!(
            args.contains(&OsStr::new("-p")),
            "args must contain -p flag; got: {args:?}"
        );
        assert!(
            args.contains(&OsStr::new("ls")),
            "target program must be forwarded as an arg; got: {args:?}"
        );
    }

    // -----------------------------------------------------------------------
    // Integration tests — actually run sandboxed child processes
    // -----------------------------------------------------------------------

    /// Test that a sandboxed child with write access to /tmp can create files.
    #[test]
    fn test_restrict_child_allows_write_to_permitted_dir() {
        use std::process::Command;

        let test_file = "/tmp/hone_seatbelt_test_write.txt";
        let _ = std::fs::remove_file(test_file);

        let policy = SandboxPolicyBuilder::new(SandboxMode::Manual)
            .allow_write(PathBuf::from("/tmp"))
            .build()
            .expect("policy");

        let sandbox = SeatbeltSandbox;
        let mut cmd = Command::new("sh");
        cmd.args(["-c", &format!("echo seatbelt_ok > {test_file}")]);
        cmd.stdout(std::process::Stdio::null());
        cmd.stderr(std::process::Stdio::null());

        sandbox.restrict_child(&mut cmd, &policy).expect("restrict_child");

        let status = cmd.status().expect("spawn child");
        let content = std::fs::read_to_string(test_file).ok();
        let _ = std::fs::remove_file(test_file);

        assert!(
            status.success(),
            "write to allowed /tmp should succeed (status: {})",
            status,
        );
        assert_eq!(
            content.as_deref().map(str::trim),
            Some("seatbelt_ok"),
        );
    }

    /// Test that network access is blocked when policy disables it.
    /// Uses `curl --connect-timeout 1` to a public IP; the sandbox should
    /// cause it to fail immediately.
    #[test]
    fn test_restrict_child_blocks_network() {
        use std::process::Command;

        let policy = SandboxPolicyBuilder::new(SandboxMode::Manual)
            .allow_write(PathBuf::from("/tmp"))
            .build()
            .expect("policy");

        let sandbox = SeatbeltSandbox;
        let mut cmd = Command::new("curl");
        cmd.args(["--connect-timeout", "2", "http://1.1.1.1"]);
        cmd.stdout(std::process::Stdio::null());
        cmd.stderr(std::process::Stdio::null());

        sandbox.restrict_child(&mut cmd, &policy).expect("restrict_child");

        let status = cmd.status().expect("spawn child");

        // Network denied → curl should fail.
        assert!(
            !status.success(),
            "curl should be blocked when network is denied (status: {})",
            status,
        );
    }
}
