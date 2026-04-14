use std::path::PathBuf;
use std::time::{Duration, Instant};

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use hone_proto::AgentEvent;
use tokio::sync::mpsc;

use crate::{
    animation::AnimationState,
    commands::{Command, TddSubcommand, parse_inline_agent},
    input::InputState,
    layout::{FocusTarget, LayoutMode},
    message::{DisplayMessage, DisplayRole},
    shell_escape::ShellEscapeResult,
    status::{ProviderStatus, StatusInfo},
    tdd_mode::{TddEvent, TddWatcher},
    theme::Theme,
    vim_mode::{VimAction, VimMode, VimOverlay},
};

/// Duration that the splash screen is shown before auto-dismissing.
const SPLASH_DURATION: Duration = Duration::from_millis(600);

/// High-level lifecycle state of the application.
#[derive(Debug, Clone, PartialEq)]
pub enum AppState {
    /// Ready for user input.
    Idle,
    /// A turn is in progress; waiting for agent events.
    WaitingForResponse,
    /// An unrecoverable error occurred.
    Error(String),
}

/// Root state of the TUI application.
pub struct App {
    pub(crate) state: AppState,
    pub(crate) input: InputState,
    pub(crate) messages: Vec<DisplayMessage>,
    pub(crate) status: StatusInfo,
    pub(crate) should_quit: bool,
    pub(crate) animation: AnimationState,
    pub(crate) layout: LayoutMode,
    pub(crate) focus: FocusTarget,
    pub(crate) scroll_offset: usize,
    pub(crate) theme: Theme,
    pub(crate) show_help: bool,
    pub(crate) show_splash: bool,
    pub(crate) splash_start: Option<Instant>,
    pub(crate) preview_content: Option<String>,
    pub(crate) preview_path: Option<String>,

    // --- Shell escape -------------------------------------------------------
    /// Current effective working directory for shell escapes.
    pub(crate) working_dir: PathBuf,
    /// Immutable project root — `!cd` cannot escape this boundary.
    /// Set once at startup from the initial working directory.
    project_root: PathBuf,

    // --- Vim mode -----------------------------------------------------------
    /// Whether vim-style keybindings are active.
    pub(crate) vim_enabled: bool,
    /// Vim overlay state machine.
    pub(crate) vim: VimOverlay,

    // --- TDD mode -----------------------------------------------------------
    /// Active TDD watcher, if any.
    pub(crate) tdd_watcher: Option<TddWatcher>,
    /// Channel receiver for TDD events (polled each tick).
    pub(crate) tdd_rx: Option<mpsc::Receiver<TddEvent>>,

    // --- Shell escape suspend signal ----------------------------------------
    /// When `Some`, the event loop must suspend the TUI, run this command
    /// interactively, then restore the TUI and clear this field.
    pub(crate) pending_interactive: Option<(String, PathBuf)>,

    /// Pending non-interactive shell command awaiting async execution.
    pub(crate) pending_shell_cmd: Option<String>,

    // --- Review mode ----------------------------------------------------------
    /// Active review session (hunk-by-hunk accept/reject).
    pub(crate) review_session: Option<crate::review_mode::ReviewSession>,
    /// Set to true when :review is waiting for git diff output.
    pending_review: bool,

    // --- Inline @agent dispatch -----------------------------------------------
    /// When `Some`, the next AI turn will be routed through this named agent
    /// persona.  Cleared after one turn.
    pub(crate) inline_agent: Option<String>,

    // --- Clarify mode ---------------------------------------------------------
    /// When `true`, the next user prompt will be pre-processed through the
    /// clarification detector regardless of heuristics (forced by `/clarify`).
    pub(crate) force_clarify: bool,
}

impl App {
    /// Create a new, empty application.
    pub fn new() -> Self {
        Self {
            state: AppState::Idle,
            input: InputState::new(),
            messages: Vec::new(),
            status: StatusInfo::default(),
            should_quit: false,
            animation: AnimationState::new(),
            layout: LayoutMode::default(),
            focus: FocusTarget::default(),
            scroll_offset: 0,
            theme: Theme::detect(),
            show_help: false,
            show_splash: true,
            splash_start: Some(Instant::now()),
            preview_content: None,
            preview_path: None,
            working_dir: std::env::current_dir().unwrap_or_else(|_| PathBuf::from("/")),
            project_root: std::env::current_dir().unwrap_or_else(|_| PathBuf::from("/")),
            vim_enabled: false,
            vim: VimOverlay::new(),
            tdd_watcher: None,
            tdd_rx: None,
            pending_interactive: None,
            pending_shell_cmd: None,
            review_session: None,
            pending_review: false,
            inline_agent: None,
            force_clarify: false,
        }
    }

