use std::sync::{Mutex, MutexGuard};

use termy_core::{
    DetectedLink, DetectedViewportLink, KittyGraphicsRenderPlacement, Terminal as CoreTerminal,
    TerminalCursorState, TerminalDirtySpan, TerminalEngineDiagnostics, TerminalEvent,
    TerminalKeyboardMode, TerminalMouseMode, TerminalOptions, TerminalPalette, TerminalQueryColors,
    TerminalRenderCell, TerminalRenderDamageSnapshot, TerminalRenderRead, TerminalReplyHost,
    TerminalRuntimeConfig, TerminalSize, TerminalViewportMetadata, TerminalWakeupNotifier,
};

/// In-memory terminal emulator for a tmux pane.
///
/// Tmux owns the process, title, and input transport. This type owns only the
/// display-side terminal state used to hydrate captures and apply live output.
pub struct PaneTerminal {
    terminal: Mutex<CoreTerminal>,
}

impl PaneTerminal {
    fn runtime_config(options: TerminalOptions) -> TerminalRuntimeConfig {
        TerminalRuntimeConfig {
            scrollback_history: options.scrollback_history,
            default_cursor_style: options.default_cursor_style,
            ..TerminalRuntimeConfig::default()
        }
    }

    fn lock(&self) -> MutexGuard<'_, CoreTerminal> {
        self.terminal
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    pub fn new(size: TerminalSize, options: TerminalOptions) -> Self {
        Self::new_with_wakeup_notifier(size, options, None)
    }

    pub fn new_with_wakeup_notifier(
        size: TerminalSize,
        options: TerminalOptions,
        wakeup_notifier: Option<TerminalWakeupNotifier>,
    ) -> Self {
        let runtime_config = Self::runtime_config(options);
        Self {
            terminal: Mutex::new(CoreTerminal::new_display_with_wakeup_notifier(
                size,
                Some(&runtime_config),
                wakeup_notifier,
            )),
        }
    }

    /// Apply live tmux control-mode output. Synchronized-update fragments stay
    /// staged until their commit sequence or watchdog produces a wakeup.
    pub fn feed_output(&self, bytes: &[u8]) {
        self.lock().feed_output(bytes);
    }

    /// Replay a bounded `capture-pane` snapshot without carrying an incomplete
    /// escape or UTF-8 sequence into the subsequent live stream.
    pub fn hydrate_output(&self, bytes: &[u8]) {
        self.lock().hydrate_output(bytes);
    }

    pub fn resize(&self, new_size: TerminalSize) {
        self.lock().resize(new_size);
    }

    pub fn size(&self) -> TerminalSize {
        self.lock().size()
    }

