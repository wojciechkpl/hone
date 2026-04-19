use std::path::PathBuf;
use std::time::{Duration, Instant};

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use hone_proto::AgentEvent;
use tokio::sync::mpsc;

use crate::{
    animation::AnimationState,
    commands::{Command, TddSubcommand, parse_inline_agent},
    history::PromptHistory,
    input::InputState,
    layout::{FocusTarget, LayoutMode},
    message::{DisplayMessage, DisplayRole},
    shell_escape::ShellEscapeResult,
    status::{ProviderStatus, StatusInfo},
    tdd_mode::{TddEvent, TddWatcher},
    theme::Theme,
    turn_ledger::{Phase, TurnLedger, extract_path, tool_op},
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

    /// Pending model change — the model ID string to apply on the next event
    /// loop iteration.  Set by `/model <id>` and `/configure model <id>`.
    pub(crate) pending_model_change: Option<String>,

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

    // --- Agent rail / TurnLedger --------------------------------------------
    /// Whether the agent rail sidebar is currently visible.
    pub(crate) agent_rail_visible: bool,
    /// Ledger for the turn currently in progress.
    pub(crate) current_turn: Option<TurnLedger>,
    /// Sealed ledgers for completed turns (most recent last).
    pub(crate) completed_turns: Vec<TurnLedger>,
    /// Rail scroll offset (j/k navigation within the rail).
    pub(crate) rail_scroll: usize,
    /// Pending tool call ID → name mapping for ToolCallEnd duration tracking.
    pub(crate) pending_tool_start: Option<(String, String, serde_json::Value)>,
    /// Currently highlighted row in the autocomplete dropdown (0-based).
    /// `None` means no row is selected.
    pub(crate) completion_index: Option<usize>,
    /// Index of the message that has keyboard focus in the conversation pane.
    /// `None` means no message is focused.
    pub(crate) focus_msg_index: Option<usize>,
    /// Set by Escape during WaitingForResponse; the event loop aborts the task
    /// and calls cancel_turn().
    pub(crate) pending_cancel: bool,
    /// Message typed while a turn was active; restored to the input box on Done.
    pub(crate) queued_message: Option<String>,

    // --- Prompt history -----------------------------------------------------
    /// Persistent per-session prompt history (Up/Down navigation, Ctrl+F search).
    pub(crate) history: PromptHistory,
    /// Whether the history-search overlay is currently open.
    pub(crate) history_search_open: bool,
    /// The query string currently typed in the history-search overlay.
    pub(crate) history_search_query: String,
    /// Currently highlighted row in the history-search overlay (0-based, None = none).
    pub(crate) history_search_index: Option<usize>,
    // --- Command palette (Ctrl+K) -------------------------------------------
    /// Whether the command palette overlay is open.
    pub palette_open: bool,
    /// Current filter query typed in the palette.
    pub(crate) palette_query: String,
    /// Currently selected row in the filtered palette list (0-based).
    pub(crate) palette_selection: usize,
    // --- Scrollback search (Ctrl+/) -----------------------------------------
    /// Whether the scrollback search bar is open.
    pub search_open: bool,
    /// Current search query string.
    pub(crate) search_query: String,
    /// Indices into `self.messages` that match the current query.
    pub(crate) search_matches: Vec<usize>,
    /// Current match index into `search_matches`.
    pub(crate) search_cursor: usize,
    // --- Session Anatomy overlay --------------------------------------------
    /// Whether the Session Anatomy full-screen overlay is open.
    pub anatomy_open: bool,
    /// Active tab: 0 = Phase Timeline, 1 = Memory Graph.
    pub(crate) anatomy_tab: usize,
    /// Vertical scroll offset within the anatomy overlay.
    pub(crate) anatomy_scroll: usize,
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
            pending_model_change: None,
            review_session: None,
            pending_review: false,
            inline_agent: None,
            force_clarify: false,
            agent_rail_visible: false,
            current_turn: None,
            completed_turns: Vec::new(),
            rail_scroll: 0,
            pending_tool_start: None,
            completion_index: None,
            focus_msg_index: None,
            pending_cancel: false,
            queued_message: None,
            history: PromptHistory::load(),
            history_search_open: false,
            history_search_query: String::new(),
            history_search_index: None,
            palette_open: false,
            palette_query: String::new(),
            palette_selection: 0,
            search_open: false,
            search_query: String::new(),
            search_matches: Vec::new(),
            search_cursor: 0,
            anatomy_open: false,
            anatomy_tab: 0,
            anatomy_scroll: 0,
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

    pub fn completion_index(&self) -> Option<usize> {
        self.completion_index
    }

    /// Signal that the current agent turn should be aborted.
    /// The event loop calls `cancel_turn()` after aborting the task.
    pub fn request_cancel(&mut self) {
        self.pending_cancel = true;
    }

    /// Drain the cancel flag; returns `true` once after `request_cancel`.
    pub fn take_pending_cancel(&mut self) -> bool {
        std::mem::take(&mut self.pending_cancel)
    }

    /// Immediately reset to Idle after the task has been aborted externally.
    pub fn cancel_turn(&mut self) {
        if let Some(msg) = self.messages.last_mut() {
            msg.is_streaming = false;
        }
        self.status.active_tool = None;
        self.state = AppState::Idle;
        // Clear stale per-turn tracking so the next turn starts fresh.
        self.pending_tool_start = None;
        self.current_turn = None;
        self.messages.push(DisplayMessage::new(
            DisplayRole::System,
            "⚡ interrupted".to_string(),
        ));
    }

    /// If idle or errored and a queued follow-up message exists, restore it to the input box.
    pub fn restore_queued_message_if_idle(&mut self) {
        if matches!(self.state, AppState::Idle | AppState::Error(_)) {
            if let Some(msg) = self.queued_message.take() {
                self.input.buffer = msg;
                self.input.cursor_position = self.input.buffer.len();
            }
        }
    }

    pub fn has_queued_message(&self) -> bool {
        self.queued_message.is_some()
    }

    /// Returns the current autocomplete matches and sigil for the input buffer.
    pub fn active_completions(&self) -> (Vec<&'static str>, char) {
        let buf = &self.input.buffer;
        if buf.starts_with('/') {
            (Command::completions(&buf[1..]), '/')
        } else if buf.starts_with('@') {
            (Command::agent_completions(&buf[1..]), '@')
        } else {
            (vec![], '/')
        }
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

    /// Return the current conversation message focus index.
    pub fn focus_msg_index(&self) -> Option<usize> {
        self.focus_msg_index
    }

    /// Toggle `is_collapsed` on the currently focused message if it is a `ToolResult`.
    /// Non-ToolResult messages are not collapsed regardless of the flag.
    fn toggle_focused_message_collapse(&mut self) {
        if let Some(idx) = self.focus_msg_index {
            if let Some(msg) = self.messages.get_mut(idx) {
                if matches!(msg.role, DisplayRole::ToolResult) {
                    msg.is_collapsed = !msg.is_collapsed;
                }
            }
        }
    }

    // --- Agent rail ---------------------------------------------------------

    /// Toggle the agent rail sidebar on/off.
    pub fn toggle_agent_rail(&mut self) {
        self.agent_rail_visible = !self.agent_rail_visible;
        self.rail_scroll = 0;
    }

    /// Whether the agent rail is currently visible.
    pub fn agent_rail_visible(&self) -> bool {
        self.agent_rail_visible
    }

    /// The active turn ledger: in-progress if a turn is running, otherwise the
    /// most recent completed turn.
    pub fn active_ledger(&self) -> Option<&TurnLedger> {
        self.current_turn
            .as_ref()
            .or_else(|| self.completed_turns.last())
    }

    /// Completed turn ledgers (read-only slice for reports).
    pub fn completed_turns(&self) -> &[TurnLedger] {
        &self.completed_turns
    }

    /// Rail scroll offset.
    pub fn rail_scroll(&self) -> usize {
        self.rail_scroll
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

        // Advance hint rotation tick.
        self.status.hint_tick = self.status.hint_tick.wrapping_add(1);

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
        // Session Anatomy overlay consumes all keys while open.
        if self.anatomy_open {
            self.handle_key_anatomy(key);
            return;
        }
        // Command palette takes priority over everything.
        if self.palette_open {
            self.handle_key_palette(key);
            return;
        }
        // Scrollback search takes priority over normal input.
        if self.search_open {
            self.handle_key_search(key);
            return;
        }
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
        // --- History-search overlay intercepts most keys when open. -----------
        if self.history_search_open {
            self.handle_key_history_search(key);
            return;
        }

        match key.code {
            KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.should_quit = true;
            }
            // Ctrl+F — open history-search overlay.
            KeyCode::Char('f') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.history_search_open = true;
                self.history_search_query.clear();
                self.history_search_index = None;
                return;
            }
            // Ctrl+K — open command palette.
            KeyCode::Char('k') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.palette_open = true;
                self.palette_query.clear();
                self.palette_selection = 0;
            }
            // Ctrl+/ — open scrollback search.
            KeyCode::Char('/') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.search_open = true;
                self.search_query.clear();
                self.recompute_search_matches();
            }
            // Ctrl+A — toggle Session Anatomy overlay.
            KeyCode::Char('a') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.anatomy_open = !self.anatomy_open;
                self.anatomy_scroll = 0;
            }
            // Ctrl+R — toggle agent rail sidebar.
            KeyCode::Char('r') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.toggle_agent_rail();
            }
            // Alt+Right — widen left pane (+5%, ceiling 70).
            KeyCode::Right if key.modifiers.contains(KeyModifiers::ALT) => {
                self.layout = self.layout.adjust_ratio(5);
            }
            // Alt+Left — narrow left pane (-5%, floor 30).
            KeyCode::Left if key.modifiers.contains(KeyModifiers::ALT) => {
                self.layout = self.layout.adjust_ratio(-5);
            }
            // Shift+Enter or Alt+Enter — insert a literal newline without submitting.
            KeyCode::Enter
                if key.modifiers.contains(KeyModifiers::SHIFT)
                    || key.modifiers.contains(KeyModifiers::ALT) =>
            {
                self.input.insert_char('\n');
            }
            KeyCode::Enter => {
                // When conversation pane is focused, toggle collapse on the focused message.
                if self.focus == FocusTarget::Conversation {
                    self.toggle_focused_message_collapse();
                    return;
                }
                // Accept highlighted completion if one is selected.
                let (matches, sigil) = self.active_completions();
                let count = matches.len().min(8);
                if let Some(idx) = self.completion_index {
                    if idx < count {
                        let completed = format!("{sigil}{} ", matches[idx]);
                        self.input.cursor_position = completed.len();
                        self.input.buffer = completed;
                        self.completion_index = None;
                        return;
                    }
                }
                self.submit_input();
            }
            KeyCode::Char(' ') if self.focus == FocusTarget::Conversation => {
                self.toggle_focused_message_collapse();
            }
            KeyCode::Char('j') if self.focus == FocusTarget::Conversation => {
                let count = self.messages.len();
                if count > 0 {
                    self.focus_msg_index = Some(match self.focus_msg_index {
                        None => 0,
                        Some(i) => (i + 1).min(count - 1),
                    });
                }
            }
            KeyCode::Char('k') if self.focus == FocusTarget::Conversation => {
                if let Some(i) = self.focus_msg_index {
                    self.focus_msg_index = Some(i.saturating_sub(1));
                }
            }
            KeyCode::Up => {
                // When conversation pane is focused, Up navigates focus_msg_index.
                if self.focus == FocusTarget::Conversation {
                    if let Some(i) = self.focus_msg_index {
                        self.focus_msg_index = Some(i.saturating_sub(1));
                    }
                    return;
                }
                let (matches, _) = self.active_completions();
                let count = matches.len().min(8);
                if count > 0 {
                    // Autocomplete popup is open — navigate it.
                    self.completion_index = Some(match self.completion_index {
                        None | Some(0) => count - 1,
                        Some(i) => i - 1,
                    });
                } else if !self.input.buffer.starts_with('/')
                    && !self.input.buffer.starts_with('@')
                {
                    // No autocomplete and no sigil prefix — navigate history.
                    let live = self.input.buffer.clone();
                    if let Some(entry) = self.history.prev(&live) {
                        self.input.buffer = entry.to_string();
                        self.input.cursor_position = self.input.buffer.len();
                    }
                } else {
                    // Sigil prefix with no completion — scroll conversation.
                    self.scroll_offset = self.scroll_offset.saturating_add(1);
                }
            }
            KeyCode::Down => {
                // When conversation pane is focused, Down navigates focus_msg_index.
                if self.focus == FocusTarget::Conversation {
                    let count = self.messages.len();
                    if count > 0 {
                        self.focus_msg_index = Some(match self.focus_msg_index {
                            None => 0,
                            Some(i) => (i + 1).min(count - 1),
                        });
                    }
                    return;
                }
                let (matches, _) = self.active_completions();
                let count = matches.len().min(8);
                if count > 0 {
                    // Autocomplete popup is open — navigate it.
                    self.completion_index = Some(match self.completion_index {
                        None => 0,
                        Some(i) => (i + 1) % count,
                    });
                } else if !self.input.buffer.starts_with('/')
                    && !self.input.buffer.starts_with('@')
                    && self.history.is_navigating()
                {
                    // In history navigation mode — go forward.
                    match self.history.next() {
                        Some(entry) => {
                            self.input.buffer = entry.to_string();
                            self.input.cursor_position = self.input.buffer.len();
                        }
                        None => {
                            // Past the end — restore the live draft.
                            let draft = self.history.draft().to_string();
                            self.input.buffer = draft;
                            self.input.cursor_position = self.input.buffer.len();
                        }
                    }
                } else if !self.input.buffer.starts_with('/')
                    && !self.input.buffer.starts_with('@')
                {
                    // Not in history mode and no autocomplete — scroll conversation.
                    self.scroll_offset = self.scroll_offset.saturating_sub(1);
                } else {
                    self.scroll_offset = self.scroll_offset.saturating_sub(1);
                }
            }
            KeyCode::Backspace => {
                self.input.delete_char();
                self.completion_index = None;
            }
            KeyCode::Left => {
                self.input.move_cursor_left();
            }
            KeyCode::Right => {
                self.input.move_cursor_right();
            }
            KeyCode::Char(c) => {
                self.input.insert_char(c);
                self.completion_index = None;
            }
            KeyCode::F(1) => {
                self.show_help = !self.show_help;
            }
            KeyCode::Tab => {
                let (matches, sigil) = self.active_completions();
                let count = matches.len().min(8);
                let pick = self.completion_index.filter(|&i| i < count).unwrap_or(0);
                if count > 0 {
                    let completed = format!("{sigil}{} ", matches[pick]);
                    self.input.cursor_position = completed.len();
                    self.input.buffer = completed;
                    self.completion_index = None;
                } else {
                    self.focus = self.focus.cycle();
                }
            }
            KeyCode::Esc => {
                if matches!(self.state, AppState::WaitingForResponse) {
                    self.request_cancel();
                } else if matches!(self.state, AppState::Idle) {
                    self.input.buffer.clear();
                    self.input.cursor_position = 0;
                    self.completion_index = None;
                }
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

    /// Handle a key event while the history-search overlay is open.
    fn handle_key_history_search(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Esc => {
                // Cancel — close overlay, leave input unchanged.
                self.history_search_open = false;
                self.history_search_index = None;
            }
            KeyCode::Enter => {
                // Accept the highlighted entry (or the first match).
                let matches = self.history.search(&self.history_search_query, 8);
                let idx = self.history_search_index.unwrap_or(0);
                if let Some(entry) = matches.get(idx) {
                    let text = (*entry).to_string();
                    self.input.buffer = text;
                    self.input.cursor_position = self.input.buffer.len();
                }
                self.history_search_open = false;
                self.history_search_index = None;
            }
            KeyCode::Up => {
                // Move selection up (toward index 0 = most recent).
                let matches = self.history.search(&self.history_search_query, 8);
                let count = matches.len();
                if count > 0 {
                    self.history_search_index = Some(match self.history_search_index {
                        None | Some(0) => 0,
                        Some(i) => i - 1,
                    });
                }
            }
            KeyCode::Down => {
                // Move selection down (toward older entries).
                let matches = self.history.search(&self.history_search_query, 8);
                let count = matches.len();
                if count > 0 {
                    self.history_search_index = Some(match self.history_search_index {
                        None => 0,
                        Some(i) => (i + 1).min(count - 1),
                    });
                }
            }
            KeyCode::Backspace => {
                self.history_search_query.pop();
                // Reset selection when query changes.
                self.history_search_index = None;
            }
            KeyCode::Char(c) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.history_search_query.push(c);
                // Reset selection when query changes.
                self.history_search_index = None;
            }
            _ => {}
        }
    }

    // -----------------------------------------------------------------------
    // Command palette key handling
    // -----------------------------------------------------------------------
    fn handle_key_palette(&mut self, key: KeyEvent) {
        use crate::render::command_palette::PALETTE_COMMANDS;
        match key.code {
            KeyCode::Esc => {
                self.palette_open = false;
            }
            KeyCode::Char('k') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.palette_open = false;
            }
            KeyCode::Up | KeyCode::Char('k') => {
                self.palette_selection = self.palette_selection.saturating_sub(1);
            }
            KeyCode::Down | KeyCode::Char('j') => {
                let count = PALETTE_COMMANDS
                    .iter()
                    .filter(|e| palette_matches(e.name, e.description, &self.palette_query))
                    .count();
                if count > 0 {
                    self.palette_selection = (self.palette_selection + 1).min(count - 1);
                }
            }
            KeyCode::Enter => {
                let selected = PALETTE_COMMANDS
                    .iter()
                    .filter(|e| palette_matches(e.name, e.description, &self.palette_query))
                    .nth(self.palette_selection)
                    .map(|e| e.name);
                if let Some(name) = selected {
                    self.input.buffer = format!("/{name} ");
                    self.input.cursor_position = self.input.buffer.len();
                }
                self.palette_open = false;
            }
            KeyCode::Char(c) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.palette_query.push(c);
                self.palette_selection = 0;
            }
            KeyCode::Backspace => {
                self.palette_query.pop();
            }
            _ => {}
        }
    }

    // -----------------------------------------------------------------------
    // Scrollback search key handling
    // -----------------------------------------------------------------------
    fn handle_key_search(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Esc => {
                self.search_open = false;
                self.search_matches.clear();
                self.search_cursor = 0;
            }
            KeyCode::Enter | KeyCode::Char('n') => {
                if !self.search_matches.is_empty() {
                    self.search_cursor = (self.search_cursor + 1) % self.search_matches.len();
                }
            }
            KeyCode::Char('N') => {
                if !self.search_matches.is_empty() {
                    self.search_cursor = self.search_cursor
                        .checked_sub(1)
                        .unwrap_or(self.search_matches.len() - 1);
                }
            }
            KeyCode::Char(c) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.search_query.push(c);
                self.recompute_search_matches();
            }
            KeyCode::Backspace => {
                self.search_query.pop();
                self.recompute_search_matches();
            }
            _ => {}
        }
    }

    fn recompute_search_matches(&mut self) {
        self.search_matches.clear();
        self.search_cursor = 0;
        if self.search_query.is_empty() {
            return;
        }
        let query_lower = self.search_query.to_lowercase();
        for (i, msg) in self.messages.iter().enumerate() {
            if msg.content.to_lowercase().contains(&query_lower) {
                self.search_matches.push(i);
            }
        }
    }

    pub fn palette_query(&self) -> &str { &self.palette_query }
    pub fn palette_selection(&self) -> usize { self.palette_selection }
    pub fn search_query(&self) -> &str { &self.search_query }
    pub fn search_matches(&self) -> &[usize] { &self.search_matches }
    pub fn search_cursor(&self) -> usize { self.search_cursor }
    pub fn recompute_search_matches_pub(&mut self) { self.recompute_search_matches(); }
    pub fn handle_key_search_pub(&mut self, key: KeyEvent) { self.handle_key_search(key); }
    pub fn handle_key_palette_pub(&mut self, key: KeyEvent) { self.handle_key_palette(key); }

    // -----------------------------------------------------------------------
    // Session Anatomy key handling
    // -----------------------------------------------------------------------
    pub fn handle_key_anatomy(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Esc => { self.anatomy_open = false; }
            KeyCode::Char('a') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.anatomy_open = false;
            }
            KeyCode::Tab => {
                self.anatomy_tab = (self.anatomy_tab + 1) % 2;
            }
            KeyCode::Char('j') | KeyCode::Down => {
                self.anatomy_scroll = self.anatomy_scroll.saturating_add(1);
            }
            KeyCode::Char('k') | KeyCode::Up => {
                self.anatomy_scroll = self.anatomy_scroll.saturating_sub(1);
            }
            KeyCode::Char('g') => { self.anatomy_scroll = 0; }
            KeyCode::Char('G') => { self.anatomy_scroll = usize::MAX / 2; }
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

        // During an active turn, queue regular messages as a follow-up.
        // Commands (/) and shell escapes (!) pass through immediately.
        if matches!(self.state, AppState::WaitingForResponse) {
            let is_cmd = self.input.buffer.starts_with('/');
            let is_shell = self.input.buffer.starts_with('!');
            if !is_cmd && !is_shell {
                if self.queued_message.is_some() {
                    // Already one message queued — only one slot is supported.
                    self.messages.push(DisplayMessage::new(
                        DisplayRole::System,
                        "⚠ only one message can be queued at a time — previous follow-up kept".to_string(),
                    ));
                    self.input.take_input();
                    return;
                }
                self.queued_message = Some(self.input.take_input());
                self.messages.push(DisplayMessage::new(
                    DisplayRole::System,
                    "↪ queued — will send when this turn completes".to_string(),
                ));
                return;
            }
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
        self.history.push(&text);
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

    /// Take the pending model change request set by `/model` or
    /// `/configure model`.  Returns `None` when no change is pending.
    pub fn take_pending_model_change(&mut self) -> Option<String> {
        self.pending_model_change.take()
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

    /// Insert all characters from a bracketed-paste event, including newlines,
    /// without triggering submission. Called by the event loop on `Event::Paste`.
    pub fn handle_paste(&mut self, text: &str) {
        for c in text.chars() {
            self.input.insert_char(c);
        }
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
                if id.is_empty() {
                    self.messages.push(DisplayMessage::new(
                        DisplayRole::Error,
                        "Usage: /model <provider:model-id>".to_string(),
                    ));
                } else {
                    self.status.model = id.clone();
                    self.pending_model_change = Some(id);
                    self.messages.push(DisplayMessage::new(
                        DisplayRole::System,
                        format!(
                            "Model changed to: {}  (takes effect on next turn)",
                            self.status.model
                        ),
                    ));
                }
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
/configure                       — show current config summary\n\
/configure model <provider:id>   — switch model (takes effect on next turn)\n\
/configure theme <name>          — switch theme (crimson, neon, chalk, minimal)\n\
/configure vim on|off            — toggle vim keybindings\n\
/configure budget <usd>          — set session budget; 'off' to clear\n\
/configure provider <name>       — note: requires session restart\n\
/configure help                  — list configure subcommands\n\
/model <id>        — switch model (shorthand)\n\
/cost              — show session cost\n\
/tdd [command]     — start TDD file watcher (:tdd off to stop)\n\
/review            — review session changes (hunk accept/reject)\n\
/clarify [prompt]  — ask clarifying questions before coding\n\
/set vim | novim   — toggle vim keybindings\n\
/theme <name>      — switch theme (shorthand)\n\
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
            // Show current configuration summary.
            let budget_str = match self.status.budget_usd {
                Some(b) => format!("${b:.2}"),
                None => "not set".to_string(),
            };
            let vim_str = if self.vim_enabled { "on" } else { "off" };
            let provider_str = match self.status.model.split_once(':') {
                Some((provider, _)) => provider.to_string(),
                None if self.status.model.is_empty() => "(unknown)".to_string(),
                None => self.status.model.clone(),
            };
            let info = format!(
                "Current Configuration:\n\
                 \n\
                 Model:    {model}\n\
                 Provider: {provider}\n\
                 Theme:    (use /configure theme to change)\n\
                 Vim mode: {vim}\n\
                 Budget:   {budget}\n\
                 Mode:     {mode}\n\
                 Session:  {session}\n\
                 Cost:     {cost}\n\
                 Context:  {context}\n\
                 Phase:    {phase}\n\
                 \n\
                 Use /configure help for subcommand list.",
                model = if self.status.model.is_empty() { "none" } else { &self.status.model },
                provider = provider_str,
                vim = vim_str,
                budget = budget_str,
                mode = &self.status.mode,
                session = self.status.session_name.as_deref().unwrap_or("(unnamed)"),
                cost = self.status.format_cost(),
                context = self.status.format_context(),
                phase = self.status.phase_long(),
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
            "help" => {
                let help = "\
/configure                       — show current config summary\n\
/configure model <provider:id>   — switch model (takes effect on next turn)\n\
/configure theme <name>          — switch theme (crimson, neon, chalk, minimal)\n\
/configure vim on|off            — toggle vim keybindings\n\
/configure budget <usd>          — set session budget (e.g. 5.00)\n\
/configure budget off            — clear session budget\n\
/configure provider <name>       — note: requires session restart\n\
/configure help                  — this message";
                self.messages.push(DisplayMessage::new(DisplayRole::System, help));
            }
            "model" => {
                // Delegate to the existing Command::Model handler (reuses its
                // validation and pending_model_change logic).
                self.handle_command(Command::Model(value.to_string()));
            }
            "theme" => {
                // Delegate to the existing theme-switch handler.
                self.handle_command(Command::ThemeSwitch(value.to_string()));
            }
            "vim" => {
                let enable = matches!(value, "on" | "true" | "1" | "yes" | "");
                self.handle_command(Command::SetVim(enable));
            }
            "budget" => {
                if value == "off" || value == "none" || value == "clear" {
                    self.status.budget_usd = None;
                    self.messages.push(DisplayMessage::new(
                        DisplayRole::System,
                        "Session budget cleared".to_string(),
                    ));
                } else if value.is_empty() {
                    self.messages.push(DisplayMessage::new(
                        DisplayRole::Error,
                        "Usage: /configure budget <usd>  or  /configure budget off".to_string(),
                    ));
                } else {
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
                                format!(
                                    "Invalid budget: '{value}' — use a number like 1.00, or 'off'"
                                ),
                            ));
                        }
                    }
                }
            }
            "provider" => {
                if value.is_empty() {
                    self.messages.push(DisplayMessage::new(
                        DisplayRole::Error,
                        "Usage: /configure provider <name>".to_string(),
                    ));
                } else {
                    self.messages.push(DisplayMessage::new(
                        DisplayRole::System,
                        format!(
                            "Provider change to '{value}' noted — restart the session to apply. \
                             (Mid-session provider swap is not supported.)"
                        ),
                    ));
                }
            }
            _ => {
                self.messages.push(DisplayMessage::new(
                    DisplayRole::Error,
                    format!(
                        "Unknown configure subcommand: {subcmd}. Try /configure help."
                    ),
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

            AgentEvent::ToolCallStart { id, name, args } => {
                self.status.update_phase_from_tool(&name);
                self.status.active_tool = Some(name.clone());
                self.status.provider_status = ProviderStatus::Connected;
                self.messages.push(DisplayMessage::new(
                    DisplayRole::ToolCall { name: name.clone() },
                    String::new(),
                ));

                // Ensure we have an active ledger for this turn.
                if self.current_turn.is_none() {
                    let turn_index = self.completed_turns.len();
                    self.current_turn = Some(TurnLedger::new(turn_index));
                }

                // Record phase and file touch in the ledger.
                if let Some(ledger) = &mut self.current_turn {
                    let phase = Phase::from_label(&self.status.phase);
                    // Only record a transition if this is the first tool call.
                    if ledger.phase_transitions.is_empty() {
                        ledger.record_phase_transition(phase);
                    }
                    // Record file touch if the tool touches a file.
                    if let Some(op) = tool_op(&name) {
                        if let Some(path) = extract_path(&args) {
                            ledger.record_file_touch(path, op, None);
                        }
                    }
                }

                // Store pending tool start for duration tracking on ToolCallEnd.
                self.pending_tool_start = Some((id, name, args));
            }

            AgentEvent::ToolCallEnd { id, result, is_error, duration_ms, cache_hit } => {
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

                // Record tool call in the ledger.
                if let Some(ledger) = &mut self.current_turn {
                    let error_str = if is_error {
                        Some(result.to_string())
                    } else {
                        None
                    };
                    // Find tool name from pending start.
                    let tool_name = self
                        .pending_tool_start
                        .as_ref()
                        .filter(|(start_id, _, _)| start_id == &id)
                        .map(|(_, n, _)| n.clone())
                        .unwrap_or_default();
                    ledger.record_tool_call(tool_name, duration_ms, cache_hit, error_str);
                }
                self.pending_tool_start = None;

                self.messages
                    .push(DisplayMessage::new(DisplayRole::ToolResult, content));
            }

            AgentEvent::Done { usage } => {
                // Seal the current turn ledger.
                if let Some(ledger) = self.current_turn.take() {
                    self.completed_turns.push(ledger);
                }

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

            AgentEvent::ModelInfo { context_window } => {
                self.status.context_limit = context_window;
            }

            AgentEvent::ContextInjected { strategy, tokens_used, bm25_hits, confidence: _ } => {
                if self.current_turn.is_none() {
                    let turn_index = self.completed_turns.len();
                    self.current_turn = Some(TurnLedger::new(turn_index));
                }
                if let Some(ledger) = &mut self.current_turn {
                    if bm25_hits.is_empty() {
                        ledger.record_skeleton_entry("(skeleton)", &strategy, None, tokens_used);
                    } else {
                        let per_file = tokens_used / (bm25_hits.len() as u32).max(1);
                        for (path, score) in &bm25_hits {
                            ledger.record_skeleton_entry(path, &strategy, Some(*score), per_file);
                        }
                    }
                }
            }

            AgentEvent::MemoryRecalled { entries } => {
                if self.current_turn.is_none() {
                    let turn_index = self.completed_turns.len();
                    self.current_turn = Some(TurnLedger::new(turn_index));
                }
                if let Some(ledger) = &mut self.current_turn {
                    for (key, snippet) in entries {
                        ledger.record_memory_recall(key, snippet);
                    }
                }
            }

            // Forward-compat: unknown AgentEvent variants are silently ignored by the TUI.
            _ => {}
        }
    }
}

impl Default for App {
    fn default() -> Self {
        Self::new()
    }
}

/// Returns `true` when `name` or `description` contains `query` (case-insensitive).
pub(crate) fn palette_matches(name: &str, description: &str, query: &str) -> bool {
    if query.is_empty() {
        return true;
    }
    let q = query.to_lowercase();
    name.to_lowercase().contains(&q) || description.to_lowercase().contains(&q)
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

#[cfg(test)]
mod tests {
    use super::*;
    use hone_proto::AgentEvent;

    /// Sending `AgentEvent::ModelInfo` must update `status.context_limit`.
    #[test]
    fn model_info_event_updates_context_limit() {
        let mut app = App::new();
        // Default should be the conservative fallback (128 000).
        assert_eq!(app.status.context_limit, 128_000);

        app.handle_agent_event(AgentEvent::ModelInfo {
            context_window: 200_000,
        });

        assert_eq!(app.status.context_limit, 200_000,
            "context_limit should be updated by ModelInfo event");
    }

    /// A second ModelInfo event with a different value must also update.
    #[test]
    fn model_info_event_is_idempotent_with_same_value() {
        let mut app = App::new();
        app.handle_agent_event(AgentEvent::ModelInfo { context_window: 200_000 });
        app.handle_agent_event(AgentEvent::ModelInfo { context_window: 200_000 });
        assert_eq!(app.status.context_limit, 200_000);
    }

    // -----------------------------------------------------------------------
    // /configure command tests
    // -----------------------------------------------------------------------

    fn last_system_msg(app: &App) -> &str {
        app.messages
            .iter()
            .rev()
            .find(|m| matches!(m.role, DisplayRole::System))
            .map(|m| m.content.as_str())
            .unwrap_or("")
    }

    fn last_error_msg(app: &App) -> &str {
        app.messages
            .iter()
            .rev()
            .find(|m| matches!(m.role, DisplayRole::Error))
            .map(|m| m.content.as_str())
            .unwrap_or("")
    }

    /// `/configure` with no args produces a status summary message that
    /// includes key fields: model, provider, vim, budget.
    #[test]
    fn configure_no_args_shows_summary() {
        let mut app = App::new();
        app.status.model = "anthropic:claude-3-5-sonnet".to_string();
        app.vim_enabled = true;
        app.status.budget_usd = Some(3.50);

        app.handle_command(Command::Configure(String::new()));

        let msg = last_system_msg(&app);
        assert!(msg.contains("Current Configuration"), "should contain header");
        assert!(msg.contains("anthropic:claude-3-5-sonnet"), "should contain model");
        assert!(msg.contains("anthropic"), "should contain provider");
        assert!(msg.contains("on"), "should show vim on");
        assert!(msg.contains("$3.50"), "should show budget");
    }

    /// `/configure model <id>` triggers the model-switch path (sets
    /// `pending_model_change` and appends a system message).
    #[test]
    fn configure_model_delegates_to_model_handler() {
        let mut app = App::new();
        app.handle_command(Command::Configure("model anthropic:claude-opus-4".to_string()));

        assert_eq!(
            app.pending_model_change.as_deref(),
            Some("anthropic:claude-opus-4"),
            "pending_model_change must be set"
        );
        assert_eq!(app.status.model, "anthropic:claude-opus-4");
        let msg = last_system_msg(&app);
        assert!(msg.contains("Model changed to"), "should produce model-changed message");
    }

    /// `/configure model` with no id produces the usage error from Command::Model.
    #[test]
    fn configure_model_empty_id_is_error() {
        let mut app = App::new();
        app.handle_command(Command::Configure("model".to_string()));

        assert!(
            app.pending_model_change.is_none(),
            "should not set pending_model_change"
        );
        let msg = last_error_msg(&app);
        assert!(msg.contains("Usage"), "should print usage hint");
    }

    /// `/configure budget 5.0` sets `status.budget_usd = Some(5.0)`.
    #[test]
    fn configure_budget_sets_value() {
        let mut app = App::new();
        app.handle_command(Command::Configure("budget 5.0".to_string()));

        assert!(
            (app.status.budget_usd.unwrap() - 5.0).abs() < 1e-9,
            "budget_usd should be 5.0"
        );
        let msg = last_system_msg(&app);
        assert!(msg.contains("$5.00"), "should confirm budget in message");
    }

    /// `/configure budget off` clears `status.budget_usd`.
    #[test]
    fn configure_budget_off_clears_value() {
        let mut app = App::new();
        app.status.budget_usd = Some(10.0);
        app.handle_command(Command::Configure("budget off".to_string()));

        assert!(app.status.budget_usd.is_none(), "budget_usd should be cleared");
        let msg = last_system_msg(&app);
        assert!(msg.contains("cleared"), "should confirm budget cleared");
    }

    /// `/configure budget` with no value produces a usage error.
    #[test]
    fn configure_budget_no_value_is_error() {
        let mut app = App::new();
        app.handle_command(Command::Configure("budget".to_string()));

        let msg = last_error_msg(&app);
        assert!(msg.contains("Usage"), "should print usage hint");
    }

    /// `/configure budget notanumber` produces an error message.
    #[test]
    fn configure_budget_invalid_value_is_error() {
        let mut app = App::new();
        app.handle_command(Command::Configure("budget notanumber".to_string()));

        assert!(app.status.budget_usd.is_none());
        let msg = last_error_msg(&app);
        assert!(msg.contains("notanumber"), "should include offending value");
    }

    /// `/configure provider <name>` emits a "session restart required" hint.
    #[test]
    fn configure_provider_emits_restart_hint() {
        let mut app = App::new();
        app.handle_command(Command::Configure("provider deepseek".to_string()));

        let msg = last_system_msg(&app);
        assert!(msg.contains("deepseek"), "should mention provider name");
        assert!(
            msg.to_lowercase().contains("restart"),
            "should mention restart"
        );
    }

    /// `/configure provider` with no name produces an error.
    #[test]
    fn configure_provider_no_name_is_error() {
        let mut app = App::new();
        app.handle_command(Command::Configure("provider".to_string()));

        let msg = last_error_msg(&app);
        assert!(msg.contains("Usage"), "should print usage hint");
    }

    /// `/configure help` produces a list of subcommands.
    #[test]
    fn configure_help_produces_subcommand_list() {
        let mut app = App::new();
        app.handle_command(Command::Configure("help".to_string()));

        let msg = last_system_msg(&app);
        assert!(msg.contains("model"), "help should list 'model'");
        assert!(msg.contains("budget"), "help should list 'budget'");
        assert!(msg.contains("theme"), "help should list 'theme'");
        assert!(msg.contains("vim"), "help should list 'vim'");
        assert!(msg.contains("provider"), "help should list 'provider'");
    }

    /// `/configure <badsubcmd>` produces an error mentioning the unknown subcommand.
    #[test]
    fn configure_unknown_subcommand_produces_error() {
        let mut app = App::new();
        app.handle_command(Command::Configure("badsubcmd".to_string()));

        let msg = last_error_msg(&app);
        assert!(msg.contains("badsubcmd"), "error should name the bad subcommand");
        assert!(
            msg.to_lowercase().contains("try /configure help"),
            "error should suggest /configure help"
        );
    }

    /// `/configure vim on` enables vim mode.
    #[test]
    fn configure_vim_on_enables_vim() {
        let mut app = App::new();
        assert!(!app.vim_enabled);
        app.handle_command(Command::Configure("vim on".to_string()));
        assert!(app.vim_enabled);
    }

    /// `/configure vim off` disables vim mode.
    #[test]
    fn configure_vim_off_disables_vim() {
        let mut app = App::new();
        app.vim_enabled = true;
        app.handle_command(Command::Configure("vim off".to_string()));
        assert!(!app.vim_enabled);
    }

    /// `/configure theme crimson` switches the theme (delegates to ThemeSwitch).
    #[test]
    fn configure_theme_delegates_to_theme_switch() {
        let mut app = App::new();
        app.handle_command(Command::Configure("theme crimson".to_string()));

        let msg = last_system_msg(&app);
        assert!(msg.contains("crimson"), "should confirm theme switch");
    }

    // -----------------------------------------------------------------------
    // Input ergonomics tests
    // -----------------------------------------------------------------------

    fn make_key(code: KeyCode, modifiers: KeyModifiers) -> KeyEvent {
        KeyEvent::new(code, modifiers)
    }

    #[test]
    fn shift_enter_inserts_newline_does_not_submit() {
        let mut app = App::new();
        app.show_splash = false;
        app.type_text("hello");
        app.handle_key(make_key(KeyCode::Enter, KeyModifiers::SHIFT));
        assert!(app.input.buffer.contains('\n'), "buffer must contain newline after Shift+Enter");
        assert_eq!(app.messages.len(), 0, "Shift+Enter must not submit");
        assert!(matches!(app.state, AppState::Idle));
    }

    #[test]
    fn enter_submits_buffer_to_messages() {
        let mut app = App::new();
        app.show_splash = false;
        app.type_text("hello world");
        app.handle_key(make_key(KeyCode::Enter, KeyModifiers::NONE));
        assert!(app.input.buffer.is_empty(), "buffer must be cleared after submit");
        assert_eq!(app.messages.len(), 1, "one user message must be created");
        assert_eq!(app.messages[0].content, "hello world");
    }

    #[test]
    fn handle_paste_inserts_with_newlines_without_submit() {
        let mut app = App::new();
        app.show_splash = false;
        app.handle_paste("line one\nline two\n");
        assert_eq!(app.input.buffer, "line one\nline two\n");
        assert_eq!(app.messages.len(), 0, "paste must not submit");
    }

    #[test]
    fn submit_empty_input_is_noop() {
        let mut app = App::new();
        app.show_splash = false;
        app.handle_key(make_key(KeyCode::Enter, KeyModifiers::NONE));
        assert_eq!(app.messages.len(), 0, "empty submit must produce no messages");
    }

    #[test]
    fn escape_clears_input_buffer_when_idle() {
        let mut app = App::new();
        app.show_splash = false;
        app.type_text("some text");
        app.handle_key(make_key(KeyCode::Esc, KeyModifiers::NONE));
        assert!(app.input.buffer.is_empty(), "Esc in Idle must clear the input buffer");
        assert_eq!(app.input.cursor_position, 0);
    }

    #[test]
    fn second_queue_attempt_warns_and_keeps_first() {
        let mut app = App::new();
        app.show_splash = false;
        app.state = AppState::WaitingForResponse;
        app.type_text("first message");
        app.handle_key(make_key(KeyCode::Enter, KeyModifiers::NONE));
        let first = app.queued_message.clone();
        app.type_text("second message");
        app.handle_key(make_key(KeyCode::Enter, KeyModifiers::NONE));
        assert_eq!(app.queued_message, first, "first queued message must be preserved");
        let warn = app.messages.iter().any(|m| m.content.contains("only one message"));
        assert!(warn, "a warning must be shown when a second queue is attempted");
    }

    #[test]
    fn cancel_turn_clears_stale_ledger_and_tool_start() {
        let mut app = App::new();
        app.show_splash = false;
        app.current_turn = Some(TurnLedger::new(0));
        app.pending_tool_start = Some(("id".into(), "read_file".into(), serde_json::Value::Null));
        app.cancel_turn();
        assert!(app.current_turn.is_none(), "cancel_turn must clear current_turn");
        assert!(app.pending_tool_start.is_none(), "cancel_turn must clear pending_tool_start");
    }

    // -----------------------------------------------------------------------
    // Feature A: tool-card collapse toggle
    // -----------------------------------------------------------------------

    /// a) New DisplayMessage for ToolResult role has is_collapsed=true by default.
    #[test]
    fn tool_result_message_is_collapsed_by_default() {
        let msg = DisplayMessage::new(DisplayRole::ToolResult, "output data");
        assert!(msg.is_collapsed, "ToolResult messages must start collapsed");
    }

    /// a) Non-ToolResult messages are NOT collapsed by default.
    #[test]
    fn user_message_is_not_collapsed_by_default() {
        let msg = DisplayMessage::new(DisplayRole::User, "hello");
        assert!(!msg.is_collapsed, "User messages must not start collapsed");
    }

    #[test]
    fn assistant_message_is_not_collapsed_by_default() {
        let msg = DisplayMessage::new(DisplayRole::Assistant, "hello");
        assert!(!msg.is_collapsed, "Assistant messages must not start collapsed");
    }

    /// b) App has a focus_msg_index: Option<usize> field, defaults to None.
    #[test]
    fn app_focus_msg_index_defaults_to_none() {
        let app = App::new();
        assert!(app.focus_msg_index.is_none(), "focus_msg_index must default to None");
    }

    /// c) When FocusTarget::Conversation and j pressed, focus_msg_index increments.
    #[test]
    fn j_key_advances_focus_msg_index_in_conversation_pane() {
        let mut app = App::new();
        app.show_splash = false;
        app.focus = FocusTarget::Conversation;
        app.messages.push(DisplayMessage::new(DisplayRole::System, "msg0"));
        app.messages.push(DisplayMessage::new(DisplayRole::System, "msg1"));
        app.messages.push(DisplayMessage::new(DisplayRole::System, "msg2"));

        // First j: None → 0
        app.handle_key(make_key(KeyCode::Char('j'), KeyModifiers::NONE));
        assert_eq!(app.focus_msg_index, Some(0));

        // Second j: 0 → 1
        app.handle_key(make_key(KeyCode::Char('j'), KeyModifiers::NONE));
        assert_eq!(app.focus_msg_index, Some(1));
    }

    /// c) Down arrow also advances focus_msg_index when Conversation focused.
    #[test]
    fn down_key_advances_focus_msg_index_in_conversation_pane() {
        let mut app = App::new();
        app.show_splash = false;
        app.focus = FocusTarget::Conversation;
        app.messages.push(DisplayMessage::new(DisplayRole::System, "msg0"));
        app.messages.push(DisplayMessage::new(DisplayRole::System, "msg1"));

        app.handle_key(make_key(KeyCode::Down, KeyModifiers::NONE));
        assert_eq!(app.focus_msg_index, Some(0));

        app.handle_key(make_key(KeyCode::Down, KeyModifiers::NONE));
        assert_eq!(app.focus_msg_index, Some(1));
    }

    /// c) j floors at last message index (no wrap-around past the end).
    #[test]
    fn j_key_floors_at_last_message() {
        let mut app = App::new();
        app.show_splash = false;
        app.focus = FocusTarget::Conversation;
        app.messages.push(DisplayMessage::new(DisplayRole::System, "only"));
        app.focus_msg_index = Some(0);

        app.handle_key(make_key(KeyCode::Char('j'), KeyModifiers::NONE));
        assert_eq!(app.focus_msg_index, Some(0), "j must not advance past the last message");
    }

    /// d) When FocusTarget::Conversation and k pressed, focus_msg_index decrements (floor 0).
    #[test]
    fn k_key_decrements_focus_msg_index_in_conversation_pane() {
        let mut app = App::new();
        app.show_splash = false;
        app.focus = FocusTarget::Conversation;
        app.messages.push(DisplayMessage::new(DisplayRole::System, "msg0"));
        app.messages.push(DisplayMessage::new(DisplayRole::System, "msg1"));
        app.focus_msg_index = Some(1);

        app.handle_key(make_key(KeyCode::Char('k'), KeyModifiers::NONE));
        assert_eq!(app.focus_msg_index, Some(0));
    }

    /// d) k is floored at 0 (does not underflow).
    #[test]
    fn k_key_floors_at_zero() {
        let mut app = App::new();
        app.show_splash = false;
        app.focus = FocusTarget::Conversation;
        app.messages.push(DisplayMessage::new(DisplayRole::System, "only"));
        app.focus_msg_index = Some(0);

        app.handle_key(make_key(KeyCode::Char('k'), KeyModifiers::NONE));
        assert_eq!(app.focus_msg_index, Some(0), "k must floor at 0");
    }

    /// e) Enter on a focused collapsed ToolResult message sets is_collapsed=false.
    #[test]
    fn enter_expands_focused_collapsed_tool_result() {
        let mut app = App::new();
        app.show_splash = false;
        app.focus = FocusTarget::Conversation;
        app.messages.push(DisplayMessage::new(DisplayRole::ToolResult, "result data\nline two\nline three"));
        app.focus_msg_index = Some(0);
        assert!(app.messages[0].is_collapsed, "message must start collapsed");

        app.handle_key(make_key(KeyCode::Enter, KeyModifiers::NONE));
        assert!(!app.messages[0].is_collapsed, "Enter must expand a collapsed ToolResult");
    }

    /// f) Enter on a focused expanded ToolResult message sets is_collapsed=true (toggle).
    #[test]
    fn enter_collapses_focused_expanded_tool_result() {
        let mut app = App::new();
        app.show_splash = false;
        app.focus = FocusTarget::Conversation;
        let mut msg = DisplayMessage::new(DisplayRole::ToolResult, "result data");
        msg.is_collapsed = false;
        app.messages.push(msg);
        app.focus_msg_index = Some(0);

        app.handle_key(make_key(KeyCode::Enter, KeyModifiers::NONE));
        assert!(app.messages[0].is_collapsed, "Enter must collapse an expanded ToolResult");
    }

    /// Space also toggles collapse on a focused ToolResult.
    #[test]
    fn space_toggles_focused_tool_result_collapse() {
        let mut app = App::new();
        app.show_splash = false;
        app.focus = FocusTarget::Conversation;
        app.messages.push(DisplayMessage::new(DisplayRole::ToolResult, "result"));
        app.focus_msg_index = Some(0);
        assert!(app.messages[0].is_collapsed);

        app.handle_key(make_key(KeyCode::Char(' '), KeyModifiers::NONE));
        assert!(!app.messages[0].is_collapsed, "Space must expand a collapsed ToolResult");
    }

    /// g) Non-ToolResult messages (User, Assistant) are not toggled by Enter.
    #[test]
    #[allow(clippy::bool_assert_comparison)]
    fn enter_does_not_toggle_non_tool_result_message() {
        let mut app = App::new();
        app.show_splash = false;
        app.focus = FocusTarget::Conversation;
        app.messages.push(DisplayMessage::new(DisplayRole::User, "hello"));
        app.focus_msg_index = Some(0);
        assert!(!app.messages[0].is_collapsed);

        app.handle_key(make_key(KeyCode::Enter, KeyModifiers::NONE));
        // Enter on a User message should NOT change is_collapsed (stays false).
        assert!(!app.messages[0].is_collapsed,
            "Enter on User message must not change is_collapsed");
    }
}