    /// Create a new application with vim mode pre-enabled (for `--vim` flag).
    pub fn with_vim() -> Self {
        let mut app = Self::new();
        app.vim_enabled = true;
        app
    }

    /// Immediately dismiss the splash screen (for tests and resume scenarios).
    pub fn skip_splash(&mut self) {
        self.show_splash = false;
        self.splash_start = None;
    }

    // -----------------------------------------------------------------------
    // Accessors
    // -----------------------------------------------------------------------

    pub fn state(&self) -> &AppState {
        &self.state
    }

    pub fn input(&self) -> &InputState {
        &self.input
    }

    pub fn messages(&self) -> &[DisplayMessage] {
        &self.messages
    }

    pub fn status(&self) -> &StatusInfo {
        &self.status
    }

    pub fn status_mut(&mut self) -> &mut StatusInfo {
        &mut self.status
    }

    pub fn should_quit(&self) -> bool {
        self.should_quit
    }

    pub fn theme(&self) -> &Theme {
        &self.theme
    }

    pub fn animation(&self) -> &AnimationState {
        &self.animation
    }

    pub fn layout(&self) -> &LayoutMode {
        &self.layout
    }

    pub fn focus(&self) -> &FocusTarget {
        &self.focus
    }

    pub fn show_help(&self) -> bool {
        self.show_help
    }

    pub fn scroll_offset(&self) -> usize {
        self.scroll_offset
    }

    pub fn show_splash(&self) -> bool {
        self.show_splash
    }

    pub fn preview_content(&self) -> Option<&str> {
        self.preview_content.as_deref()
    }

    pub fn preview_path(&self) -> Option<&str> {
        self.preview_path.as_deref()
    }

    /// Set the file preview content and path directly (for testing and external use).
    pub fn set_preview(&mut self, content: Option<String>, path: Option<String>) {
        self.preview_content = content;
        self.preview_path = path;
    }

    /// The current effective working directory for shell escapes.
    pub fn working_dir(&self) -> &std::path::Path {
        &self.working_dir
    }

    /// Whether vim-style keybindings are active.
    pub fn vim_enabled(&self) -> bool {
        self.vim_enabled
    }

    /// The current vim mode (Normal / Insert).  Only meaningful when
    /// `vim_enabled()` returns `true`.
    pub fn vim_mode(&self) -> VimMode {
        self.vim.mode
    }

    /// Take the pending interactive command, clearing it from the app state.
    /// The event loop should call this after detecting a pending command, run
    /// the command outside the TUI, then resume the TUI loop.
    pub fn take_pending_interactive(&mut self) -> Option<(String, PathBuf)> {
        self.pending_interactive.take()
    }

    /// Whether TDD mode is currently active.
    pub fn tdd_active(&self) -> bool {
        self.tdd_watcher.is_some()
    }

    /// Take the pending inline agent name, clearing it from state.
    ///
    /// The event loop reads this after a submit to know whether to swap the
    /// system prompt for this turn.
    pub fn take_inline_agent(&mut self) -> Option<String> {
        self.inline_agent.take()
    }

    /// Whether the next turn was explicitly forced into clarification mode.
    pub fn take_force_clarify(&mut self) -> bool {
        let v = self.force_clarify;
        self.force_clarify = false;
        v
    }

    /// Set splash_start to a specific instant (for testing auto-dismiss).
    pub fn set_splash_start(&mut self, start: std::time::Instant) {
        self.splash_start = Some(start);
    }

    /// Advance the animation by one tick (call at 100ms intervals).
    pub fn tick(&mut self) {
        self.animation.tick();

        // Sync elapsed time into StatusInfo so the status bar can render it.
        self.status.elapsed_secs = self.animation.elapsed_secs();

        // Auto-dismiss splash after SPLASH_DURATION.
        if self.show_splash {
            if let Some(start) = self.splash_start {
                if start.elapsed() >= SPLASH_DURATION {
                    self.show_splash = false;
                    self.splash_start = None;
                }
            }
        }

        // Drain TDD events.
        self.drain_tdd_events();
    }

