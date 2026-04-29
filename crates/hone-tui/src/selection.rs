//! Native in-app text selection for the TUI.
//!
//! Mouse drag in the terminal normally gets swallowed by `EnableMouseCapture`
//! (because hone forwards every mouse event to the app), so the terminal
//! emulator's own selection is unavailable. This module re-implements a
//! minimal selection layer on top of the rendered [`Buffer`]: anchor on
//! Left-Down, track on Drag, extract + copy on Left-Up.
//!
//! Coordinates are screen-relative. If the user scrolls mid-drag, callers
//! cancel the selection — this matches what most terminals do anyway.

use ratatui::{buffer::Buffer, style::Modifier};

/// An in-progress or completed mouse selection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Selection {
    /// Cell where the drag started, in terminal screen coordinates.
    pub anchor: (u16, u16),
    /// Most recent cell the cursor moved to during the drag.
    pub current: (u16, u16),
    /// Conversation scroll offset captured at drag start. If the viewport
    /// scrolls mid-drag, the selection is cancelled so on-screen coordinates
    /// stay meaningful.
    pub anchor_scroll: usize,
}

impl Selection {
    pub fn new(col: u16, row: u16, scroll: usize) -> Self {
        Self {
            anchor: (col, row),
            current: (col, row),
            anchor_scroll: scroll,
        }
    }

    /// Return `(start, end)` ordered so `start` is top-left-most in reading
    /// order. Same-row selections order by column; multi-row order by row.
    pub fn normalized(&self) -> ((u16, u16), (u16, u16)) {
        let a = self.anchor;
        let b = self.current;
        if (a.1, a.0) <= (b.1, b.0) {
            (a, b)
        } else {
            (b, a)
        }
    }

    /// True if the selection covers only the anchor cell.
    pub fn is_empty(&self) -> bool {
        self.anchor == self.current
    }
}

/// Iterate the cells inside a selection in reading order.
///
/// Same-row selections yield `(start.0..=end.0, row)`. Multi-row selections
/// yield the anchor row from `start.0` to the line end, every middle row
/// from `0` to line end, and the current row from `0` to `end.0` inclusive.
fn visit_cells(sel: &Selection, width: u16, mut f: impl FnMut(u16, u16)) {
    if width == 0 {
        return;
    }
    let ((sc, sr), (ec, er)) = sel.normalized();
    let last_col = width - 1;

    if sr == er {
        let end = ec.min(last_col);
        for col in sc..=end {
            f(col, sr);
        }
        return;
    }

    // First row: sc..=last_col
    for col in sc..=last_col {
        f(col, sr);
    }
    // Middle rows: full width
    if er > sr + 1 {
        for row in (sr + 1)..er {
            for col in 0..=last_col {
                f(col, row);
            }
        }
    }
    // Last row: 0..=ec
    let end = ec.min(last_col);
    for col in 0..=end {
        f(col, er);
    }
}

/// Overlay a REVERSED modifier on every cell inside the selection.
pub fn apply_highlight(buf: &mut Buffer, sel: &Selection) {
    if sel.is_empty() {
        return;
    }
    let width = buf.area.width;
    let height = buf.area.height;
    if width == 0 || height == 0 {
        return;
    }
    visit_cells(sel, width, |col, row| {
        if row >= height {
            return;
        }
        if let Some(cell) = buf.cell_mut((col, row)) {
            cell.modifier.insert(Modifier::REVERSED);
        }
    });
}

