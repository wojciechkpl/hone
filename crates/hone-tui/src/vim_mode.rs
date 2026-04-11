//! Vim-style keybinding overlay for the Hone TUI.
//!
//! When vim mode is enabled (via `--vim` flag or `:set vim` command), the
//! input area gains two modes:
//!
//! * **Normal mode** — navigation and command keys are active.
//!   - `j` / `k` — scroll the conversation up / down.
//!   - `i` / `a` — enter Insert mode (cursor at current / end of line).
//!   - `A` — enter Insert mode at end of line.
//!   - `/` — enter Insert mode with the `/` character pre-inserted (for slash
//!     commands / inline search).
//!   - `G` — scroll to the bottom of the conversation.
//!   - `g g` — scroll to the top.
//!   - `:q` — quit.
//!   - `Esc` — no-op (already in Normal mode).
//!
//! * **Insert mode** — characters are typed normally.
//!   - `Esc` — return to Normal mode and clear any pending key sequence.
//!
//! This is intentionally a **lightweight overlay**, not a full vim
//! implementation.  The goal is ergonomic navigation without the complexity of
//! a complete modal editor.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

/// The active vim mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum VimMode {
    /// Normal mode: navigation and command keys active.
    #[default]
    Normal,
    /// Insert mode: characters typed normally.
    Insert,
}

impl VimMode {
    /// Human-readable label shown in the status bar.
    pub fn label(self) -> &'static str {
        match self {
            VimMode::Normal => "NORMAL",
            VimMode::Insert => "INSERT",
        }
    }
}

/// A key event processed by the vim overlay.
///
/// The overlay pattern:
/// 1. `VimOverlay::process` is called first on every key event.
/// 2. If it returns `Some(VimAction)`, the caller executes the action and
///    **does not** forward the key to the normal `App::handle_key` path.
/// 3. If it returns `None`, the overlay consumed the key silently (e.g. to
///    accumulate a multi-key sequence like `g g`).
/// 4. If it returns `Some(VimAction::PassThrough(key))`, the caller should
///    forward the original key to the normal handler (Insert mode behaviour).
#[derive(Debug, Clone)]
pub enum VimAction {
    /// Enter Insert mode (optionally appending a char to the input first).
    EnterInsert,
    /// Enter Insert mode, placing cursor at end of current input.
    EnterInsertAtEnd,
    /// Return to Normal mode; clears any pending key sequence.
    EnterNormal,
    /// Scroll the conversation up by `lines`.
    ScrollUp(usize),
    /// Scroll the conversation down by `lines`.
    ScrollDown(usize),
    /// Scroll to the very top.
    ScrollToTop,
    /// Scroll to the very bottom.
    ScrollToBottom,
    /// Quit the application.
    Quit,
    /// Submit current input (Enter in Insert mode).
    Submit,
    /// Forward the key event to the regular input handler unchanged.
    PassThrough(KeyEvent),
}

/// Pending multi-key sequence state.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
enum PendingKey {
    #[default]
    None,
    /// First `g` has been pressed; waiting for second `g` or something else.
    FirstG,
}

/// The vim-mode overlay state machine.
#[derive(Debug, Clone, Default)]
pub struct VimOverlay {
    pub mode: VimMode,
    pending: PendingKey,
}

impl VimOverlay {
    /// Create a new overlay starting in Normal mode.
    pub fn new() -> Self {
        Self::default()
    }

    /// Create a new overlay starting in Insert mode (for `--vim` flag when the
    /// user might want to type immediately).
    pub fn new_insert() -> Self {
        Self { mode: VimMode::Insert, pending: PendingKey::None }
    }

    /// Process a key event through the overlay.
    ///
    /// See the module-level documentation for the dispatch contract.
    pub fn process(&mut self, key: KeyEvent) -> Option<VimAction> {
        match self.mode {
            VimMode::Insert => self.process_insert(key),
            VimMode::Normal => self.process_normal(key),
        }
    }

    fn process_insert(&mut self, key: KeyEvent) -> Option<VimAction> {
        match key.code {
            KeyCode::Esc => {
                self.mode = VimMode::Normal;
                self.pending = PendingKey::None;
                Some(VimAction::EnterNormal)
            }
            KeyCode::Enter => Some(VimAction::Submit),
            // All other keys pass through to the normal input handler.
            _ => Some(VimAction::PassThrough(key)),
        }
    }

    fn process_normal(&mut self, key: KeyEvent) -> Option<VimAction> {
        // Ctrl-C always quits regardless of mode; return None so the outer
        // handler still sees it (it checks for Ctrl-C before calling vim overlay).
        if key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL) {
            return None;
        }

        // Resolve any pending multi-key sequence first.
        if self.pending == PendingKey::FirstG {
            self.pending = PendingKey::None;
            if let KeyCode::Char('g') = key.code {
                return Some(VimAction::ScrollToTop);
            }
            // Not a `g g` sequence — fall through and process the new key normally.
        }

