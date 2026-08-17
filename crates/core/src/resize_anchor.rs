//! Bottom-anchored resize: pull scrollback back into trailing blank rows.
//!
//! Alacritty's column reflow keeps the cursor's distance from the bottom of
//! the screen fixed. When narrowing the window wraps lines, the extra rows
//! push the top of the screen into scrollback history even when blank rows
//! below the cursor could absorb them — visually the prompt gets stranded
//! mid-screen above dead rows while the start of the output is cut off.
//! Ghostty, Kitty, and Terminal.app instead fill downward into the blank
//! rows first, keeping content bottom-anchored.
//!
//! This module restores that behavior after a resize by round-tripping
//! through alacritty's own row resize paths: shrinking rows drops trailing
//! blank rows (the cursor sits above them, so nothing scrolls), and growing
//! back pulls the same number of lines out of history, moving the cursor
//! down with the restored content.

use alacritty_terminal::event::EventListener;
use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::index::Line;
use alacritty_terminal::term::{Term, TermMode};
use alacritty_terminal::vte::ansi;
use std::sync::atomic::{AtomicBool, Ordering};

use crate::runtime::TerminalSize;

/// Tracks when a primary-screen clear made trailing blank rows intentional.
///
/// ED2 preserves scrollback, so bottom anchoring must stay disabled until new
/// output reaches the bottom of the viewport. Otherwise a resize mistakes the
/// cleared rows for spare layout space and pulls old history back on-screen.
#[derive(Debug, Default)]
pub(crate) struct ResizeAnchorState {
    suppressed_after_clear: AtomicBool,
}

impl ResizeAnchorState {
    pub(crate) fn clear_screen<T: EventListener>(&self, term: &Term<T>, mode: &ansi::ClearMode) {
        match mode {
            ansi::ClearMode::All if !term.mode().contains(TermMode::ALT_SCREEN) => {
                self.suppressed_after_clear.store(true, Ordering::Release);
            }
            ansi::ClearMode::Saved => self.reset(),
            _ => (),
        }
    }

    pub(crate) fn observe_live_output<T: EventListener>(&self, term: &Term<T>) {
        if !self.suppressed_after_clear.load(Ordering::Acquire)
            || term.mode().contains(TermMode::ALT_SCREEN)
        {
            return;
        }

        let screen_lines = term.grid().screen_lines();
        let cursor_line = term.grid().cursor.point.line.0.max(0) as usize;
        if screen_lines > 0 && cursor_line.saturating_add(1) >= screen_lines {
            self.reset();
        }
    }

    pub(crate) fn reset(&self) {
        self.suppressed_after_clear.store(false, Ordering::Release);
    }

    pub(crate) fn allows_restore(&self) -> bool {
        !self.suppressed_after_clear.load(Ordering::Acquire)
    }
}