    pub fn engine_label(&self) -> &'static str {
        self.lock().engine_label()
    }

    pub fn engine_diagnostics(&self) -> TerminalEngineDiagnostics {
        self.lock().engine_diagnostics().clone()
    }

    /// Drain display events and protocol replies. Tmux remains authoritative
    /// for pane lifecycle, titles, and process state; callers decide which
    /// display events should trigger presentation work.
    pub fn drain_events(&self, host: &mut impl TerminalReplyHost) -> (Vec<TerminalEvent>, bool) {
        self.lock().drain_events(host)
    }

    pub fn set_query_colors(&self, query_colors: TerminalQueryColors) {
        self.lock().set_query_colors(query_colors);
    }

    pub fn palette(&self) -> TerminalPalette {
        self.lock().palette()
    }

    pub fn take_render_damage_snapshot(&self) -> TerminalRenderDamageSnapshot {
        self.lock().take_render_damage_snapshot()
    }

    pub fn render_read(&self, force_full: bool) -> TerminalRenderRead {
        self.lock().render_read(force_full)
    }

    /// Visit one coherent viewport snapshot. The callback must not call back
    /// into this `PaneTerminal` while the visit is in progress.
    pub fn visit_viewport_cells(
        &self,
        visitor: impl FnMut(usize, i32, usize, &TerminalRenderCell),
    ) -> TerminalViewportMetadata {
        self.lock().visit_viewport_cells(visitor)
    }

    /// Visit dirty ranges only when `generation` still names the current core
    /// state. The callback must not call back into this terminal.
    pub fn visit_viewport_ranges_at_generation(
        &self,
        generation: u64,
        spans: &[TerminalDirtySpan],
        visitor: impl FnMut(usize, usize, i32, usize, &TerminalRenderCell),
    ) -> bool {
        self.lock()
            .visit_viewport_ranges_at_generation(generation, spans, visitor)
    }

    pub fn line_bounds(&self) -> (i32, i32) {
        self.lock().line_bounds()
    }

    /// Visit an inclusive buffer-line range under one coherent core read.
    /// The callback must not call back into this terminal.
    pub fn visit_line_cells(
        &self,
        requested_first: i32,
        requested_last: i32,
        visitor: impl FnMut((i32, i32, usize), i32, usize, &TerminalRenderCell),
    ) -> (i32, i32, usize) {
        self.lock()
            .visit_line_cells(requested_first, requested_last, visitor)
    }

    pub fn hyperlink_at(&self, row: usize, col: usize) -> Option<DetectedLink> {
        self.lock().hyperlink_at(row, col)
    }

    pub fn link_at(&self, row: usize, col: usize) -> Option<DetectedViewportLink> {
        self.lock().link_at(row, col)
    }

    pub fn scroll_display(&self, delta_lines: i32) -> bool {
        self.lock().scroll_display(delta_lines)
    }

    pub fn scroll_to_bottom(&self) -> bool {
        self.lock().scroll_to_bottom()
    }

    pub fn scroll_state(&self) -> (usize, usize) {
        self.lock().scroll_state()
    }

    pub fn cursor_state(&self) -> Option<TerminalCursorState> {
        self.lock().cursor_state()
    }

    /// Returns the cursor position regardless of visibility (for IME positioning).
    pub fn cursor_position(&self) -> (usize, usize) {
        self.lock().cursor_position()
    }

    pub fn set_term_options(&self, options: TerminalOptions) {
        self.lock().set_term_options(options);
    }

    pub fn bracketed_paste_mode(&self) -> bool {
        self.lock().bracketed_paste_mode()
    }

    pub fn mouse_mode(&self) -> TerminalMouseMode {
        self.lock().mouse_mode()
    }

    pub fn keyboard_mode(&self) -> TerminalKeyboardMode {
        self.lock().keyboard_mode()
    }

    pub fn alternate_screen_mode(&self) -> bool {
        self.lock().alternate_screen_mode()
    }

    pub fn kitty_graphics_placements(&self) -> Vec<KittyGraphicsRenderPlacement> {
        self.lock().kitty_graphics_placements()
    }
}

