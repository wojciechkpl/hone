#[cfg(target_os = "macos")]
mod macos_tests {
    use hone_sandbox::{SandboxMode, SandboxPolicyBuilder, SeatbeltProfile};
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
}
