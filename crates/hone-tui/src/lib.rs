pub mod animation;
pub mod app;
pub mod commands;
pub mod error;
pub mod governance;
pub mod history;
pub mod input;
pub mod layout;
pub mod message;
pub mod plan_preview;
pub mod render;
pub mod review_mode;
pub mod selection;
pub mod session_switcher;
pub mod shell_escape;
pub mod status;
pub mod syntax;
pub mod tdd_mode;
pub mod theme;
pub mod turn_ledger;
pub mod user_commands;
pub mod vim_mode;

pub use animation::AnimationState;
pub use app::{App, AppState};
pub use error::TuiError;
pub use layout::{FocusTarget, LayoutMode};
pub use review_mode::ReviewSession;
pub use session_switcher::SessionSummary;
pub use shell_escape::{
    ShellEscapeResult, execute_shell_escape, is_interactive_command, run_interactive_blocking,
};
pub use tdd_mode::{TddEvent, TddWatcher};
pub use user_commands::{UserCommand, UserCommandError, UserCommandRegistry};
pub use vim_mode::{VimAction, VimMode, VimOverlay};
