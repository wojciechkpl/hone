use std::path::PathBuf;

use crate::error::SandboxError;

/// Operating mode that controls how aggressively the sandbox is applied.
///
/// The mode is informational for the policy builder's validation logic; the
/// actual enforcement is determined by whichever [`crate::Sandbox`] backend
/// is active.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SandboxMode {
    /// The operator configures the policy by hand.  No minimum-path
    /// requirements are enforced at build time.
    Manual,

    /// A human is watching and can approve or reject individual actions.
    /// Like [`SandboxMode::Manual`], no build-time path requirements.
    Supervised,

    /// The agent runs fully autonomously without human oversight.
    /// At least one readable path must be declared so the agent can do
    /// useful work while remaining confined.
    Autonomous,
}

/// Declarative description of what a sandboxed child process is allowed to do.
///
/// Build instances through [`SandboxPolicyBuilder`]; never construct directly,
/// as the builder enforces cross-field invariants.
#[derive(Debug, Clone)]
pub struct SandboxPolicy {
    /// The mode for which this policy was built.
    pub mode: SandboxMode,
    /// Filesystem paths the child may open for reading.
    pub allow_read: Vec<PathBuf>,
    /// Filesystem paths the child may open for writing (implies read).
    pub allow_write: Vec<PathBuf>,
    /// Executables the child is permitted to `exec`.
    pub allow_exec: Vec<PathBuf>,
    /// Whether any outbound network connections are permitted.
    pub allow_network: bool,
    /// If `allow_network` is `true`, the set of hostnames the child may
    /// connect to.  An empty list means all destinations are permitted.
    pub allowed_domains: Vec<String>,
    /// Environment variable names the child is allowed to inherit.
    pub allow_env: Vec<String>,
}

impl SandboxPolicy {
    /// Return a permissive policy that places no meaningful restrictions on the
    /// child process.
    ///
    /// The policy uses [`SandboxMode::Manual`] (the least restrictive mode
    /// semantically, with no build-time path requirements) and leaves all
    /// allow-lists empty.  When passed to the [`NullSandbox`] backend — or any
    /// backend that respects the fail-open manual mode — it imposes zero
    /// overhead.
    ///
    /// This is the default for [`AgentConfig::sandbox_policy`] and
    /// [`ToolContext::sandbox_policy`] when no per-agent profile is configured.
    pub fn permissive() -> Self {
        Self {
            mode: SandboxMode::Manual,
            allow_read: Vec::new(),
            allow_write: Vec::new(),
            allow_exec: Vec::new(),
            allow_network: true,
            allowed_domains: Vec::new(),
            allow_env: Vec::new(),
        }
    }
}

/// Builder for [`SandboxPolicy`].
///
/// # Example
///
/// ```rust
/// use hone_sandbox::{SandboxMode, SandboxPolicyBuilder};
/// use std::path::PathBuf;
///
/// let policy = SandboxPolicyBuilder::new(SandboxMode::Manual)
///     .allow_read(PathBuf::from("/workspace"))
///     .allow_write(PathBuf::from("/workspace/output"))
///     .allow_env("PATH")
///     .build()
///     .unwrap();
/// ```
#[derive(Debug)]
pub struct SandboxPolicyBuilder {
    mode: SandboxMode,
    allow_read: Vec<PathBuf>,
    allow_write: Vec<PathBuf>,
    allow_exec: Vec<PathBuf>,
    allow_network: bool,
    allowed_domains: Vec<String>,
    allow_env: Vec<String>,
}

impl SandboxPolicyBuilder {
    /// Create a new builder for the given [`SandboxMode`].
    pub fn new(mode: SandboxMode) -> Self {
        Self {
            mode,
            allow_read: Vec::new(),
            allow_write: Vec::new(),
            allow_exec: Vec::new(),
            allow_network: false,
            allowed_domains: Vec::new(),
            allow_env: Vec::new(),
        }
    }

    /// Permit read access to `path`.
    #[must_use]
    pub fn allow_read(mut self, path: PathBuf) -> Self {
        self.allow_read.push(path);
        self
    }

    /// Permit write access to `path` (implies read access).
    #[must_use]
    pub fn allow_write(mut self, path: PathBuf) -> Self {
        self.allow_write.push(path);
        self
    }

    /// Permit the child to exec the binary at `path`.
    #[must_use]
    pub fn allow_exec(mut self, path: PathBuf) -> Self {
        self.allow_exec.push(path);
        self
    }

    /// Enable or disable outbound network access entirely.
    #[must_use]
    pub fn allow_network(mut self, enabled: bool) -> Self {
        self.allow_network = enabled;
        self
    }

    /// Permit connections to a specific domain (only meaningful when
    /// `allow_network` is `true`).
    #[must_use]
    pub fn allow_domain(mut self, domain: impl Into<String>) -> Self {
        self.allowed_domains.push(domain.into());
        self
    }

    /// Allow the child to inherit the environment variable named `var`.
    #[must_use]
    pub fn allow_env(mut self, var: impl Into<String>) -> Self {
        self.allow_env.push(var.into());
        self
    }

    /// Validate and construct the [`SandboxPolicy`].
    ///
    /// # Errors
    ///
    /// Returns [`SandboxError::InvalidPolicy`] when:
    /// - `mode` is [`SandboxMode::Autonomous`] and no readable paths were
    ///   declared (the agent would have no useful filesystem surface).
    pub fn build(self) -> Result<SandboxPolicy, SandboxError> {
        if self.mode == SandboxMode::Autonomous && self.allow_read.is_empty() {
            return Err(SandboxError::InvalidPolicy {
                reason: "autonomous mode requires at least one readable path".to_string(),
            });
        }

        Ok(SandboxPolicy {
            mode: self.mode,
            allow_read: self.allow_read,
            allow_write: self.allow_write,
            allow_exec: self.allow_exec,
            allow_network: self.allow_network,
            allowed_domains: self.allowed_domains,
            allow_env: self.allow_env,
        })
    }
}