    /// Drain any pending TDD events and display them as system messages.
    fn drain_tdd_events(&mut self) {
        if let Some(rx) = &mut self.tdd_rx {
            while let Ok(event) = rx.try_recv() {
                let msg = match &event {
                    TddEvent::Started { command } => {
                        format!("[TDD] watching for changes, running: {command}")
                    }
                    TddEvent::RunningTests { changed_path } => {
                        format!("[TDD] change detected: {} — running tests...", changed_path.display())
                    }
                    TddEvent::Passed { output } => {
                        format!("[TDD] tests PASSED\n{output}")
                    }
                    TddEvent::Failed { output } => {
                        format!("[TDD] tests FAILED\n{output}")
                    }
                    TddEvent::WatchError(e) => {
                        format!("[TDD] watcher error: {e}")
                    }
                    TddEvent::Stopped => {
                        self.tdd_watcher = None;
                        "[TDD] stopped".to_string()
                    }
                };

                let role = match &event {
                    TddEvent::Failed { .. } | TddEvent::WatchError(_) => DisplayRole::Error,
                    _ => DisplayRole::System,
                };
                self.messages.push(DisplayMessage::new(role, msg));
            }
        }

        // If the watcher finished, clean up the receiver too.
        if self.tdd_watcher.is_none() {
            self.tdd_rx = None;
        }
    }

    // -----------------------------------------------------------------------
    // Keyboard handling
    // -----------------------------------------------------------------------

    /// Process a terminal key event and update state accordingly.
    pub fn handle_key(&mut self, key: KeyEvent) {
        // When vim mode is active, route through the overlay first.
        if self.vim_enabled {
            if let Some(action) = self.vim.process(key) {
                self.apply_vim_action(action);
            }
            return;
        }

        // Normal (non-vim) key handling.
        self.handle_key_normal(key);
    }

    /// Apply a vim overlay action to the app state.
    fn apply_vim_action(&mut self, action: VimAction) {
        match action {
            VimAction::EnterInsert | VimAction::EnterNormal => {
                // Mode transition handled by the overlay; nothing else to do.
            }
            VimAction::EnterInsertAtEnd => {
                // Move cursor to end of input buffer.
                self.input.cursor_position = self.input.buffer.len();
            }
            VimAction::ScrollUp(n) => {
                self.scroll_offset = self.scroll_offset.saturating_add(n);
            }
            VimAction::ScrollDown(n) => {
                self.scroll_offset = self.scroll_offset.saturating_sub(n);
            }
            VimAction::ScrollToTop => {
                self.scroll_offset = usize::MAX;
            }
            VimAction::ScrollToBottom => {
                self.scroll_offset = 0;
            }
            VimAction::Quit => {
                self.should_quit = true;
            }
            VimAction::Submit => {
                self.submit_input();
            }
            VimAction::PassThrough(key) => {
                self.handle_key_normal(key);
            }
        }
    }

