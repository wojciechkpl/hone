use ratatui::{Frame, layout::Rect, style::Style};

use crate::app::App;

/// Render the scrollback search bar as a 1-row overlay at the bottom of
/// `conversation_area`.
pub fn render_search_bar(app: &App, frame: &mut Frame, conversation_area: Rect) {
    if conversation_area.height == 0 {
        return;
    }

    let theme = app.theme();
    let query = app.search_query();
    let matches = app.search_matches();
    let cursor = app.search_cursor();

    // The bar sits on the very last row of the conversation area.
    let bar_y = conversation_area.y + conversation_area.height - 1;
    let bar_x = conversation_area.x;
    let bar_width = conversation_area.width;

    let bg_style = Style::default()
        .bg(theme.surface_overlay)
        .fg(theme.text_primary);
    let accent_style = Style::default()
        .bg(theme.surface_overlay)
        .fg(theme.accent_hot);
    let muted_style = Style::default()
        .bg(theme.surface_overlay)
        .fg(theme.text_muted);
    let red_style = Style::default()
        .bg(theme.surface_overlay)
        .fg(theme.role_error);

    let buf = frame.buffer_mut();

    // Fill bar background.
    for col in 0..bar_width {
        buf[(bar_x + col, bar_y)].set_char(' ').set_style(bg_style);
    }

    // Build the text to display.
    let label = "  Search: ";
    let cursor_glyph = "▌";
    let match_info = if query.is_empty() {
        String::new()
    } else if matches.is_empty() {
        "  No matches".to_string()
    } else {
        format!(
            "   {}/{} matches   n next  N prev  Esc close",
            cursor + 1,
            matches.len()
        )
    };

    let mut col = bar_x;
    let right = bar_x + bar_width;

    // Write label in muted style.
    for ch in label.chars() {
        if col >= right {
            break;
        }
        buf[(col, bar_y)].set_char(ch).set_style(muted_style);
        col += 1;
    }

    // Write query + cursor in accent style (or red when no matches).
    let query_style = if !query.is_empty() && matches.is_empty() {
        red_style
    } else {
        accent_style
    };

    let query_with_cursor = format!("{query}{cursor_glyph}");
    for ch in query_with_cursor.chars() {
        if col >= right {
            break;
        }
        buf[(col, bar_y)].set_char(ch).set_style(query_style);
        col += 1;
    }

    // Write match info / hint.
    if !query.is_empty() {
        let info_style = if matches.is_empty() {
            red_style
        } else {
            muted_style
        };
        for ch in match_info.chars() {
            if col >= right {
                break;
            }
            buf[(col, bar_y)].set_char(ch).set_style(info_style);
            col += 1;
        }
    } else {
        // Show a brief hint when no query has been typed yet.
        let hint = "   type to search  Esc close";
        for ch in hint.chars() {
            if col >= right {
                break;
            }
            buf[(col, bar_y)].set_char(ch).set_style(muted_style);
            col += 1;
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use crate::app::App;
    use crate::message::{DisplayMessage, DisplayRole};

    fn make_app_with_messages(contents: &[&str]) -> App {
        let mut app = App::new();
        app.show_splash = false;
        for &c in contents {
            app.messages
                .push(DisplayMessage::new(DisplayRole::System, c));
        }
        app
    }

    #[test]
    fn search_matches_case_insensitive() {
        let mut app =
            make_app_with_messages(&["Hello World", "foo bar", "HELLO again", "unrelated content"]);
        app.search_open = true;
        app.search_query = "hello".to_string();
        app.recompute_search_matches_pub();

        let matches = app.search_matches();
        assert_eq!(
            matches.len(),
            2,
            "should match 'Hello World' and 'HELLO again'"
        );
        assert!(
            matches.contains(&0),
            "index 0 ('Hello World') must be in matches"
        );
        assert!(
            matches.contains(&2),
            "index 2 ('HELLO again') must be in matches"
        );
    }

    #[test]
    fn search_matches_empty_when_no_hit() {
        let mut app = make_app_with_messages(&["alpha beta", "gamma delta"]);
        app.search_open = true;
        app.search_query = "zzznotfound".to_string();
        app.recompute_search_matches_pub();

        assert!(
            app.search_matches().is_empty(),
            "no matches expected for nonsense query"
        );
    }

    #[test]
    fn search_matches_empty_query_returns_no_matches() {
        let mut app = make_app_with_messages(&["hello", "world"]);
        app.search_open = true;
        app.search_query = String::new();
        app.recompute_search_matches_pub();

        assert!(
            app.search_matches().is_empty(),
            "empty query must produce no matches"
        );
    }

    #[test]
    fn search_cursor_wraps_on_advance() {
        use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

        let mut app = make_app_with_messages(&["foo", "bar foo", "baz foo"]);
        app.search_open = true;
        app.search_query = "foo".to_string();
        app.recompute_search_matches_pub();
        assert_eq!(app.search_matches().len(), 3);

        // Advance three times → should wrap back to 0.
        for _ in 0..3 {
            app.handle_key_search_pub(KeyEvent::new(KeyCode::Char('n'), KeyModifiers::NONE));
        }
        assert_eq!(
            app.search_cursor(),
            0,
            "cursor should wrap around after 3 advances on 3 matches"
        );
    }
}
