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
}