/// Extract the selected text from the rendered buffer.
///
/// Trailing whitespace on each row is trimmed (terminal rows are space-padded
/// to full width). Rows are joined with `\n`.
pub fn extract_text(buf: &Buffer, sel: &Selection) -> String {
    if sel.is_empty() {
        return String::new();
    }
    let width = buf.area.width;
    let height = buf.area.height;
    if width == 0 || height == 0 {
        return String::new();
    }

    let ((sc, sr), (ec, er)) = sel.normalized();
    let last_col = width - 1;

    let read_row = |row: u16, col_start: u16, col_end: u16| -> String {
        if row >= height {
            return String::new();
        }
        let end = col_end.min(last_col);
        let mut line = String::new();
        for col in col_start..=end {
            if let Some(cell) = buf.cell((col, row)) {
                line.push_str(cell.symbol());
            }
        }
        line.trim_end().to_string()
    };

    if sr == er {
        return read_row(sr, sc, ec);
    }

    let mut out = String::new();
    out.push_str(&read_row(sr, sc, last_col));
    for row in (sr + 1)..er {
        out.push('\n');
        out.push_str(&read_row(row, 0, last_col));
    }
    out.push('\n');
    out.push_str(&read_row(er, 0, ec));
    out
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::{buffer::Buffer, layout::Rect, style::Modifier};

    fn buf_with(lines: &[&str]) -> Buffer {
        let width = lines.iter().map(|s| s.chars().count()).max().unwrap_or(0) as u16;
        let height = lines.len() as u16;
        let mut buf = Buffer::empty(Rect::new(0, 0, width, height));
        for (row, line) in lines.iter().enumerate() {
            let mut col: u16 = 0;
            for ch in line.chars() {
                if let Some(cell) = buf.cell_mut((col, row as u16)) {
                    cell.set_char(ch);
                }
                col += 1;
            }
        }
        buf
    }

    #[test]
    fn normalized_orders_top_left_first() {
        let s = Selection {
            anchor: (5, 3),
            current: (2, 1),
            anchor_scroll: 0,
        };
        assert_eq!(s.normalized(), ((2, 1), (5, 3)));

        let s = Selection {
            anchor: (5, 3),
            current: (8, 3),
            anchor_scroll: 0,
        };
        assert_eq!(s.normalized(), ((5, 3), (8, 3)));

        let s = Selection {
            anchor: (8, 3),
            current: (5, 3),
            anchor_scroll: 0,
        };
        assert_eq!(s.normalized(), ((5, 3), (8, 3)));
    }

    #[test]
    fn empty_selection_extracts_nothing() {
        let buf = buf_with(&["hello world"]);
        let sel = Selection::new(3, 0, 0);
        assert_eq!(extract_text(&buf, &sel), "");
    }

    #[test]
    fn single_row_extract_inclusive_range() {
        let buf = buf_with(&["hello world"]);
        let sel = Selection {
            anchor: (0, 0),
            current: (4, 0),
            anchor_scroll: 0,
        };
        assert_eq!(extract_text(&buf, &sel), "hello");
    }

    #[test]
    fn single_row_reversed_drag_still_extracts_forward() {
        let buf = buf_with(&["hello world"]);
        let sel = Selection {
            anchor: (4, 0),
            current: (0, 0),
            anchor_scroll: 0,
        };
        assert_eq!(extract_text(&buf, &sel), "hello");
    }

    #[test]
    fn multi_row_extract_joins_with_newline() {
        let buf = buf_with(&["first line", "second line", "third line"]);
        let sel = Selection {
            anchor: (6, 0),
            current: (5, 2),
            anchor_scroll: 0,
        };
        // First row cols 6..=last: "line"; middle row in full; last row cols
        // 0..=5 = "third " which trim_end collapses to "third".
        assert_eq!(extract_text(&buf, &sel), "line\nsecond line\nthird");
    }

    #[test]
    fn extract_trims_trailing_spaces() {
        // Buffer is wider than the text; trailing cells are blank spaces.
        let mut buf = Buffer::empty(Rect::new(0, 0, 20, 1));
        for (i, ch) in "hi".chars().enumerate() {
            if let Some(cell) = buf.cell_mut((i as u16, 0)) {
                cell.set_char(ch);
            }
        }
        let sel = Selection {
            anchor: (0, 0),
            current: (19, 0),
            anchor_scroll: 0,
        };
        assert_eq!(extract_text(&buf, &sel), "hi");
    }

    #[test]
    fn highlight_sets_reversed_modifier_on_selected_cells() {
        let mut buf = buf_with(&["abcdef"]);
        let sel = Selection {
            anchor: (1, 0),
            current: (3, 0),
            anchor_scroll: 0,
        };
        apply_highlight(&mut buf, &sel);

        let has_reversed = |col: u16| {
            buf.cell((col, 0))
                .map(|c| c.modifier.contains(Modifier::REVERSED))
                .unwrap_or(false)
        };
        assert!(!has_reversed(0));
        assert!(has_reversed(1));
        assert!(has_reversed(2));
        assert!(has_reversed(3));
        assert!(!has_reversed(4));
    }

    #[test]
    fn highlight_noop_when_empty() {
        let mut buf = buf_with(&["abcdef"]);
        let sel = Selection::new(2, 0, 0);
        apply_highlight(&mut buf, &sel);
        for col in 0..6 {
            let m = buf
                .cell((col, 0))
                .map(|c| c.modifier)
                .unwrap_or(Modifier::empty());
            assert!(
                !m.contains(Modifier::REVERSED),
                "cell {col} unexpectedly reversed"
            );
        }
    }

    #[test]
    fn extract_clamps_past_buffer_width() {
        let buf = buf_with(&["short"]);
        // End column 50 exceeds buffer width 5; should clamp to the last col.
        let sel = Selection {
            anchor: (0, 0),
            current: (50, 0),
            anchor_scroll: 0,
        };
        assert_eq!(extract_text(&buf, &sel), "short");
    }
}