#[cfg(test)]
mod tests {
    use super::PaneTerminal;
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };
    use termy_core::{
        TerminalDamageSnapshot, TerminalEvent, TerminalOptions, TerminalSize,
        TerminalWakeupNotifier,
    };

    fn test_term_options(scrollback_history: usize) -> TerminalOptions {
        TerminalOptions {
            scrollback_history,
            ..TerminalOptions::default()
        }
    }

    fn visible_viewport_text(terminal: &PaneTerminal) -> String {
        let size = terminal.size();
        let cols = usize::from(size.cols);
        let rows = usize::from(size.rows);
        let mut grid = vec![vec![' '; cols]; rows];

        terminal.visit_viewport_cells(|display_offset, line, col, cell| {
            let row = line + i32::try_from(display_offset).unwrap_or(i32::MAX);
            if row < 0 {
                return;
            }
            let row = row as usize;
            if row >= rows || col >= cols || cell.wide_character_spacer || cell.hidden {
                return;
            }
            let character = cell.text.chars().next().unwrap_or('\0');
            if character != '\0' && !character.is_control() {
                grid[row][col] = character;
            }
        });

        grid.into_iter()
            .map(|row| {
                let line: String = row.into_iter().collect();
                line.trim_end().to_string()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn drain_events(terminal: &PaneTerminal) -> (Vec<TerminalEvent>, bool) {
        terminal.drain_events(&mut |_| None)
    }

    #[test]
    fn feed_output_handles_tmux_prompt_repaint_without_prefix_duplication() {
        let terminal = PaneTerminal::new(
            TerminalSize {
                cols: 120,
                rows: 10,
                ..TerminalSize::default()
            },
            test_term_options(2000),
        );

        terminal.feed_output(
            b"c\x08cd Desk\x08\x08\x08\x08\x08\x08\x08\x1b[32mc\x1b[32md\x1b[39m\x1b[5C\r\r\n",
        );
        terminal.feed_output(b"cd: no such file or directory: Desk\r\n");
        terminal.feed_output(b"c\x08\x1b[4mc\x1b[24m\r\r\n");
        terminal.feed_output(b"zsh: command not found: c\r\n");

        let visible = visible_viewport_text(&terminal);
        assert!(visible.contains("cd: no such file or directory: Desk"));
        assert!(visible.contains("zsh: command not found: c"));
        assert!(!visible.contains("cdcd:"));
        assert!(!visible.contains("czsh:"));
    }

    #[test]
    fn clamps_zero_size_on_new_and_resize() {
        let terminal = PaneTerminal::new(
            TerminalSize {
                cols: 0,
                rows: 0,
                ..TerminalSize::default()
            },
            test_term_options(2000),
        );
        assert_eq!(terminal.size().cols, 1);
        assert_eq!(terminal.size().rows, 1);

        terminal.resize(TerminalSize {
            cols: 0,
            rows: 0,
            ..TerminalSize::default()
        });
        assert_eq!(terminal.size().cols, 1);
        assert_eq!(terminal.size().rows, 1);
    }

    #[test]
    fn resize_keeps_core_and_pane_dimensions_in_sync() {
        let terminal = PaneTerminal::new(
            TerminalSize {
                cols: 4,
                rows: 3,
                ..TerminalSize::default()
            },
            test_term_options(2000),
        );

        terminal.resize(TerminalSize {
            cols: 9,
            rows: 7,
            ..TerminalSize::default()
        });

        assert_eq!(terminal.size().cols, 9);
        assert_eq!(terminal.size().rows, 7);
    }

    #[test]
    fn capture_hydration_does_not_carry_truncated_escape_into_live_output() {
        let terminal = PaneTerminal::new(TerminalSize::default(), test_term_options(2000));

        terminal.hydrate_output(b"capture\r\n\x1b[31");
        terminal.feed_output(b"LIVE");

        let visible = visible_viewport_text(&terminal);
        assert!(visible.contains("capture"));
        assert!(visible.contains("LIVE"));
    }

    #[test]
    fn capture_then_live_output_has_no_missing_or_duplicate_marker() {
        let terminal = PaneTerminal::new(TerminalSize::default(), test_term_options(2000));

        terminal.hydrate_output(b"CAPTURE\r\n");
        terminal.feed_output(b"LIVE-MARKER");

        let visible = visible_viewport_text(&terminal);
        assert_eq!(visible.matches("CAPTURE").count(), 1);
        assert_eq!(visible.matches("LIVE-MARKER").count(), 1);
    }

    #[test]
    fn synchronized_output_wakes_only_after_commit() {
        let wakeups = Arc::new(AtomicUsize::new(0));
        let observed = wakeups.clone();
        let terminal = PaneTerminal::new_with_wakeup_notifier(
            TerminalSize::default(),
            test_term_options(2000),
            Some(TerminalWakeupNotifier::new(move || {
                observed.fetch_add(1, Ordering::Relaxed);
            })),
        );

        terminal.feed_output(b"\x1b[?2026hSTAGED");
        let (events, has_more) = drain_events(&terminal);
        assert!(!has_more);
        assert!(
            !events
                .iter()
                .any(|event| matches!(event, TerminalEvent::Wakeup))
        );
        assert_eq!(wakeups.load(Ordering::Relaxed), 0);

        terminal.feed_output(b"\x1b[?2026l");
        let (events, has_more) = drain_events(&terminal);
        assert!(!has_more);
        assert!(
            events
                .iter()
                .any(|event| matches!(event, TerminalEvent::Wakeup))
        );
        assert!(wakeups.load(Ordering::Relaxed) >= 1);
        assert!(visible_viewport_text(&terminal).contains("STAGED"));
    }

    #[test]
    fn generation_checked_damage_rejects_stale_partial_reads() {
        let terminal = PaneTerminal::new(
            TerminalSize {
                cols: 12,
                rows: 3,
                ..TerminalSize::default()
            },
            test_term_options(2000),
        );
        let _ = terminal.take_render_damage_snapshot();
        terminal.feed_output(b"damage");
        let snapshot = terminal.take_render_damage_snapshot();
        let TerminalDamageSnapshot::Partial(spans) = snapshot.damage else {
            panic!("expected partial damage");
        };

        let mut visited = 0;
        assert!(terminal.visit_viewport_ranges_at_generation(
            snapshot.generation,
            &spans,
            |_, _, _, _, _| visited += 1,
        ));
        assert!(visited > 0);

        terminal.feed_output(b" newer");
        assert!(!terminal.visit_viewport_ranges_at_generation(
            snapshot.generation,
            &spans,
            |_, _, _, _, _| {},
        ));
    }

    #[test]
    fn mouse_mode_detects_click_drag_motion_and_sgr_flags() {
        let terminal = PaneTerminal::new(TerminalSize::default(), test_term_options(2000));

        terminal.feed_output(b"\x1b[?1000h\x1b[?1006h");
        let click = terminal.mouse_mode();
        assert!(click.enabled);
        assert!(click.report_click);
        assert!(click.sgr_encoding);

        terminal.feed_output(b"\x1b[?1002h");
        assert!(terminal.mouse_mode().report_drag);
        terminal.feed_output(b"\x1b[?1003h");
        assert!(terminal.mouse_mode().report_motion);
    }

    #[test]
    fn tmux_link_lookup_is_owned_by_pane_terminal() {
        let terminal = PaneTerminal::new(
            TerminalSize {
                cols: 12,
                rows: 3,
                ..TerminalSize::default()
            },
            test_term_options(2000),
        );
        terminal.feed_output(
            b"\x1b]8;id=docs;https://example.com/docs\x1b\\docs\x1b]8;;\x1b\\ https://x.io",
        );

        let hyperlink = terminal.hyperlink_at(0, 1).expect("OSC 8 link");
        assert_eq!((hyperlink.start_col, hyperlink.end_col), (0, 3));
        assert_eq!(hyperlink.target, "https://example.com/docs");

        let detected = terminal.link_at(1, 3).expect("wrapped text link");
        assert_eq!(detected.target, "https://x.io");
    }

    #[test]
    fn feed_output_intercepts_kitty_graphics_for_tmux_panes() {
        let terminal = PaneTerminal::new(
            TerminalSize {
                cols: 20,
                rows: 10,
                cell_width: 10.0,
                cell_height: 20.0,
            },
            test_term_options(2000),
        );

        terminal.feed_output(b"\x1b_Ga=T,f=32,s=1,v=1,i=91,c=2,r=2;AQID/w==\x1b\\");

        let placements = terminal.kitty_graphics_placements();
        assert_eq!(placements.len(), 1);
        assert_eq!(placements[0].image_id, 91);
        assert_eq!(placements[0].display_cols, Some(2));
        assert_eq!(terminal.cursor_position(), (2, 2));
    }

    #[test]
    fn clear_screen_removes_kitty_graphics_from_tmux_panes() {
        let terminal = PaneTerminal::new(
            TerminalSize {
                cols: 20,
                rows: 10,
                cell_width: 10.0,
                cell_height: 20.0,
            },
            test_term_options(2000),
        );
        terminal.feed_output(b"\x1b_Ga=T,f=32,s=1,v=1,i=96,c=2,r=2,C=1;AQID/w==\x1b\\");
        assert_eq!(terminal.kitty_graphics_placements().len(), 1);

        terminal.feed_output(b"\x1b[H\x1b[2J");

        assert!(terminal.kitty_graphics_placements().is_empty());
    }

    #[test]
    fn kitty_cursor_advance_scrolls_tmux_pane_at_bottom() {
        let terminal = PaneTerminal::new(
            TerminalSize {
                cols: 20,
                rows: 4,
                cell_width: 10.0,
                cell_height: 20.0,
            },
            test_term_options(2000),
        );

        terminal.feed_output(b"\x1b[4;1H\x1b_Ga=T,f=32,s=1,v=1,i=92,c=2,r=3;AQID/w==\x1b\\");

        assert_eq!(terminal.scroll_state(), (0, 3));
        assert_eq!(terminal.cursor_position(), (2, 3));
        let placements = terminal.kitty_graphics_placements();
        assert_eq!(placements.len(), 1);
        assert_eq!(placements[0].viewport_row, 0);
    }

    #[test]
    fn kitty_cursor_advance_tracks_tmux_alternate_screen_scroll() {
        let terminal = PaneTerminal::new(
            TerminalSize {
                cols: 20,
                rows: 4,
                cell_width: 10.0,
                cell_height: 20.0,
            },
            test_term_options(2000),
        );

        terminal
            .feed_output(b"\x1b[?1049h\x1b[4;1H\x1b_Ga=T,f=32,s=1,v=1,i=93,c=2,r=3;AQID/w==\x1b\\");

        assert!(terminal.alternate_screen_mode());
        assert_eq!(terminal.scroll_state(), (0, 0));
        assert_eq!(terminal.cursor_position(), (2, 3));
        let placements = terminal.kitty_graphics_placements();
        assert_eq!(placements.len(), 1);
        assert_eq!(placements[0].viewport_row, 0);
    }

    #[test]
    fn ordinary_newlines_shift_tmux_alternate_screen_kitty_placement() {
        let terminal = PaneTerminal::new(
            TerminalSize {
                cols: 20,
                rows: 4,
                cell_width: 10.0,
                cell_height: 20.0,
            },
            test_term_options(2000),
        );
        terminal.feed_output(
            b"\x1b[?1049h\x1b[2;1H\x1b_Ga=T,f=32,s=1,v=1,i=95,c=2,r=1,C=1;AQID/w==\x1b\\",
        );

        terminal.feed_output(b"\x1b[4;1H\n");

        let placements = terminal.kitty_graphics_placements();
        assert_eq!(placements.len(), 1);
        assert_eq!(placements[0].viewport_row, 0);
    }

    #[test]
    fn kitty_cursor_advance_does_not_scroll_tmux_partial_region() {
        let terminal = PaneTerminal::new(
            TerminalSize {
                cols: 20,
                rows: 4,
                cell_width: 10.0,
                cell_height: 20.0,
            },
            test_term_options(2000),
        );

        terminal.feed_output(
            b"\x1b[2;3r\x1b[3;1H\x1b_Ga=T,f=32,s=1,v=1,i=94,c=2,r=3;AQID/w==\x1b\\\x1b[r",
        );

        assert_eq!(terminal.scroll_state(), (0, 0));
        assert_eq!(terminal.cursor_position(), (0, 0));
        let placements = terminal.kitty_graphics_placements();
        assert_eq!(placements.len(), 1);
        assert_eq!(placements[0].image_id, 94);
        assert_eq!(placements[0].viewport_row, 2);
    }
}
