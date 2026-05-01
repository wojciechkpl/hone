use ratatui::{Frame, layout::Rect, style::Style};

use crate::{app::App, app::palette_matches};

/// A single entry in the command palette.
pub struct PaletteEntry {
    pub name: &'static str,
    pub description: &'static str,
}

/// All commands exposed in the palette.
pub const PALETTE_COMMANDS: &[PaletteEntry] = &[
    PaletteEntry {
        name: "quit",
        description: "Exit hone",
    },
    PaletteEntry {
        name: "help",
        description: "Show keyboard shortcuts",
    },
    PaletteEntry {
        name: "clear",
        description: "Clear conversation history",
    },
    PaletteEntry {
        name: "review",
        description: "Enter review mode",
    },
    PaletteEntry {
        name: "configure",
        description: "Open configuration wizard",
    },
    PaletteEntry {
        name: "tdd",
        description: "Toggle step-through debugger",
    },
    PaletteEntry {
        name: "model",
        description: "Switch AI model",
    },
    PaletteEntry {
        name: "cost",
        description: "Show session cost",
    },
    PaletteEntry {
        name: "theme",
        description: "Switch color theme",
    },
    PaletteEntry {
        name: "clarify",
        description: "Force clarification step",
    },
];

/// Render the command palette overlay centered on `area`.
pub fn render_command_palette(app: &App, frame: &mut Frame, area: Rect) {
    let theme = app.theme();
    let query = app.palette_query();
    let selection = app.palette_selection();

    // Collect filtered entries.
    let filtered: Vec<&PaletteEntry> = PALETTE_COMMANDS
        .iter()
        .filter(|e| palette_matches(e.name, e.description, query))
        .collect();

    let row_count = filtered.len() as u16;
    // Height = rows + top_border + query_footer + bottom_border = rows + 3
    // but we reserve 1 extra row for the footer hint line → rows + 4, capped at 16.
    let popup_height = (row_count + 4).min(16);
    let max_width = area.width.saturating_sub(4);
    let popup_width = 70u16.min(area.width * 60 / 100).min(max_width).max(30);

    // Center the popup.
    let popup_x = area.x + (area.width.saturating_sub(popup_width)) / 2;
    let popup_y = area.y + (area.height.saturating_sub(popup_height)) / 3;
    let popup = Rect::new(popup_x, popup_y, popup_width, popup_height);

    let bg_style = Style::default()
        .bg(theme.surface_overlay)
        .fg(theme.text_primary);
    let border_style = Style::default()
        .bg(theme.surface_overlay)
        .fg(theme.surface_border);
    let accent_style = Style::default()
        .bg(theme.surface_overlay)
        .fg(theme.accent_hot);
    let muted_style = Style::default()
        .bg(theme.surface_overlay)
        .fg(theme.text_muted);
    let sel_style = Style::default().bg(theme.accent_mid).fg(theme.surface_base);
    let sel_accent_style = Style::default().bg(theme.accent_mid).fg(theme.surface_base);
    let red_style = Style::default()
        .bg(theme.surface_overlay)
        .fg(theme.role_error);

    let buf = frame.buffer_mut();

    // --- Fill background ---------------------------------------------------
    for row in 0..popup_height {
        for col in 0..popup_width {
            buf[(popup.x + col, popup.y + row)]
                .set_char(' ')
                .set_style(bg_style);
        }
    }

    let right = popup.x + popup_width - 1;
    let bottom = popup.y + popup_height - 1;

    // --- Draw rounded border ----------------------------------------------
    buf[(popup.x, popup.y)]
        .set_char('╭')
        .set_style(border_style);
    buf[(right, popup.y)].set_char('╮').set_style(border_style);
    buf[(popup.x, bottom)].set_char('╰').set_style(border_style);
    buf[(right, bottom)].set_char('╯').set_style(border_style);

    for col in 1..popup_width - 1 {
        buf[(popup.x + col, popup.y)]
            .set_char('─')
            .set_style(border_style);
        buf[(popup.x + col, bottom)]
            .set_char('─')
            .set_style(border_style);
    }
    for row in 1..popup_height - 1 {
        buf[(popup.x, popup.y + row)]
            .set_char('│')
            .set_style(border_style);
        buf[(right, popup.y + row)]
            .set_char('│')
            .set_style(border_style);
    }

    // --- Title in top border ----------------------------------------------
    let title = " Commands ";
    let title_start = popup.x + (popup_width.saturating_sub(title.len() as u16)) / 2;
    let mut col = title_start;
    for ch in title.chars() {
        buf[(col, popup.y)].set_char(ch).set_style(border_style);
        col += 1;
    }

    // --- Command rows (inside border, from row 1) -------------------------
    let inner_width = popup_width.saturating_sub(2) as usize;
    let max_rows = (popup_height.saturating_sub(4)) as usize; // reserve footer + borders

    for (i, entry) in filtered.iter().take(max_rows).enumerate() {
        let row_y = popup.y + 1 + i as u16;
        let is_sel = i == selection;
        let row_bg = if is_sel { sel_style } else { bg_style };
        let name_style = if is_sel {
            sel_accent_style
        } else {
            accent_style
        };
        let desc_secondary_style = Style::default()
            .bg(theme.surface_overlay)
            .fg(theme.text_secondary);
        // For selected rows, use text_primary as the description fg so that
        // the foreground index distance from surface_overlay is >= 6.
        // (surface_base, which sel_style uses, is Indexed(233) — only 2 shades
        //  from surface_overlay Indexed(235).)
        let sel_desc_style = Style::default().bg(theme.accent_mid).fg(theme.text_primary);
        let desc_style = if is_sel {
            sel_desc_style
        } else {
            desc_secondary_style
        };

        // Fill row background.
        for c in 0..inner_width as u16 {
            buf[(popup.x + 1 + c, row_y)]
                .set_char(' ')
                .set_style(row_bg);
        }

        // Write " /name   description"
        let name_text = format!(" /{}", entry.name);
        let name_col_end = popup.x + 1 + name_text.len() as u16;

        let mut c = popup.x + 1;
        for ch in name_text.chars().take(inner_width) {
            buf[(c, row_y)].set_char(ch).set_style(name_style);
            c += 1;
        }

        // Pad with spaces to a fixed name column width then description.
        let name_col_width = 14usize; // "/configure   " style padding
        let pad_end = popup.x + 1 + name_col_width as u16;
        while c < pad_end && c < popup.x + popup_width - 1 {
            buf[(c, row_y)].set_char(' ').set_style(row_bg);
            c += 1;
        }
        let _ = name_col_end; // suppress warning

        let desc_start = c;
        let desc_budget = (popup.x + popup_width - 1).saturating_sub(desc_start) as usize;
        for ch in entry.description.chars().take(desc_budget) {
            buf[(c, row_y)].set_char(ch).set_style(desc_style);
            c += 1;
        }
    }

    // --- Separator line before footer ------------------------------------
    let sep_y = popup.y + popup_height - 3;
    buf[(popup.x, sep_y)].set_char('├').set_style(border_style);
    buf[(right, sep_y)].set_char('┤').set_style(border_style);
    for col in 1..popup_width - 1 {
        buf[(popup.x + col, sep_y)]
            .set_char('─')
            .set_style(border_style);
    }

    // --- Query line -------------------------------------------------------
    let query_y = popup.y + popup_height - 2;
    let query_label = "  query: ";
    let cursor_char = "▌";
    let query_display = format!("{query_label}{query}{cursor_char}");
    let mut c = popup.x + 1;
    for ch in query_display.chars().take(inner_width) {
        let style = if c >= popup.x + 1 + query_label.len() as u16 {
            accent_style
        } else {
            muted_style
        };
        buf[(c, query_y)].set_char(ch).set_style(style);
        c += 1;
    }

    // Show "No matches" if filtered list is empty and query is non-empty.
    if filtered.is_empty() && !query.is_empty() {
        let msg = "  No matches";
        let mut c = popup.x + 1;
        for ch in msg.chars().take(inner_width) {
            buf[(c, popup.y + 1)].set_char(ch).set_style(red_style);
            c += 1;
        }
    }

    // --- Hint footer at the bottom ----------------------------------------
    // (bottom is the border row, hint goes in the row above)
    // Actually bottom is drawn as border; we put hint in query_y area — already done.
    // Put navigation hint in bottom border region? No — show it at bottom - 1 = query_y.
    // The hint at the very bottom border is embedded in the border itself (minimal).
    // Write a compact hint line.
    let hint = "  \u{2191}\u{2193} nav  Enter select  Esc cancel";
    let hint_y = bottom - 1; // one row above bottom border → same as query_y - 1?
    // Actually layout: top_border(0), rows(1..max_rows+1), sep(popup_height-3),
    // query(popup_height-2), hint(popup_height-1=bottom).
    // But bottom is drawn as border ╰─╯. So let's fit hint right into the bottom border title.
    let hint_start = popup.x + (popup_width.saturating_sub(hint.len() as u16 + 2)) / 2;
    let _ = hint_y;
    let mut hc = hint_start;
    for ch in hint.chars() {
        if hc >= right {
            break;
        }
        buf[(hc, bottom)].set_char(ch).set_style(muted_style);
        hc += 1;
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::App;

    fn filtered_names(query: &str) -> Vec<&'static str> {
        PALETTE_COMMANDS
            .iter()
            .filter(|e| palette_matches(e.name, e.description, query))
            .map(|e| e.name)
            .collect()
    }

    #[test]
    fn palette_filters_by_query() {
        // "quit" should match exactly one entry.
        let names = filtered_names("quit");
        assert_eq!(names, vec!["quit"], "query 'quit' must match only /quit");

        // Case-insensitive: "HELP" should match "help".
        let names = filtered_names("HELP");
        assert!(
            names.contains(&"help"),
            "query 'HELP' must match /help case-insensitively"
        );

        // Description match: "Exit" matches /quit via description.
        let names = filtered_names("Exit");
        assert!(
            names.contains(&"quit"),
            "query 'Exit' must match /quit via description"
        );

        // Empty query returns all entries.
        let all = filtered_names("");
        assert_eq!(
            all.len(),
            PALETTE_COMMANDS.len(),
            "empty query must return all commands"
        );

        // Non-matching query returns empty.
        let none = filtered_names("zzz-no-match");
        assert!(none.is_empty(), "nonsense query must return no matches");
    }

    #[test]
    fn palette_selection_clamps() {
        let mut app = App::new();
        app.show_splash = false;

        // Open the palette.
        app.palette_open = true;
        app.palette_query = String::new();
        app.palette_selection = 0;

        // Press Up from 0 — must stay at 0.
        use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
        app.handle_key_palette_pub(KeyEvent::new(KeyCode::Up, KeyModifiers::NONE));
        assert_eq!(app.palette_selection, 0, "Up from 0 must clamp to 0");

        // Press Down repeatedly past the end.
        let total = PALETTE_COMMANDS.len();
        for _ in 0..total + 5 {
            app.handle_key_palette_pub(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
        }
        assert_eq!(
            app.palette_selection,
            total - 1,
            "Down past end must clamp at last index"
        );
    }

    #[test]
    fn palette_enter_inserts_command_into_buffer() {
        let mut app = App::new();
        app.show_splash = false;
        app.palette_open = true;
        app.palette_query = "quit".to_string();
        app.palette_selection = 0;

        use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
        app.handle_key_palette_pub(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));

        assert!(!app.palette_open, "palette must close on Enter");
        assert!(
            app.input.buffer.starts_with("/quit"),
            "input buffer must contain selected command"
        );
    }

    #[test]
    fn palette_esc_closes_without_inserting() {
        let mut app = App::new();
        app.show_splash = false;
        app.palette_open = true;
        app.palette_query = "quit".to_string();
        app.palette_selection = 0;

        use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
        app.handle_key_palette_pub(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));

        assert!(!app.palette_open, "Esc must close the palette");
        assert!(
            app.input.buffer.is_empty(),
            "Esc must not modify the input buffer"
        );
    }

    // -----------------------------------------------------------------------
    // TUI bug tests (red phase)
    // -----------------------------------------------------------------------

    /// The description text rendered in a command palette row must use a
    /// foreground color that provides sufficient contrast against the row
    /// background (`theme.surface_overlay`).
    ///
    /// Bug: `desc_style` uses `theme.text_muted` (Indexed(238)) on
    /// `theme.surface_overlay` (Indexed(235)) — only 3 shades apart in the
    /// ANSI-256 ramp.  The fix must change `desc_style` to use
    /// `theme.text_secondary` (Indexed(244)) or any color whose index distance
    /// from `surface_overlay` is at least 6.
    ///
    /// Contract: for each cell that contains a character from the description
    /// of the /quit row, the foreground color must differ from
    /// `surface_overlay` by an ANSI-256 index distance >= 6.
    #[test]
    fn palette_description_fg_has_sufficient_contrast_against_row_bg() {
        use crate::render::root::render;
        use ratatui::style::Color;
        use ratatui::{Terminal, backend::TestBackend};

        let mut app = App::new();
        app.show_splash = false;
        app.palette_open = true;
        // Empty query shows all entries; /quit is the first one.
        app.palette_query = String::new();
        app.palette_selection = 0;

        let backend = TestBackend::new(80, 24);
        let mut terminal = Terminal::new(backend).expect("TestBackend");
        terminal.draw(|f| render(&app, f)).expect("draw");

        let theme = crate::theme::Theme::crimson();

        // Extract surface_overlay index so we can measure contrast distance.
        let overlay_index = match theme.surface_overlay {
            Color::Indexed(i) => i,
            _ => panic!("expected surface_overlay to be Color::Indexed"),
        };

        // The /quit row description "Exit hone" starts after the name column
        // padding.  The palette popup is centered on an 80-wide terminal.
        // popup_width = 70.min(80*60/100).min(76).max(30) = 48
        // popup_x = (80 - 48) / 2 = 16
        // popup_y = (24 - popup_height) / 3; popup_height = min(10+4,16)=14
        // popup_y = (24-14)/3 = 3
        // Row 0 of commands is at y = popup_y + 1 = 4.
        // Name column starts at popup_x+1=17, name_col_width=14.
        // Description starts at popup_x + 1 + 14 = 31.
        let desc_start_x: u16 = 31;
        let row_y: u16 = 4;

        // Find at least one non-space cell in the description column region
        // and verify its foreground color index distance from surface_overlay.
        let buf = terminal.backend().buffer();
        let mut found_desc_cell = false;
        for x in desc_start_x..80 {
            let cell = buf.cell((x, row_y)).expect("cell exists");
            let ch = cell.symbol().chars().next().unwrap_or(' ');
            if ch == ' ' {
                continue;
            }
            // We found a character — this should be from "Exit hone".
            found_desc_cell = true;
            let fg = cell.style().fg.unwrap_or(Color::Reset);
            let desc_fg_index = match fg {
                Color::Indexed(i) => i,
                other => panic!(
                    "description cell fg must be Color::Indexed, got {:?}",
                    other
                ),
            };
            // The contrast distance must be >= 6 in the ANSI-256 ramp.
            let distance = (desc_fg_index as i16 - overlay_index as i16).unsigned_abs();
            assert!(
                distance >= 6,
                "description fg Indexed({desc_fg_index}) is only {distance} shades from \
                 surface_overlay Indexed({overlay_index}) — insufficient contrast; \
                 use theme.text_secondary (Indexed(244)) or a color >= 6 indices away"
            );
            // Only check the first description character to keep the test focused.
            break;
        }

        assert!(
            found_desc_cell,
            "could not find any non-space character in the description column at y={row_y}, \
             x={desc_start_x}..80 — is the palette rendering the description at all?"
        );
    }
}
