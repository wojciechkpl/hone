//! Platform detection and sandbox backend selection.

use crate::Sandbox;

/// Factory that selects the strongest sandbox backend available on the current
/// platform.
///
/// # Selection order
///
/// | Platform | First choice  | Fallback    |
/// |----------|---------------|-------------|
/// | macOS    | `Seatbelt`    | `Null`      |
/// | Linux    | `Landlock`    | `Null`      |
/// | Other    | `Null`        | —           |
///
/// # Example
///
/// ```rust
/// use hone_sandbox::{Sandbox, SandboxBuilder};
///
/// let sandbox: Box<dyn Sandbox> = SandboxBuilder::detect();
/// println!("active backend: {:?}", sandbox.platform());
/// ```
pub struct SandboxBuilder;

impl SandboxBuilder {
    /// Detect the platform and return the strongest available sandbox.
    pub fn detect() -> Box<dyn Sandbox> {
        cfg_if::cfg_if! {
            if #[cfg(target_os = "macos")] {
                use crate::SeatbeltSandbox;
                tracing::debug!("SandboxBuilder: selected Seatbelt (macOS)");
                Box::new(SeatbeltSandbox)
            } else if #[cfg(target_os = "linux")] {
                use crate::LandlockSandbox;
                tracing::debug!("SandboxBuilder: selected Landlock (Linux)");
                Box::new(LandlockSandbox)
            } else {
                use crate::NullSandbox;
                tracing::warn!("SandboxBuilder: no native sandbox available, falling back to NullSandbox");
                Box::new(NullSandbox)
            }
        }
    }
}