    /// Non-vim key handling (also used as the passthrough target from vim Insert mode).
    fn handle_key_normal(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.should_quit = true;
            }
            KeyCode::Enter => {
                self.submit_input();
            }
            KeyCode::Backspace => {
                self.input.delete_char();
            }
            KeyCode::Left => {
                self.input.move_cursor_left();
            }
            KeyCode::Right => {
                self.input.move_cursor_right();
            }
            KeyCode::Char(c) => {
                self.input.insert_char(c);
            }
            KeyCode::F(1) => {
                self.show_help = !self.show_help;
            }
            KeyCode::Tab => {
                self.focus = self.focus.cycle();
            }
            KeyCode::PageUp => {
                self.scroll_offset = self.scroll_offset.saturating_add(10);
            }
            KeyCode::PageDown => {
                self.scroll_offset = self.scroll_offset.saturating_sub(10);
            }
            _ => {}
        }
    }

    // -----------------------------------------------------------------------
    // Input submission
    // -----------------------------------------------------------------------

    /// Take the current input, interpret it, and update state.
    ///
    /// Public so tests (and the event loop) can call it directly.
    pub fn submit_input(&mut self) {
        if self.input.is_empty() {
            return;
        }

        let text = self.input.take_input();

        // Shell escape: `!command` bypasses the agent.
        if let Some(cmd) = text.strip_prefix('!') {
            self.initiate_shell_escape(cmd.trim());
            return;
        }

        // Check whether this is a slash-command.
        if let Some(cmd) = Command::parse(&text) {
            self.handle_command(cmd);
            return;
        }

        // @agent-name prefix — inline persona dispatch.
        // Must be checked after slash-commands so `/clarify` is not shadowed.
        if let Some((agent_name, prompt)) = parse_inline_agent(&text) {
            let agent_name = agent_name.to_owned();
            let prompt = prompt.to_owned();

            self.messages.push(DisplayMessage::new(
                DisplayRole::System,
                format!("Switching to @{agent_name} persona for this turn."),
            ));
            self.inline_agent = Some(agent_name);

            let display_text = if prompt.is_empty() {
                // No prompt given — just acknowledge; the user can follow up.
                self.state = AppState::Idle;
                return;
            } else {
                prompt
            };

            self.messages.push(DisplayMessage::new(DisplayRole::User, display_text));
            self.state = AppState::WaitingForResponse;
            return;
        }

        // Plain message — add it to the conversation and start waiting.
        self.messages.push(DisplayMessage::new(DisplayRole::User, text));
        self.state = AppState::WaitingForResponse;
    }

    /// Begin handling a shell escape (`!command`).
    ///
    /// For `cd` and interactive commands we produce a marker message and set
    /// internal state; the actual async execution is performed by
    /// [`App::handle_shell_escape_result`] which the event loop calls after
    /// awaiting the async operation.
    ///
    /// For interactive commands we store the command in `pending_interactive`
    /// and the event loop is responsible for suspending the TUI, running the
    /// command, and then calling us back with the result.
    pub fn initiate_shell_escape(&mut self, cmd: &str) {
        if cmd.is_empty() {
            self.messages.push(DisplayMessage::new(
                DisplayRole::System,
                "shell escape: empty command (usage: !<command>)".to_string(),
            ));
            return;
        }

        // Echo the command so the user can see what ran.
        self.messages.push(DisplayMessage::new(
            DisplayRole::User,
            format!("! {cmd}"),
        ));

        // Detect interactive commands synchronously — no async needed.
        if crate::shell_escape::is_interactive_command(cmd) {
            self.pending_interactive = Some((cmd.to_owned(), self.working_dir.clone()));
            // State stays Idle; the event loop will handle the suspend/restore.
            return;
        }

        // For `cd` and captured commands we need async execution; signal the
        // event loop by entering a new variant state.  We reuse
        // WaitingForResponse semantically — the event loop checks
        // `pending_shell_escape` before spawning an agent turn.
        //
        // We store the command string as the "user text" would be used by the
        // agent, but the event loop intercepts shell escapes before routing to
        // the agent.  We push the execution request via a lightweight signal:
        // the state machine just returns to Idle after the result is processed.
        //
        // For simplicity we directly set a field the event loop polls:
        self.state = AppState::WaitingForResponse;
        // The event loop reads `pending_shell_escape` before deciding whether
        // to spawn an agent turn.  We repurpose the user message text buffer to
        // communicate the command.  See `take_pending_shell_escape`.
        self.pending_shell_cmd = Some(cmd.to_owned());
    }

    /// Take the pending non-interactive shell command (set by
    /// `initiate_shell_escape`).  Returns `None` if no shell escape is pending.
    pub fn take_pending_shell_cmd(&mut self) -> Option<String> {
        self.pending_shell_cmd.take()
    }

    /// Apply the result of an async shell escape execution to the app state.
    pub fn handle_shell_escape_result(&mut self, result: ShellEscapeResult) {
        match result {
            ShellEscapeResult::Output { text, success } => {
                // If this was a review command (git diff), open review session
                // instead of just displaying the raw diff.
                if self.pending_review && success && !text.is_empty() {
                    self.pending_review = false;
                    self.open_review_session(&text);
                    self.state = AppState::Idle;
                    return;
                }
                self.pending_review = false;
                let role = if success { DisplayRole::System } else { DisplayRole::Error };
                self.messages.push(DisplayMessage::new(role, text));
                self.state = AppState::Idle;
            }
            ShellEscapeResult::DirectoryChanged { new_dir } => {
                // Security: confine cd to the project root. The project_root
                // is the initial working_dir set at startup. Allowing cd outside
                // would move the anchor for resolve_within_working_dir, breaking
                // path traversal prevention for the agent's file tools.
                let project_root = self.project_root.clone();
                if new_dir.starts_with(&project_root) {
                    let msg = format!("cd: {}", new_dir.display());
                    self.working_dir = new_dir;
                    self.messages.push(DisplayMessage::new(DisplayRole::System, msg));
                } else {
                    let msg = format!(
                        "cd: {} is outside the project root ({}). Denied for security.",
                        new_dir.display(),
                        project_root.display()
                    );
                    self.messages.push(DisplayMessage::new(DisplayRole::Error, msg));
                }
                self.state = AppState::Idle;
            }
            ShellEscapeResult::CdError(e) => {
                self.messages.push(DisplayMessage::new(DisplayRole::Error, e));
                self.state = AppState::Idle;
            }
            ShellEscapeResult::NeedsTerminalSuspend { command, working_dir } => {
                // This path should not be reached here (handled in initiate_shell_escape
                // before async dispatch), but handle it defensively.
                self.pending_interactive = Some((command, working_dir));
                self.state = AppState::Idle;
            }
        }
    }

    /// Display the result of an interactive command (called after the TUI is
    /// restored and the command has exited).
    pub fn handle_interactive_command_done(&mut self) {
        self.messages.push(DisplayMessage::new(
            DisplayRole::System,
            "(interactive command exited — TUI restored)".to_string(),
        ));
    }

    /// Helper used by tests to populate the input buffer without going through
    /// individual key events.
    pub fn type_text(&mut self, text: &str) {
        for c in text.chars() {
            self.input.insert_char(c);
        }
    }

    // -----------------------------------------------------------------------
    // Command handling
    // -----------------------------------------------------------------------

    fn handle_command(&mut self, cmd: Command) {
        match cmd {
            Command::Quit => {
                self.should_quit = true;
            }
            Command::Model(id) => {
                self.status.model = id;
            }
            Command::Cost => {
                let msg = format!(
                    "Session cost: ${:.6} | tokens: {}",
                    self.status.session_cost_usd, self.status.token_count
                );
                self.messages
                    .push(DisplayMessage::new(DisplayRole::System, msg));
            }
            Command::Help => {
                let help = "\
/configure         — show/change config (model, theme, budget, vim)\n\
/model <id>        — switch model\n\
/cost              — show session cost\n\
/tdd [command]     — start TDD file watcher (:tdd off to stop)\n\
/review            — review session changes (hunk accept/reject)\n\
/clarify [prompt]  — ask clarifying questions before coding\n\
/set vim | novim   — toggle vim keybindings\n\
/theme <name>      — switch theme (crimson, neon, chalk, minimal)\n\
/undo              — undo last action\n\
/redo              — redo\n\
/quit  /q          — quit\n\
/help              — this message\n\
\n\
@agent-name [prompt] — invoke specialist (e.g. @debugger why does test fail?)\n\
!command             — run shell command (e.g. !cargo test, !cd src, !nvim file)";
                self.messages
                    .push(DisplayMessage::new(DisplayRole::System, help));
            }
            Command::Undo | Command::Redo => {
                // Not yet implemented — silently no-op.
            }
            Command::Split => {
                self.layout = self.layout.toggle_split();
            }
            Command::Panel => {
                self.layout = match self.layout {
                    LayoutMode::BottomPanel => LayoutMode::SinglePane,
                    _ => LayoutMode::BottomPanel,
                };
            }
            Command::ThemeSwitch(name) => {
                self.theme = match name.as_str() {
                    "crimson" => Theme::crimson(),
                    "neon" => Theme::neon(),
                    "chalk" | "light" => Theme::chalk(),
                    "minimal" => Theme::minimal(),
                    _ => {
                        self.messages.push(DisplayMessage::new(
                            DisplayRole::Error,
                            format!("Unknown theme: {name}  (try: crimson, neon, chalk, minimal)"),
                        ));
                        return;
                    }
                };
                self.messages.push(DisplayMessage::new(
                    DisplayRole::System,
                    format!("Theme switched to: {name}"),
                ));
            }
            Command::ThemePreview => {
                let t = &self.theme;
                // Build a text-based color swatch as a System message.
                let preview = format!(
                    "Theme preview\n\
                     \n\
                     Accent:  \u{2588}\u{2588}\u{2588}\u{2588} hot  \u{2588}\u{2588}\u{2588}\u{2588} mid  \u{2588}\u{2588}\u{2588}\u{2588} deep\n\
                     Surface: \u{2588}\u{2588}\u{2588}\u{2588} base \u{2588}\u{2588}\u{2588}\u{2588} raised \u{2588}\u{2588}\u{2588}\u{2588} overlay\n\
                     Roles:   user | assistant | tool | error\n\
                     Syntax:  fn let \"str\" 42 // comment Type\n\
                     Diff:    + added  - removed  @@ hunk\n\
                     Budget:  [\u{2588}\u{2588}\u{2588}\u{2588}\u{2588}\u{2588}\u{2588}\u{2588}] low  [\u{2588}\u{2588}\u{2588}\u{2588}\u{2588}\u{2588}\u{2588}\u{2588}] mid  [\u{2588}\u{2588}\u{2588}\u{2588}\u{2588}\u{2588}\u{2588}\u{2588}] high\n\
                     no_color: {}",
                    t.no_color,
                );
                self.messages.push(DisplayMessage::new(DisplayRole::System, preview));
            }
            Command::SetVim(enable) => {
                self.vim_enabled = enable;
                if enable {
                    self.vim = VimOverlay::new();
                }
                let msg = if enable {
                    "vim mode enabled (i/a = insert, Esc = normal, j/k = scroll, gg/G = top/bottom)"
                        .to_string()
                } else {
                    "vim mode disabled".to_string()
                };
                self.messages.push(DisplayMessage::new(DisplayRole::System, msg));
            }
            Command::Tdd(sub) => {
                self.handle_tdd_command(sub);
            }
            Command::Review => {
                self.handle_review_command();
            }
            Command::Clarify(prompt) => {
                self.handle_clarify_command(prompt);
            }
            Command::Configure(arg) => {
                self.handle_configure_command(&arg);
            }
            Command::Unknown(name) => {
                let msg = format!("Unknown command: /{name}  (type /help for help)");
                self.messages
                    .push(DisplayMessage::new(DisplayRole::Error, msg));
            }
        }
    }

    // -----------------------------------------------------------------------
    // TDD command handling
    // -----------------------------------------------------------------------

    fn handle_tdd_command(&mut self, sub: TddSubcommand) {
        match sub {
            TddSubcommand::Stop => {
                if self.tdd_watcher.is_some() {
                    // Drop the watcher — its stop signal fires on drop.
                    self.tdd_watcher = None;
                    self.messages.push(DisplayMessage::new(
                        DisplayRole::System,
                        "[TDD] stopping watcher...".to_string(),
                    ));
                } else {
                    self.messages.push(DisplayMessage::new(
                        DisplayRole::System,
                        "[TDD] not active".to_string(),
                    ));
                }
            }
            TddSubcommand::Start(maybe_cmd) => {
                if self.tdd_watcher.is_some() {
                    self.messages.push(DisplayMessage::new(
                        DisplayRole::System,
                        "[TDD] already active — use :tdd off to stop first".to_string(),
                    ));
                    return;
                }

                // Auto-detect the test command when none was given.
                let test_cmd = maybe_cmd.or_else(|| detect_test_cmd_for_dir(&self.working_dir));

                let Some(cmd) = test_cmd else {
                    self.messages.push(DisplayMessage::new(
                        DisplayRole::Error,
                        "[TDD] could not auto-detect a test command. \
                         Pass one explicitly: :tdd cargo test".to_string(),
                    ));
                    return;
                };

                let (tx, rx) = mpsc::channel::<TddEvent>(64);
                let watcher = TddWatcher::start(self.working_dir.clone(), cmd, tx);
                self.tdd_watcher = Some(watcher);
                self.tdd_rx = Some(rx);
            }
        }
    }

    // -----------------------------------------------------------------------
    // Review command handling
    // -----------------------------------------------------------------------

    fn handle_configure_command(&mut self, arg: &str) {
        let trimmed = arg.trim();

        if trimmed.is_empty() {
            // Show current configuration
            let info = format!(
                "Current Configuration:\n\
                 \n\
                 Model:    {}\n\
                 Mode:     {}\n\
                 Session:  {}\n\
                 Cost:     {}\n\
                 Context:  {}\n\
                 Phase:    {}\n\
                 \n\
                 Commands:\n\
                 /configure model <provider:model>  — change model\n\
                 /configure theme <name>            — change theme (crimson, neon, chalk, minimal)\n\
                 /configure budget <usd>            — set session budget\n\
                 /configure vim on|off              — toggle vim keybindings\n\
                 \n\
                 For full setup: exit and run 'hone configure'",
                if self.status.model.is_empty() { "none" } else { &self.status.model },
                &self.status.mode,
                self.status.session_name.as_deref().unwrap_or("(unnamed)"),
                self.status.format_cost(),
                self.status.format_context(),
                self.status.phase_long(),
            );
            self.messages.push(DisplayMessage::new(DisplayRole::System, info));
            return;
        }

        // Parse subcommands
        let (subcmd, value) = match trimmed.split_once(char::is_whitespace) {
            Some((k, v)) => (k.trim(), v.trim()),
            None => (trimmed, ""),
        };

        match subcmd {
            "model" if !value.is_empty() => {
                self.status.model = value.to_string();
                self.messages.push(DisplayMessage::new(
                    DisplayRole::System,
                    format!("Model changed to: {value}  (takes effect on next turn)"),
                ));
                // Note: actually swapping the provider requires the event loop in main.rs
                // to pick this up. We store it and let main.rs handle the swap.
                self.pending_shell_cmd = Some(format!("__configure_model:{value}"));
            }
            "theme" if !value.is_empty() => {
                // Dispatch to existing theme switch
                self.handle_command(Command::ThemeSwitch(value.to_string()));
            }
            "budget" if !value.is_empty() => {
                match value.parse::<f64>() {
                    Ok(usd) => {
                        self.status.budget_usd = Some(usd);
                        self.messages.push(DisplayMessage::new(
                            DisplayRole::System,
                            format!("Session budget set to ${usd:.2}"),
                        ));
                    }
                    Err(_) => {
                        self.messages.push(DisplayMessage::new(
                            DisplayRole::Error,
                            format!("Invalid budget: '{value}' — use a number like 1.00"),
                        ));
                    }
                }
            }
            "vim" => {
                let enable = matches!(value, "on" | "true" | "1" | "yes" | "");
                self.handle_command(Command::SetVim(enable));
            }
            _ => {
                self.messages.push(DisplayMessage::new(
                    DisplayRole::Error,
                    format!("Unknown configure option: '{subcmd}'. Try: model, theme, budget, vim"),
                ));
            }
        }
    }

    fn handle_review_command(&mut self) {
        self.messages.push(DisplayMessage::new(
            DisplayRole::System,
            "Generating session diff...".to_string(),
        ));
        self.pending_review = true;
        self.pending_shell_cmd = Some("git diff".to_owned());
        self.state = AppState::WaitingForResponse;
    }

    // -----------------------------------------------------------------------
    // Clarify command handling
    // -----------------------------------------------------------------------

    fn handle_clarify_command(&mut self, prompt: String) {
        if prompt.is_empty() {
            // No inline prompt — set the flag so the next regular message
            // is forced through clarification regardless of heuristics.
            self.force_clarify = true;
            self.messages.push(DisplayMessage::new(
                DisplayRole::System,
                "[Clarify mode] Your next message will trigger a clarification step.".to_string(),
            ));
        } else {
            // Prompt provided inline — synthesise clarifying questions now.
            use hone_core::ClarificationDetector;
            let questions = ClarificationDetector::needs_clarification(&prompt)
                .unwrap_or_else(|| {
                    vec!["Could you provide more details about what you'd like me to do?".into()]
                });
            let formatted = ClarificationDetector::format_questions(&questions);
            self.messages.push(DisplayMessage::new(DisplayRole::User, prompt.clone()));
            self.messages.push(DisplayMessage::new(DisplayRole::System, formatted));
            // Mark force_clarify so the event loop knows this turn has been
            // pre-processed; the user's answer will go straight to the agent.
            self.force_clarify = true;
        }
    }

    /// Open a review session from a diff string.
    pub fn open_review_session(&mut self, diff: &str) {
        use crate::review_mode::ReviewSession;
        let session = ReviewSession::from_diff(diff);
        if session.total_count() == 0 {
            self.messages.push(DisplayMessage::new(
                DisplayRole::System,
                "No changes to review.".to_string(),
            ));
            return;
        }
        let hunk_display = session.format_current_for_display();
        let header = format!("[Review mode] {} hunks to review. Keys: a=accept, r=reject, j=next, k=prev, A=accept all, R=reject all, q=finish", session.total_count());
        self.messages.push(DisplayMessage::new(
            DisplayRole::System,
            format!("{header}\n\n{hunk_display}"),
        ));
        self.review_session = Some(session);
    }

    /// Handle a key press in review mode. Returns true if the key was consumed.
    pub fn handle_review_key(&mut self, key: crossterm::event::KeyCode) -> bool {
        use crossterm::event::KeyCode;

        let Some(session) = &mut self.review_session else {
            return false;
        };

        match key {
            KeyCode::Char('a') => {
                session.accept_current();
            }
            KeyCode::Char('r') => {
                session.reject_current();
            }
            KeyCode::Char('A') => {
                session.accept_all();
            }
            KeyCode::Char('R') => {
                session.reject_all();
            }
            KeyCode::Char('j') | KeyCode::Down => {
                session.next();
            }
            KeyCode::Char('k') | KeyCode::Up => {
                session.prev();
            }
            KeyCode::Char('q') | KeyCode::Esc => {
                let accepted = session.accepted_count();
                let rejected = session.rejected_count();
                let total = session.total_count();
                self.messages.push(DisplayMessage::new(
                    DisplayRole::System,
                    format!("[Review complete] {accepted} accepted, {rejected} rejected out of {total} hunks."),
                ));
                if accepted > 0 {
                    self.messages.push(DisplayMessage::new(
                        DisplayRole::System,
                        "Accepted changes are in your working tree. Use `!git add -p` to stage selectively or `!git commit -am 'message'` to commit all.".to_string(),
                    ));
                }
                self.review_session = None;
                return true;
            }
            _ => return false,
        }

        // Update display with current hunk
        if let Some(session) = &self.review_session {
            if session.is_complete() {
                let accepted = session.accepted_count();
                let rejected = session.rejected_count();
                let total = session.total_count();
                self.messages.push(DisplayMessage::new(
                    DisplayRole::System,
                    format!("[Review complete] {accepted} accepted, {rejected} rejected out of {total} hunks. Press q to exit review mode."),
                ));
            } else {
                let display = session.format_current_for_display();
                self.messages.push(DisplayMessage::new(
                    DisplayRole::System,
                    display,
                ));
            }
        }

        true
    }

    /// Whether review mode is currently active.
    pub fn is_review_active(&self) -> bool {
        self.review_session.is_some()
    }

    // -----------------------------------------------------------------------
    // Agent event handling
    // -----------------------------------------------------------------------

    /// Consume an event from the agent loop and update the display state.
    pub fn handle_agent_event(&mut self, event: AgentEvent) {
        match event {
            AgentEvent::Token { text } => {
                // Append to the last message if it is a streaming assistant message;
                // otherwise create a new one.
                if let Some(last) = self.messages.last_mut() {
                    if last.is_streaming && matches!(last.role, DisplayRole::Assistant) {
                        last.content.push_str(&text);
                        return;
                    }
                }
                let mut msg = DisplayMessage::streaming(DisplayRole::Assistant);
                msg.content.push_str(&text);
                self.messages.push(msg);
            }

            AgentEvent::ToolCallStart { name, .. } => {
                self.status.update_phase_from_tool(&name);
                self.status.active_tool = Some(name.clone());
                self.status.provider_status = ProviderStatus::Connected;
                self.messages.push(DisplayMessage::new(
                    DisplayRole::ToolCall { name },
                    String::new(),
                ));
            }

            AgentEvent::ToolCallEnd { result, is_error, .. } => {
                self.status.active_tool = None;
                let content = if is_error {
                    format!("error: {result}")
                } else {
                    result.to_string()
                };

                // If the result looks like file content (non-error, string value),
                // store it in the preview pane.
                if !is_error {
                    if let Some(text) = result.as_str() {
                        self.preview_content = Some(text.to_string());
                    }
                }

                self.messages
                    .push(DisplayMessage::new(DisplayRole::ToolResult, content));
            }

            AgentEvent::Done { usage } => {
                // Mark the last streaming message as complete.
                if let Some(last) = self.messages.last_mut() {
                    if last.is_streaming {
                        last.is_streaming = false;
                    }
                }
                self.status.token_count += usage.total();
                // Update context window estimate from cumulative token usage.
                self.status.context_tokens = usage.total();
                // Estimate cost: $3/M input, $15/M output (Sonnet pricing).
                let cost =
                    (usage.input as f64 * 3.0 + usage.output as f64 * 15.0) / 1_000_000.0;
                self.status.session_cost_usd += cost;
                self.status.active_tool = None;
                self.status.turn_count += 1;
                self.status.provider_status = ProviderStatus::Connected;
                self.state = AppState::Idle;
            }

            AgentEvent::Error { message } => {
                self.messages
                    .push(DisplayMessage::new(DisplayRole::Error, message.clone()));
                self.status.provider_status = ProviderStatus::Error(message.clone());
                self.status.active_tool = None;
                self.state = AppState::Error(message);
            }

            AgentEvent::ApprovalRequired { description, .. } => {
                self.messages.push(DisplayMessage::new(
                    DisplayRole::System,
                    format!("Approval required: {description}"),
                ));
            }

            AgentEvent::PlanCreated { goal, subtask_count } => {
                self.messages.push(DisplayMessage::new(
                    DisplayRole::System,
                    format!("Orchestrating: {goal} ({subtask_count} subtasks)"),
                ));
            }

            AgentEvent::WorkerStarted { task_id, agent, description } => {
                self.messages.push(DisplayMessage::new(
                    DisplayRole::System,
                    format!("[{task_id}] {agent}: {description}"),
                ));
            }

            AgentEvent::WorkerCompleted { task_id, agent, status, cost_usd } => {
                self.messages.push(DisplayMessage::new(
                    DisplayRole::System,
                    format!("[{task_id}] {agent}: {status} (${cost_usd:.4})"),
                ));
            }

            AgentEvent::BudgetProgress { spent_usd, budget_usd, workers_completed, workers_total } => {
                self.status.session_cost_usd = spent_usd;
                if budget_usd.is_some() {
                    self.status.budget_usd = budget_usd;
                }
                self.status.mode = format!("orchestrator {workers_completed}/{workers_total}");
                self.status.phase = "Orchestrating".to_string();
                self.status.provider_status = ProviderStatus::Connected;
            }
        }
    }
}

impl Default for App {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Detect an appropriate test command for `dir` based on project manifests.
fn detect_test_cmd_for_dir(dir: &std::path::Path) -> Option<String> {
    if dir.join("Cargo.toml").exists() {
        return Some("cargo test".to_string());
    }
    if dir.join("package.json").exists() {
        return Some("npm test".to_string());
    }
    if dir.join("pyproject.toml").exists() || dir.join("requirements.txt").exists() {
        return Some("pytest".to_string());
    }
    if dir.join("Makefile").exists() {
        return Some("make test".to_string());
    }
    if dir.join("go.mod").exists() {
        return Some("go test ./...".to_string());
    }
    None
}
