use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use hone_proto::AgentEvent;

use crate::{
    animation::AnimationState,
    commands::Command,
    input::InputState,
    layout::{FocusTarget, LayoutMode},
    message::{DisplayMessage, DisplayRole},
    status::StatusInfo,
    theme::Theme,
};

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
            show_splash: false,
        }
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

    /// Advance the animation by one tick (call at 100ms intervals).
    pub fn tick(&mut self) {
        self.animation.tick();
    }

    // -----------------------------------------------------------------------
    // Keyboard handling
    // -----------------------------------------------------------------------

    /// Process a terminal key event and update state accordingly.
    pub fn handle_key(&mut self, key: KeyEvent) {
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

        // Check whether this is a slash-command.
        if let Some(cmd) = Command::parse(&text) {
            self.handle_command(cmd);
            return;
        }

        // Plain message — add it to the conversation and start waiting.
        self.messages.push(DisplayMessage::new(DisplayRole::User, text));
        self.state = AppState::WaitingForResponse;
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
/model <id>  — switch model\n\
/cost        — show session cost\n\
/undo        — undo last action\n\
/redo        — redo\n\
/quit  /q    — quit\n\
/help        — this message";
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
            Command::Unknown(name) => {
                let msg = format!("Unknown command: /{name}  (type /help for help)");
                self.messages
                    .push(DisplayMessage::new(DisplayRole::Error, msg));
            }
        }
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
                self.messages.push(DisplayMessage::new(
                    DisplayRole::ToolCall { name },
                    String::new(),
                ));
            }

            AgentEvent::ToolCallEnd { result, is_error, .. } => {
                let content = if is_error {
                    format!("error: {result}")
                } else {
                    result.to_string()
                };
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
                self.state = AppState::Idle;
            }

            AgentEvent::Error { message } => {
                self.messages
                    .push(DisplayMessage::new(DisplayRole::Error, message.clone()));
                self.state = AppState::Error(message);
            }

            AgentEvent::ApprovalRequired { description, .. } => {
                self.messages.push(DisplayMessage::new(
                    DisplayRole::System,
                    format!("Approval required: {description}"),
                ));
            }
        }
    }
}

impl Default for App {
    fn default() -> Self {
        Self::new()
    }
}