/// Pulls history lines back into blank rows trailing below the cursor.
/// Call after `Term::resize`; no-op on the alternate screen, when history is
/// empty, or when the screen is already filled down to the cursor.
pub(crate) fn restore_bottom_anchor<T: EventListener>(term: &mut Term<T>, size: TerminalSize) {
    if term.mode().contains(TermMode::ALT_SCREEN) {
        return;
    }

    let grid = term.grid();
    let history = grid.history_size();
    if history == 0 {
        return;
    }

    let screen_lines = grid.screen_lines();
    let cursor_line = grid.cursor.point.line.0.max(0) as usize;

    // Blank rows contiguous from the bottom of the screen, strictly below the
    // cursor row. Stops at the first row with content so TUI output drawn
    // below the cursor is never dropped.
    let mut blank_rows = 0;
    for line in (cursor_line + 1..screen_lines).rev() {
        if grid[Line(line as i32)].is_clear() {
            blank_rows += 1;
        } else {
            break;
        }
    }

    let pull = blank_rows.min(history);
    if pull == 0 {
        return;
    }

    let mut shrunk = size;
    shrunk.rows = size.rows.saturating_sub(pull as u16).max(1);
    term.resize(shrunk);
    term.resize(size);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime::TerminalSize;
    use alacritty_terminal::event::VoidListener;
    use alacritty_terminal::term::Config as TermConfig;
    use alacritty_terminal::vte::ansi;

    fn size(cols: u16, rows: u16) -> TerminalSize {
        TerminalSize {
            cols,
            rows,
            cell_width: 9.0,
            cell_height: 18.0,
        }
    }

    fn term_with(input: &[u8], cols: u16, rows: u16) -> Term<VoidListener> {
        let mut term = Term::new(TermConfig::default(), &size(cols, rows), VoidListener);
        let mut parser: ansi::Processor = ansi::Processor::new();
        parser.advance(&mut term, input);
        term
    }

    fn visible_row_text(term: &Term<VoidListener>, line: usize) -> String {
        let grid = term.grid();
        let row = &grid[Line(line as i32)];
        (0..row.len())
            .map(|col| row[alacritty_terminal::index::Column(col)].c)
            .collect()
    }

    /// Narrowing wraps three 18-char lines to two rows each. Stock alacritty
    /// pushes the top three rows into history while six blank rows idle below
    /// the prompt; the anchor must pull them back so the full output stays
    /// visible and the prompt sits directly under it.
    #[test]
    fn pulls_history_into_trailing_blank_rows_on_column_shrink() {
        let mut term = term_with(
            b"AAAAAAAAAAAAAAAAAA\r\nBBBBBBBBBBBBBBBBBB\r\nCCCCCCCCCCCCCCCCCC\r\n$ ",
            20,
            10,
        );

        let narrow = size(10, 10);
        term.resize(narrow);
        // Reproduce the stock behavior this module exists to correct.
        assert_eq!(term.grid().history_size(), 3);
        assert_eq!(term.grid().cursor.point.line.0, 3);

        restore_bottom_anchor(&mut term, narrow);

        assert_eq!(term.grid().history_size(), 0);
        assert_eq!(term.grid().cursor.point.line.0, 6);
        assert_eq!(visible_row_text(&term, 0), "AAAAAAAAAA");
        assert_eq!(visible_row_text(&term, 1), "AAAAAAAA  ");
        assert_eq!(visible_row_text(&term, 6).trim_end(), "$");
    }

    /// A filled screen has no trailing blanks: overflow belongs in history and
    /// the anchor must leave the grid untouched.
    #[test]
    fn keeps_history_when_screen_is_full() {
        let mut input = Vec::new();
        for i in 0..12 {
            input.extend_from_slice(format!("line {i}\r\n").as_bytes());
        }
        input.extend_from_slice(b"$ ");
        let mut term = term_with(&input, 20, 10);

        let history_before = term.grid().history_size();
        let cursor_before = term.grid().cursor.point.line.0;
        assert!(history_before > 0);

        restore_bottom_anchor(&mut term, size(20, 10));

        assert_eq!(term.grid().history_size(), history_before);
        assert_eq!(term.grid().cursor.point.line.0, cursor_before);
    }

    #[test]
    fn does_nothing_when_history_is_empty() {
        let mut term = term_with(b"hello\r\n$ ", 20, 10);

        restore_bottom_anchor(&mut term, size(20, 10));

        assert_eq!(term.grid().history_size(), 0);
        assert_eq!(term.grid().cursor.point.line.0, 1);
    }

    #[test]
    fn does_nothing_on_the_alternate_screen() {
        let mut input = Vec::new();
        for i in 0..15 {
            input.extend_from_slice(format!("line {i}\r\n").as_bytes());
        }
        // Enter the alternate screen (vim/htop); it has no scrollback.
        input.extend_from_slice(b"\x1b[?1049h");
        let mut term = term_with(&input, 20, 10);
        assert!(term.mode().contains(TermMode::ALT_SCREEN));

        let cursor_before = term.grid().cursor.point.line.0;
        restore_bottom_anchor(&mut term, size(20, 10));
        assert_eq!(term.grid().cursor.point.line.0, cursor_before);
    }

    /// Pulls only as many rows as history holds when blanks outnumber it, and
    /// only as many as fit when history outnumbers the blanks.
    #[test]
    fn pull_is_bounded_by_history_and_blank_rows() {
        // More blanks than history: 18-char line wraps once -> history 1,
        // blanks below cursor ~7 -> pull exactly 1.
        let mut term = term_with(b"AAAAAAAAAAAAAAAAAA\r\n$ ", 20, 10);
        let narrow = size(10, 10);
        term.resize(narrow);
        assert_eq!(term.grid().history_size(), 1);

        restore_bottom_anchor(&mut term, narrow);

        assert_eq!(term.grid().history_size(), 0);
        assert_eq!(term.grid().cursor.point.line.0, 2);
    }
}
