//! Kernel-enforced sandboxing for Hone tool execution.
//!
//! # Overview
//!
//! `hone-sandbox` provides a platform-agnostic interface for restricting what
//! tool-executed child processes can do: filesystem reads and writes, process
//! execution, and network access.
//!
//! ## Fail-closed design
//!
//! Every operation that cannot be enforced returns an [`Err`] rather than
//! silently degrading.  Callers **must** propagate errors and never proceed
//! when a sandbox setup fails.
//!
//! ## Quick start
//!
//! ```rust,no_run
//! use hone_sandbox::{Sandbox, SandboxBuilder, SandboxMode, SandboxPolicyBuilder};
//! use std::path::PathBuf;
//!
//! let policy = SandboxPolicyBuilder::new(SandboxMode::Manual)
//!     .allow_read(PathBuf::from("/workspace"))
//!     .allow_write(PathBuf::from("/workspace/output"))
//!     .allow_env("PATH")
//!     .build()
//!     .unwrap();
//!
//! let sandbox = SandboxBuilder::detect();
//! // apply() calls sandbox_init(3) on macOS — irreversible for the process.
//! let _handle = sandbox.apply(&policy).unwrap();
//! ```
//!
//! ## Platform backends
//!
//! | Platform | Backend                         |
//! |----------|---------------------------------|
//! | macOS    | [`SeatbeltSandbox`] (stub)       |
//! | Linux    | [`LandlockSandbox`] (stub)       |
//! | Fallback | [`ContainerSandbox`] / [`NullSandbox`] |

mod builder;
mod container;
mod error;
mod null;
mod policy;
mod sandbox;

#[cfg(target_os = "linux")]
mod landlock;

#[cfg(target_os = "macos")]
mod seatbelt;

pub use builder::SandboxBuilder;
pub use container::ContainerSandbox;
pub use error::SandboxError;
pub use null::NullSandbox;
pub use policy::{SandboxMode, SandboxPolicy, SandboxPolicyBuilder};
pub use sandbox::{Sandbox, SandboxHandle, SandboxPlatform};

#[cfg(target_os = "linux")]
pub use landlock::LandlockSandbox;

#[cfg(target_os = "macos")]
pub use seatbelt::{SeatbeltProfile, SeatbeltSandbox};
