/// Errors produced by the sandbox subsystem.
///
/// The sandbox is fail-closed: every variant represents a condition that MUST
/// prevent the sandboxed command from running rather than silently degrading.
#[derive(Debug, thiserror::Error)]
pub enum SandboxError {
    /// The sandbox mechanism cannot enforce the requested policy.
    ///
    /// Returned when the policy asks for a restriction that the current
    /// backend cannot implement (e.g. per-domain network filtering on a
    /// platform that only supports coarse network on/off).
    #[error("sandbox policy not met: {reason}")]
    PolicyNotMet { reason: String },

    /// The sandbox mechanism itself is not available on this host.
    ///
    /// For example, `landlock` requires a kernel ≥ 5.13; `seatbelt` requires
    /// macOS.  Use `SandboxBuilder::detect()` to select the strongest
    /// available backend automatically.
    #[error("sandbox unavailable on {platform}")]
    Unavailable { platform: String },

    /// The caller supplied a logically inconsistent policy.
    ///
    /// For example, autonomous mode requires at least one readable path, so
    /// `SandboxPolicyBuilder::build()` returns this variant when none are
    /// provided.
    #[error("invalid sandbox policy: {reason}")]
    InvalidPolicy { reason: String },

    /// An OS-level call failed while setting up the sandbox.
    #[error("sandbox setup failed: {source}")]
    SetupFailed {
        #[from]
        source: std::io::Error,
    },
}