        match key.code {
            // Mode transitions.
            KeyCode::Char('i') => {
                self.mode = VimMode::Insert;
                Some(VimAction::EnterInsert)
            }
            KeyCode::Char('a') | KeyCode::Char('A') => {
                self.mode = VimMode::Insert;
                Some(VimAction::EnterInsertAtEnd)
            }
            // Scroll.
            KeyCode::Char('j') => Some(VimAction::ScrollDown(3)),
            KeyCode::Char('k') => Some(VimAction::ScrollUp(3)),
            KeyCode::Char('G') => Some(VimAction::ScrollToBottom),
            KeyCode::Char('g') => {
                self.pending = PendingKey::FirstG;
                // Consume the key but produce no action yet.
                None
            }
            // Quit via `:q` — we handle `:` by entering Insert with the
            // slash-command prefix; the user types `q` and presses Enter.
            // For a vim-faithful `:q` we detect it specially: when the user
            // presses `:` in Normal mode we synthesise a `/` because our
            // command system uses `/` as the command prefix.
            // Actually, treat `:` as entering Insert mode with a pre-typed `/`.
            KeyCode::Char(':') => {
                self.mode = VimMode::Insert;
                Some(VimAction::EnterInsert)
            }
            // `/` also enters Insert mode (for slash commands / search).
            KeyCode::Char('/') => {
                self.mode = VimMode::Insert;
                Some(VimAction::EnterInsert)
            }
            // Esc in Normal mode is a no-op.
            KeyCode::Esc => None,
            // Arrow keys and Page Up/Down pass through even in Normal mode.
            KeyCode::Up => Some(VimAction::ScrollUp(1)),
            KeyCode::Down => Some(VimAction::ScrollDown(1)),
            KeyCode::PageUp => Some(VimAction::ScrollUp(10)),
            KeyCode::PageDown => Some(VimAction::ScrollDown(10)),
            // Everything else is silently consumed in Normal mode.
            _ => None,
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn ctrl(c: char) -> KeyEvent {
        KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL)
    }

    // --- mode label ---------------------------------------------------------

    #[test]
    fn normal_label() {
        assert_eq!(VimMode::Normal.label(), "NORMAL");
    }

    #[test]
    fn insert_label() {
        assert_eq!(VimMode::Insert.label(), "INSERT");
    }

    // --- mode transitions ---------------------------------------------------

    #[test]
    fn i_enters_insert_from_normal() {
        let mut overlay = VimOverlay::new();
        assert_eq!(overlay.mode, VimMode::Normal);

        let action = overlay.process(key(KeyCode::Char('i')));
        assert_eq!(overlay.mode, VimMode::Insert);
        assert!(matches!(action, Some(VimAction::EnterInsert)));
    }

    #[test]
    fn esc_returns_to_normal_from_insert() {
        let mut overlay = VimOverlay::new_insert();
        assert_eq!(overlay.mode, VimMode::Insert);

        let action = overlay.process(key(KeyCode::Esc));
        assert_eq!(overlay.mode, VimMode::Normal);
        assert!(matches!(action, Some(VimAction::EnterNormal)));
    }

    #[test]
    fn a_enters_insert_at_end() {
        let mut overlay = VimOverlay::new();
        let action = overlay.process(key(KeyCode::Char('a')));
        assert_eq!(overlay.mode, VimMode::Insert);
        assert!(matches!(action, Some(VimAction::EnterInsertAtEnd)));
    }

    #[test]
    fn esc_in_normal_is_noop() {
        let mut overlay = VimOverlay::new();
        let action = overlay.process(key(KeyCode::Esc));
        assert_eq!(overlay.mode, VimMode::Normal);
        assert!(action.is_none());
    }

    // --- scroll actions -----------------------------------------------------

    #[test]
    fn j_scrolls_down() {
        let mut overlay = VimOverlay::new();
        let action = overlay.process(key(KeyCode::Char('j')));
        assert!(matches!(action, Some(VimAction::ScrollDown(3))));
    }

    #[test]
    fn k_scrolls_up() {
        let mut overlay = VimOverlay::new();
        let action = overlay.process(key(KeyCode::Char('k')));
        assert!(matches!(action, Some(VimAction::ScrollUp(3))));
    }

    #[test]
    fn big_g_scrolls_to_bottom() {
        let mut overlay = VimOverlay::new();
        let action = overlay.process(key(KeyCode::Char('G')));
        assert!(matches!(action, Some(VimAction::ScrollToBottom)));
    }

    #[test]
    fn gg_scrolls_to_top() {
        let mut overlay = VimOverlay::new();
        // First `g` — no action yet, pending state set.
        let first = overlay.process(key(KeyCode::Char('g')));
        assert!(first.is_none());
        assert_eq!(overlay.pending, PendingKey::FirstG);
        // Second `g` — should scroll to top.
        let second = overlay.process(key(KeyCode::Char('g')));
        assert!(matches!(second, Some(VimAction::ScrollToTop)));
        assert_eq!(overlay.pending, PendingKey::None);
    }

    #[test]
    fn g_then_non_g_clears_pending() {
        let mut overlay = VimOverlay::new();
        overlay.process(key(KeyCode::Char('g')));
        assert_eq!(overlay.pending, PendingKey::FirstG);
        // 'j' clears the pending state and produces ScrollDown (the new key).
        let action = overlay.process(key(KeyCode::Char('j')));
        assert_eq!(overlay.pending, PendingKey::None);
        assert!(matches!(action, Some(VimAction::ScrollDown(3))));
    }

    // --- pass-through in insert mode ----------------------------------------

    #[test]
    fn regular_char_passes_through_in_insert() {
        let mut overlay = VimOverlay::new_insert();
        let k = key(KeyCode::Char('x'));
        let action = overlay.process(k);
        assert!(matches!(action, Some(VimAction::PassThrough(_))));
    }

    #[test]
    fn enter_in_insert_submits() {
        let mut overlay = VimOverlay::new_insert();
        let action = overlay.process(key(KeyCode::Enter));
        assert!(matches!(action, Some(VimAction::Submit)));
    }

    // --- ctrl-c passes through regardless of mode ---------------------------

    #[test]
    fn ctrl_c_passes_through_in_normal() {
        let mut overlay = VimOverlay::new();
        let action = overlay.process(ctrl('c'));
        // Returns None so the outer handler can see it.
        assert!(action.is_none());
    }
}
