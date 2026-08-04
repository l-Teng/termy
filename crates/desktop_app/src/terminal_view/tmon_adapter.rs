use std::collections::BTreeMap;

use termy_terminal_ui::{
    KittyGraphicsRenderPlacement, ProgressState, TabTitleShellIntegration, TerminalClipboardTarget,
    TerminalCursorState, TerminalCursorStyle, TerminalDamageSnapshot, TerminalDirtySpan,
    TerminalEvent, TerminalKeyboardMode, TerminalLaunch, TerminalMouseMode, TerminalOptions,
    TerminalQueryColors, TerminalRuntimeConfig, TerminalSize, resolve_launch_working_directory,
};

pub(super) fn size(size: TerminalSize) -> tmon::Size {
    tmon::Size {
        cols: size.cols,
        rows: size.rows,
        cell_width: size.cell_width,
        cell_height: size.cell_height,
    }
}

pub(super) fn terminal_size(size: tmon::Size) -> TerminalSize {
    TerminalSize {
        cols: size.cols,
        rows: size.rows,
        cell_width: size.cell_width,
        cell_height: size.cell_height,
    }
}

fn cursor_style(style: TerminalCursorStyle) -> tmon::CursorStyle {
    match style {
        TerminalCursorStyle::Line => tmon::CursorStyle::Line,
        TerminalCursorStyle::Block => tmon::CursorStyle::Block,
    }
}

fn terminal_cursor_style(style: tmon::CursorStyle) -> TerminalCursorStyle {
    match style {
        tmon::CursorStyle::Line => TerminalCursorStyle::Line,
        tmon::CursorStyle::Block => TerminalCursorStyle::Block,
    }
}

pub(super) fn cursor_state(state: tmon::CursorState) -> TerminalCursorState {
    TerminalCursorState {
        col: state.col,
        row: state.row,
        style: terminal_cursor_style(state.style),
    }
}

pub(super) fn options(options: TerminalOptions) -> tmon::TerminalOptions {
    tmon::TerminalOptions {
        scrollback_history: options.scrollback_history,
        default_cursor_style: cursor_style(options.default_cursor_style),
    }
}

pub(super) fn query_colors(colors: TerminalQueryColors) -> tmon::QueryColors {
    let rgb = |color: alacritty_terminal::vte::ansi::Rgb| tmon::Rgb {
        r: color.r,
        g: color.g,
        b: color.b,
    };
    tmon::QueryColors {
        ansi: colors.ansi.map(rgb),
        foreground: rgb(colors.foreground),
        background: rgb(colors.background),
        cursor: colors.cursor.map(rgb),
    }
}

pub(super) fn config(
    configured_working_dir: Option<&str>,
    shell_integration: Option<&TabTitleShellIntegration>,
    runtime_config: Option<&TerminalRuntimeConfig>,
    launch: Option<&TerminalLaunch>,
) -> tmon::Config {
    let runtime_config = runtime_config.cloned().unwrap_or_default();
    let launch = launch.map(|launch| match launch {
        TerminalLaunch::ShellCommand(command) => tmon::Launch::ShellCommand(command.clone()),
        TerminalLaunch::Program { program, args } => tmon::Launch::Program {
            program: program.clone(),
            args: args.clone(),
        },
    });

    tmon::Config {
        shell: runtime_config.shell.clone(),
        working_directory: resolve_launch_working_directory(
            configured_working_dir,
            runtime_config.working_dir_fallback,
        ),
        environment: environment(shell_integration, &runtime_config),
        launch,
        scrollback_history: runtime_config.scrollback_history,
        default_cursor_style: cursor_style(runtime_config.default_cursor_style),
        osc52: tmon::Osc52::OnlyCopy,
    }
}

fn environment(
    shell_integration: Option<&TabTitleShellIntegration>,
    runtime_config: &TerminalRuntimeConfig,
) -> Vec<(String, String)> {
    let mut environment = BTreeMap::new();
    if let Ok(path) = std::env::var("PATH")
        && !path.trim().is_empty()
    {
        environment.insert("PATH".to_string(), path);
    }

    let term = runtime_config.term.trim();
    environment.insert(
        "TERM".to_string(),
        if term.is_empty() {
            "xterm-256color".to_string()
        } else {
            term.to_string()
        },
    );
    if let Some(colorterm) = runtime_config
        .colorterm
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        environment.insert("COLORTERM".to_string(), colorterm.to_string());
    }

    environment.insert("TERM_PROGRAM".to_string(), "ghostty".to_string());
    environment.insert("TERM_PROGRAM_VERSION".to_string(), "1.2.0".to_string());
    environment.insert("TERMY_TERM_PROGRAM".to_string(), "termy".to_string());

    let shell_integration_enabled = shell_integration.is_some_and(|config| config.enabled);
    environment.insert(
        "TERMY_SHELL_INTEGRATION".to_string(),
        if shell_integration_enabled { "1" } else { "0" }.to_string(),
    );
    if shell_integration_enabled {
        let prefix = shell_integration
            .and_then(|config| {
                let prefix = config.explicit_prefix.trim();
                (!prefix.is_empty()).then_some(prefix)
            })
            .unwrap_or("termy:tab:");
        environment.insert("TERMY_TAB_TITLE_PREFIX".to_string(), prefix.to_string());
    }

    for (name, value) in &runtime_config.environment {
        let name = name.trim();
        if !name.is_empty() {
            environment.insert(name.to_string(), value.clone());
        }
    }
    environment.into_iter().collect()
}

pub(super) fn event(event: tmon::Event) -> Option<TerminalEvent> {
    Some(match event {
        tmon::Event::Wakeup => TerminalEvent::Wakeup,
        tmon::Event::Title(title) => TerminalEvent::Title(title),
        tmon::Event::ResetTitle => TerminalEvent::ResetTitle,
        tmon::Event::Bell => TerminalEvent::Bell,
        tmon::Event::Exit => TerminalEvent::Exit,
        tmon::Event::ClipboardStore(text) => TerminalEvent::ClipboardStore(text),
        tmon::Event::ClipboardLoad(_) => return None,
        tmon::Event::ShellPromptStart => TerminalEvent::ShellPromptStart,
        tmon::Event::ShellCommandStart => TerminalEvent::ShellCommandStart,
        tmon::Event::ShellCommandExecuting => TerminalEvent::ShellCommandExecuting,
        tmon::Event::ShellCommandFinished(exit_code) => {
            TerminalEvent::ShellCommandFinished(exit_code)
        }
        tmon::Event::Progress(progress) => TerminalEvent::Progress(match progress {
            tmon::Progress::Clear => ProgressState::Clear,
            tmon::Progress::InProgress(value) => ProgressState::InProgress(value),
            tmon::Progress::Error(value) => ProgressState::Error(value),
            tmon::Progress::Indeterminate => ProgressState::Indeterminate,
            tmon::Progress::Warning(value) => ProgressState::Warning(value),
        }),
        tmon::Event::WorkingDirectory(path) => TerminalEvent::WorkingDirectory(path),
    })
}

pub(super) fn clipboard_target(target: tmon::ClipboardTarget) -> TerminalClipboardTarget {
    match target {
        tmon::ClipboardTarget::Clipboard => TerminalClipboardTarget::Clipboard,
        tmon::ClipboardTarget::Selection => TerminalClipboardTarget::Selection,
    }
}

pub(super) fn damage(damage: tmon::DamageSnapshot) -> TerminalDamageSnapshot {
    match damage {
        tmon::DamageSnapshot::Full => TerminalDamageSnapshot::Full,
        tmon::DamageSnapshot::Partial(spans) => TerminalDamageSnapshot::Partial(
            spans
                .into_iter()
                .map(|span| TerminalDirtySpan {
                    row: span.row,
                    left_col: span.left_col,
                    right_col: span.right_col,
                })
                .collect(),
        ),
    }
}

pub(super) fn kitty_graphics_placement(
    placement: tmon::GraphicsRenderPlacement,
) -> KittyGraphicsRenderPlacement {
    KittyGraphicsRenderPlacement {
        placement_serial: placement.placement_serial,
        image_id: placement.image_id,
        placement_id: placement.placement_id,
        png: placement.png,
        image_width: placement.image_width,
        image_height: placement.image_height,
        image_generation: placement.image_generation,
        viewport_row: placement.viewport_row,
        col: placement.col,
        source_x: placement.source_x,
        source_y: placement.source_y,
        source_width: placement.source_width,
        source_height: placement.source_height,
        display_cols: placement.display_cols,
        display_rows: placement.display_rows,
        occupied_cols: placement.occupied_cols,
        occupied_rows: placement.occupied_rows,
        x_offset: placement.x_offset,
        y_offset: placement.y_offset,
        z_index: placement.z_index,
    }
}

pub(super) fn mouse_mode(mode: tmon::MouseMode) -> TerminalMouseMode {
    TerminalMouseMode {
        enabled: mode.enabled,
        report_click: mode.report_click,
        report_drag: mode.report_drag,
        report_motion: mode.report_motion,
        sgr_encoding: mode.sgr_encoding,
        utf8_encoding: mode.utf8_encoding,
    }
}

pub(super) fn keyboard_mode(mode: tmon::KeyboardMode) -> TerminalKeyboardMode {
    TerminalKeyboardMode::from_flags(
        mode.application_cursor_keys,
        mode.disambiguate_escape_codes,
        mode.report_event_types,
        mode.report_alternate_keys,
        mode.report_all_keys_as_esc,
        mode.report_associated_text,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use alacritty_terminal::{
        grid::Dimensions,
        index::Line,
        term::cell::{Cell as AlacrittyCell, Flags},
        vte::ansi::{Color as AlacrittyColor, NamedColor},
    };

    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    enum RawColor {
        Default,
        Indexed(u8),
        Rgb(u8, u8, u8),
    }

    #[derive(Clone, Debug, PartialEq, Eq)]
    struct SemanticCell {
        character: char,
        combining: String,
        foreground: RawColor,
        background: RawColor,
        bold: bool,
        dim: bool,
        italic: bool,
        underline: bool,
        inverse: bool,
        hidden: bool,
        strikethrough: bool,
        hyperlink: bool,
        wide_spacer: bool,
        wrapped: bool,
    }

    fn alacritty_color(color: AlacrittyColor) -> RawColor {
        match color {
            AlacrittyColor::Spec(rgb) => RawColor::Rgb(rgb.r, rgb.g, rgb.b),
            AlacrittyColor::Indexed(index) => RawColor::Indexed(index),
            AlacrittyColor::Named(name) if (name as usize) < 16 => RawColor::Indexed(name as u8),
            AlacrittyColor::Named(
                NamedColor::Foreground
                | NamedColor::Background
                | NamedColor::BrightForeground
                | NamedColor::DimForeground,
            ) => RawColor::Default,
            AlacrittyColor::Named(name) => panic!("unexpected named cell color {name:?}"),
        }
    }

    fn tmon_color(color: tmon::Color) -> RawColor {
        match color {
            tmon::Color::Default => RawColor::Default,
            tmon::Color::Indexed(index) => RawColor::Indexed(index),
            tmon::Color::Rgb { r, g, b } => RawColor::Rgb(r, g, b),
        }
    }

    fn native_cell(cell: &AlacrittyCell) -> SemanticCell {
        SemanticCell {
            character: cell.c,
            combining: cell.zerowidth().into_iter().flatten().collect::<String>(),
            foreground: alacritty_color(cell.fg),
            background: alacritty_color(cell.bg),
            bold: cell.flags.contains(Flags::BOLD),
            dim: cell.flags.contains(Flags::DIM),
            italic: cell.flags.contains(Flags::ITALIC),
            underline: cell.flags.intersects(Flags::ALL_UNDERLINES),
            inverse: cell.flags.contains(Flags::INVERSE),
            hidden: cell.flags.contains(Flags::HIDDEN),
            strikethrough: cell.flags.contains(Flags::STRIKEOUT),
            hyperlink: cell.hyperlink().is_some(),
            wide_spacer: cell
                .flags
                .intersects(Flags::WIDE_CHAR_SPACER | Flags::LEADING_WIDE_CHAR_SPACER),
            wrapped: cell.flags.contains(Flags::WRAPLINE),
        }
    }

    fn tmon_cell(cell: &tmon::Cell, combining: Option<tmon::Combining<'_>>) -> SemanticCell {
        SemanticCell {
            character: cell.character,
            combining: combining.map_or_else(String::new, tmon::Combining::to_owned_string),
            foreground: tmon_color(cell.foreground),
            background: tmon_color(cell.background),
            bold: cell.attributes.bold,
            dim: cell.attributes.dim,
            italic: cell.attributes.italic,
            underline: cell.attributes.underline,
            inverse: cell.attributes.inverse,
            hidden: cell.attributes.hidden,
            strikethrough: cell.attributes.strikethrough,
            hyperlink: cell.hyperlink_id.is_some(),
            wide_spacer: cell.wide_spacer || cell.leading_wide_spacer,
            wrapped: cell.wrapped,
        }
    }

    fn native_cells(terminal: &termy_terminal_ui::Terminal) -> Vec<SemanticCell> {
        terminal.with_term(|term| {
            term.renderable_content()
                .display_iter
                .map(|indexed| native_cell(indexed.cell))
                .collect()
        })
    }

    fn tmon_cells(terminal: &tmon::Terminal) -> Vec<SemanticCell> {
        let mut cells = Vec::new();
        terminal.for_each_viewport_cell(|_, _, _, cell, combining| {
            cells.push(tmon_cell(cell, combining));
        });
        cells
    }

    fn native_grid_lines(terminal: &termy_terminal_ui::Terminal) -> Vec<Vec<SemanticCell>> {
        terminal.with_term(|term| {
            let grid = term.grid();
            let first = -(grid.history_size() as i32);
            let last = grid.screen_lines() as i32;
            (first..last)
                .map(|line| (&grid[Line(line)]).into_iter().map(native_cell).collect())
                .collect()
        })
    }

    fn tmon_grid_lines(terminal: &tmon::Terminal) -> Vec<Vec<SemanticCell>> {
        let (first, last) = terminal.line_bounds();
        (first..=last)
            .map(|line| {
                let mut cells = Vec::new();
                assert!(terminal.for_each_line_cell(line, |_, cell, combining| {
                    cells.push(tmon_cell(cell, combining));
                }));
                cells
            })
            .collect()
    }

    fn test_size(cols: u16, rows: u16) -> TerminalSize {
        TerminalSize {
            cols,
            rows,
            cell_width: 9.0,
            cell_height: 18.0,
        }
    }

    fn assert_terminal_states_match(
        native: &termy_terminal_ui::Terminal,
        tmon: &tmon::Terminal,
        context: &str,
    ) {
        let tmon_grid_lines = tmon_grid_lines(tmon);
        let native_grid_lines = native_grid_lines(native);
        if tmon_grid_lines != native_grid_lines {
            let mismatch = tmon_grid_lines
                .iter()
                .zip(&native_grid_lines)
                .position(|(tmon, native)| tmon != native)
                .unwrap_or(tmon_grid_lines.len().min(native_grid_lines.len()));
            let mismatch_col = tmon_grid_lines
                .get(mismatch)
                .zip(native_grid_lines.get(mismatch))
                .and_then(|(tmon, native)| {
                    tmon.iter()
                        .zip(native)
                        .position(|(tmon, native)| tmon != native)
                });
            let first_line = tmon.line_bounds().0;
            panic!(
                "{context}: backing grids differ at line {}, col {:?}\n  tmon: {:?}\n native: {:?}",
                first_line + mismatch as i32,
                mismatch_col,
                mismatch_col.and_then(|col| tmon_grid_lines.get(mismatch)?.get(col)),
                mismatch_col.and_then(|col| native_grid_lines.get(mismatch)?.get(col)),
            );
        }
        let tmon_cells = tmon_cells(tmon);
        let native_cells = native_cells(native);
        if tmon_cells != native_cells {
            let mismatch = tmon_cells
                .iter()
                .zip(&native_cells)
                .position(|(tmon, native)| tmon != native)
                .unwrap_or(tmon_cells.len().min(native_cells.len()));
            let cols = usize::from(tmon.size().cols);
            let row = mismatch / cols;
            let row_start = row * cols;
            let row_end = row_start.saturating_add(cols);
            panic!(
                "{context}: cells differ at row {row}, col {}\n  tmon: {:?}\n native: {:?}",
                mismatch % cols,
                &tmon_cells[row_start.min(tmon_cells.len())..row_end.min(tmon_cells.len())],
                &native_cells[row_start.min(native_cells.len())..row_end.min(native_cells.len())],
            );
        }
        let tmon_cursor = tmon.cursor_state().map(cursor_state);
        let native_cursor = native.cursor_state();
        if tmon_cursor != native_cursor {
            let row = tmon_cursor.or(native_cursor).map_or(0, |cursor| cursor.row);
            let first_line = tmon.line_bounds().0;
            let row_index = (row as i32 - first_line).max(0) as usize;
            panic!(
                "{context}: cursor differs\n  tmon: {tmon_cursor:?}\n native: {native_cursor:?}\n    row: {:?}",
                tmon_grid_lines.get(row_index),
            );
        }
        assert_eq!(
            tmon.cursor_position(),
            native.cursor_position(),
            "{context}: cursor position"
        );
        assert_eq!(
            tmon.scroll_state(),
            native.scroll_state(),
            "{context}: scroll state"
        );
        assert_eq!(
            tmon.bracketed_paste_mode(),
            native.bracketed_paste_mode(),
            "{context}: bracketed paste"
        );
        assert_eq!(
            tmon.alternate_screen_mode(),
            native.alternate_screen_mode(),
            "{context}: alternate screen"
        );
        assert_eq!(
            mouse_mode(tmon.mouse_mode()),
            native.mouse_mode(),
            "{context}: mouse mode"
        );
        assert_eq!(
            keyboard_mode(tmon.keyboard_mode()),
            native.keyboard_mode(),
            "{context}: keyboard mode"
        );
    }

    fn assert_engines_match(context: &str, bytes: &[u8]) {
        let native_size = test_size(12, 4);
        let native = termy_terminal_ui::Terminal::new_display(native_size, None);
        let tmon = tmon::Terminal::new_display(size(native_size), tmon::Config::default());
        native.feed_output(bytes);
        tmon.feed_output(bytes);
        assert_terminal_states_match(&native, &tmon, context);
    }

    #[test]
    fn common_vt_fixtures_match_the_alacritty_engine() {
        for (name, fixture) in [
            (
                "basic cursor and rendition",
                b"plain\r\ntext\x1b[2;3H@\x1b[31;44;1;3;4;7;8;9mX\x1b[0m".as_slice(),
            ),
            (
                "wide text and scrollback",
                "ab界cd\r\nline2\r\nline3\r\nline4\r\nline5".as_bytes(),
            ),
            (
                "combining characters on narrow and wide cells",
                "e\u{301}\u{308} 界\u{301}\r\n12345678901e\u{301}".as_bytes(),
            ),
            (
                "editing characters",
                b"abcdefghijklM\x1b[2D\x1b[2@++\x1b[3P\x1b[4X\x1b[2K".as_slice(),
            ),
            (
                "scroll regions and lines",
                b"one\r\ntwo\r\nthree\x1b[2;4r\x1b[4;1H\n\x1bM\x1b[2L\x1b[1M".as_slice(),
            ),
            (
                "alternate screen and modes",
                b"main\x1b7\x1b[32mP\x1b[?1049hALT\x1b[?2004h\x1b[?1000h\x1b[?1006h\x1b[=5u\x1b[?1049lR\x1b8S".as_slice(),
            ),
            (
                "DEC graphics and alignment",
                b"\x1b(0lqk\x1b(B\x1b#8\x1b[Hok".as_slice(),
            ),
            (
                "DEC charset blank mapping",
                b"\x1b(0_`x\x1b(B".as_slice(),
            ),
            (
                "alternate screen isolates charset designation",
                b"\x1b[?1049h\x1b(0x\x1b[?1049lx".as_slice(),
            ),
        ] {
            assert_engines_match(name, fixture);
        }
    }

    #[test]
    fn extended_vt_fixtures_match_the_alacritty_engine() {
        for (name, fixture) in [
            (
                "origin addressing",
                b"111\r\n222\r\n333\r\n444\x1b[2;3r\x1b[?6h\x1b[1;2H@\x1b[2B#\x1b[?6l\x1b[4;4H$"
                    .as_slice(),
            ),
            (
                "insert and autowrap modes",
                b"abcdefghijkl\x1b[1;3H\x1b[4hXY\x1b[4l\x1b[?7l\x1b[1;12HZQ\x1b[?7hR".as_slice(),
            ),
            ("newline mode", b"abc\x1b[20h\nx\x1b[20l\ny".as_slice()),
            (
                "colored erasures",
                b"abcdefghijkl\r\nmnopqrstuvwx\x1b[45m\x1b[1;4H\x1b[K\x1b[2;3H\x1b[3X\x1b[0m"
                    .as_slice(),
            ),
            (
                "extended colors",
                b"\x1b[38;5;123;48;2;4;5;6mA\x1b[39;49mB\x1b[38:2::7:8:9;48:5:201mC".as_slice(),
            ),
            (
                "SGR cancel bold and malformed colors",
                b"\x1b[1;4mA\x1b[21mB\x1b[38;2;300;1;2mC\x1b[0m\x1b[48;5;999;3mD".as_slice(),
            ),
            (
                "custom tabs",
                b"a\tb\x1b[3g\r\x1b[5G\x1bH\r\tX\x1b[2IY\x1b[1Zz".as_slice(),
            ),
            (
                "cursor visibility and style",
                b"abc\x1b[?25l\x1b[6 q\x1b[2;3H\x1b[?25h".as_slice(),
            ),
            (
                "OSC cursor shape",
                b"abc\x1b]50;CursorShape=1\x07".as_slice(),
            ),
            (
                "DEC column mode side effects",
                b"one\r\ntwo\x1b[2;3r\x1b[?3hX\x1b[?3lY".as_slice(),
            ),
            (
                "wide repeat at the margin",
                "\x1b[31;44m界\r\x1b[12G\x1b[3b".as_bytes(),
            ),
            (
                "zero-width format and combining characters",
                "a\u{200d}b c\u{093f}d e\u{feff}f g\u{1e944}h".as_bytes(),
            ),
            (
                "C1 cursor and line controls",
                b"abc\x85x\x9b2;3H@\x84y\x8dz".as_slice(),
            ),
            (
                "cancel and substitute interrupted CSI",
                b"\x1b[31\x18mX\x1b[32\x1aY".as_slice(),
            ),
            (
                "escape interrupts OSC strings",
                b"abc\x1b]2;unfinished\x1b[2;3HZ".as_slice(),
            ),
            (
                "escape interrupts DCS strings",
                b"abc\x1bP$qm\x1b[2;3HZ".as_slice(),
            ),
            (
                "escape interrupts ignored strings",
                b"abc\x1b^ignored\x1b[2;3HZ".as_slice(),
            ),
            (
                "CSI with an unrelated intermediate is ignored",
                b"abc\x1b[31 mZ".as_slice(),
            ),
            (
                "CSI parameters after intermediates are ignored",
                b"abc\x1b[31 ;mZ".as_slice(),
            ),
            (
                "duplicate CSI private markers are ignored",
                b"abc\x1b[?25?lZ".as_slice(),
            ),
            (
                "multiple CSI intermediates are ignored",
                b"abc\x1b[5  qZ".as_slice(),
            ),
            (
                "C0 controls preserve the escape state",
                b"abc\x1b\x07[2;3HZ".as_slice(),
            ),
            (
                "C0 controls preserve escape intermediates",
                b"\x1b(\x070l".as_slice(),
            ),
            (
                "multiple escape intermediates are ignored",
                b"abc\x1b((0l".as_slice(),
            ),
            (
                "unknown charset designators preserve the active charset",
                b"\x1b(0q\x1b(Xq".as_slice(),
            ),
            (
                "OSC dispatches before a cancel control",
                b"\x1b]8;;https://example.com\x18X".as_slice(),
            ),
            (
                "backward tab honors column zero and an empty tab table",
                b"\x1b[6G\x1b[Z\x1b[3g\x1b[6G\x1b[Z".as_slice(),
            ),
            (
                "origin mode horizontal addressing follows the active region",
                b"\x1b[?6h\x1b[2;4r\x1b[9G".as_slice(),
            ),
        ] {
            assert_engines_match(name, fixture);
        }
    }

    #[test]
    fn deterministic_common_vt_traces_match_the_alacritty_engine() {
        let terminal_size = test_size(12, 4);
        let mut random = 0x4d59_5df4_d0f3_3173_u64;

        for trace in 0..32 {
            let native = termy_terminal_ui::Terminal::new_display(terminal_size, None);
            let tmon = tmon::Terminal::new_display(size(terminal_size), tmon::Config::default());
            let mut stream = Vec::new();

            for step in 0..96 {
                random ^= random << 13;
                random ^= random >> 7;
                random ^= random << 17;
                let value = random;
                let count = value as usize % 3 + 1;
                let row = value as usize % 4 + 1;
                let col = (value >> 8) as usize % 12 + 1;
                let action = match (value >> 16) % 51 {
                    0 => vec![b'a' + (value as u8 % 26)],
                    1 => b"text".to_vec(),
                    2 => b"\r".to_vec(),
                    3 => b"\n".to_vec(),
                    4 => b"\x08".to_vec(),
                    5 => b"\t".to_vec(),
                    6 => format!("\x1b[{count}A").into_bytes(),
                    7 => format!("\x1b[{count}B").into_bytes(),
                    8 => format!("\x1b[{count}C").into_bytes(),
                    9 => format!("\x1b[{count}D").into_bytes(),
                    10 => format!("\x1b[{row};{col}H").into_bytes(),
                    11 => format!("\x1b[{}K", value % 3).into_bytes(),
                    12 => format!("\x1b[{}J", value % 4).into_bytes(),
                    13 => format!("\x1b[{count}X").into_bytes(),
                    14 => format!("\x1b[{count}@").into_bytes(),
                    15 => format!("\x1b[{count}P").into_bytes(),
                    16 => format!("\x1b[{count}L").into_bytes(),
                    17 => format!("\x1b[{count}M").into_bytes(),
                    18 => format!("\x1b[{count}S").into_bytes(),
                    19 => format!("\x1b[{count}T").into_bytes(),
                    20 => [
                        b"\x1b[0m".as_slice(),
                        b"\x1b[1;3;4m",
                        b"\x1b[22;23;24m",
                        b"\x1b[7;8;9m",
                        b"\x1b[27;28;29m",
                        b"\x1b[31;44m",
                        b"\x1b[39;49m",
                        b"\x1b[38;5;123;48;2;4;5;6m",
                    ][value as usize % 8]
                        .to_vec(),
                    21 => if value & 1 == 0 { b"\x1b[s" } else { b"\x1b[u" }.to_vec(),
                    22 => {
                        let top = value as usize % 3 + 1;
                        let bottom = top + (value >> 8) as usize % (5 - top) + 1;
                        format!("\x1b[{top};{bottom}r").into_bytes()
                    }
                    23 => if value & 1 == 0 {
                        b"\x1b[?6h"
                    } else {
                        b"\x1b[?6l"
                    }
                    .to_vec(),
                    24 => if value & 1 == 0 {
                        b"\x1b[?7h"
                    } else {
                        b"\x1b[?7l"
                    }
                    .to_vec(),
                    25 => if value & 1 == 0 {
                        b"\x1b[4h"
                    } else {
                        b"\x1b[4l"
                    }
                    .to_vec(),
                    26 => format!("\x1b[{count}b").into_bytes(),
                    27 => "界".as_bytes().to_vec(),
                    28 => b"\x1bM".to_vec(),
                    29 => b"\x1bE".to_vec(),
                    30 => [b"\x1bH".as_slice(), b"\x1b[g", b"\x1b[3g"][value as usize % 3].to_vec(),
                    31 => if value & 1 == 0 {
                        b"\x1b[?1049h"
                    } else {
                        b"\x1b[?1049l"
                    }
                    .to_vec(),
                    32 => if value & 1 == 0 {
                        b"\x1b[?25h"
                    } else {
                        b"\x1b[?25l"
                    }
                    .to_vec(),
                    33 => b"\x1b(0lqk\x1b(B".to_vec(),
                    34 => b"\x1b7save\x1b8".to_vec(),
                    35 => format!("\x1b[{count}E").into_bytes(),
                    36 => format!("\x1b[{count}F").into_bytes(),
                    37 => format!("\x1b[{col}G").into_bytes(),
                    38 => format!("\x1b[{row}d").into_bytes(),
                    39 => if value & 1 == 0 {
                        format!("\x1b[{count}I")
                    } else {
                        format!("\x1b[{count}Z")
                    }
                    .into_bytes(),
                    40 => [b"\x1b[g".as_slice(), b"\x1b[3g"][value as usize % 2].to_vec(),
                    41 => [
                        b"\x1b[?1000h".as_slice(),
                        b"\x1b[?1002h",
                        b"\x1b[?1003h",
                        b"\x1b[?1000l\x1b[?1002l\x1b[?1003l",
                    ][value as usize % 4]
                        .to_vec(),
                    42 => b"\x1b[38:2::12:34:56;48:5:201;4:3mX\x1b[0m".to_vec(),
                    43 => b"\x1b]8;;https://example.com\x1b\\link\x1b]8;;\x1b\\".to_vec(),
                    44 => if value & 1 == 0 {
                        b"\x1b[6 q"
                    } else {
                        b"\x1b[2 q"
                    }
                    .to_vec(),
                    45 => if value & 1 == 0 {
                        b"\x1b[?3h"
                    } else {
                        b"\x1b[?3l"
                    }
                    .to_vec(),
                    46 => b"\x1b#8\x1b[2J".to_vec(),
                    47 => b"\x1b[31\x18mX\x1b[32\x1aY".to_vec(),
                    48 => if value & 1 == 0 {
                        b"\x1b[20h"
                    } else {
                        b"\x1b[20l"
                    }
                    .to_vec(),
                    49 => b"\x1b[31 mZ".to_vec(),
                    _ => "\u{301}".as_bytes().to_vec(),
                };

                native.feed_output(&action);
                tmon.feed_output(&action);
                stream.extend_from_slice(&action);
                assert_terminal_states_match(
                    &native,
                    &tmon,
                    &format!("trace {trace}, step {step}, action {action:?}, stream {stream:?}"),
                );
            }
        }
    }

    #[test]
    fn reflow_resize_matches_the_alacritty_engine() {
        let initial = test_size(8, 3);
        let mut native = termy_terminal_ui::Terminal::new_display(initial, None);
        let tmon = tmon::Terminal::new_display(size(initial), tmon::Config::default());
        let output = b"one two three four five six seven eight";
        native.feed_output(output);
        tmon.feed_output(output);
        assert_terminal_states_match(&native, &tmon, "before reflow");

        for resized in [test_size(5, 5), test_size(13, 2), test_size(9, 4)] {
            native.resize(resized);
            tmon.resize(size(resized));
            assert_terminal_states_match(
                &native,
                &tmon,
                &format!("reflow to {}x{}", resized.cols, resized.rows),
            );
        }
    }

    #[test]
    fn cursor_after_sparse_wide_reflow_matches_the_alacritty_engine() {
        let initial = test_size(8, 2);
        let mut native = termy_terminal_ui::Terminal::new_display(initial, None);
        let tmon = tmon::Terminal::new_display(size(initial), tmon::Config::default());
        let output = "  ws界a";
        native.feed_output(output.as_bytes());
        tmon.feed_output(output.as_bytes());
        assert_terminal_states_match(&native, &tmon, "before cursor reflow");

        let resized = test_size(5, 2);
        native.resize(resized);
        tmon.resize(size(resized));
        assert_terminal_states_match(&native, &tmon, "after cursor reflow");
    }

    #[test]
    fn cursor_at_wrapped_wide_boundary_matches_the_alacritty_engine() {
        let initial = test_size(9, 3);
        let mut native = termy_terminal_ui::Terminal::new_display(initial, None);
        let tmon = tmon::Terminal::new_display(size(initial), tmon::Config::default());
        let output = "\r\n  \u{301}m界α界 \u{8}\u{301}";
        native.feed_output(output.as_bytes());
        tmon.feed_output(output.as_bytes());
        assert_terminal_states_match(&native, &tmon, "before wide boundary reflow");

        let resized = test_size(4, 6);
        native.resize(resized);
        tmon.resize(size(resized));
        assert_terminal_states_match(&native, &tmon, "after wide boundary reflow");
    }

    #[test]
    fn growing_preserves_an_empty_soft_wrap_continuation() {
        let initial = test_size(4, 3);
        let mut native = termy_terminal_ui::Terminal::new_display(initial, None);
        let tmon = tmon::Terminal::new_display(size(initial), tmon::Config::default());
        let output = b"abcdefghX\x1b[2K";
        native.feed_output(output);
        tmon.feed_output(output);
        assert_terminal_states_match(&native, &tmon, "before empty continuation growth");

        let resized = test_size(8, 3);
        native.resize(resized);
        tmon.resize(size(resized));
        assert_terminal_states_match(&native, &tmon, "after empty continuation growth");
    }

    #[test]
    fn wide_margin_reflow_matches_the_alacritty_engine() {
        let initial = test_size(4, 3);
        let mut native = termy_terminal_ui::Terminal::new_display(initial, None);
        let tmon = tmon::Terminal::new_display(size(initial), tmon::Config::default());
        let output = "abc界x".as_bytes();
        native.feed_output(output);
        tmon.feed_output(output);
        assert_terminal_states_match(&native, &tmon, "before wide margin reflow");

        for resized in [test_size(5, 3), test_size(3, 4), test_size(7, 2)] {
            native.resize(resized);
            tmon.resize(size(resized));
            assert_terminal_states_match(
                &native,
                &tmon,
                &format!("wide margin reflow to {}x{}", resized.cols, resized.rows),
            );
        }
    }

    #[test]
    fn row_only_resize_matches_the_alacritty_engine() {
        let initial = test_size(8, 3);
        let mut native = termy_terminal_ui::Terminal::new_display(initial, None);
        let tmon = tmon::Terminal::new_display(size(initial), tmon::Config::default());
        native.feed_output(b"line-1\r\nline-2\r\nline-3\r\nline-4");
        tmon.feed_output(b"line-1\r\nline-2\r\nline-3\r\nline-4");
        assert_terminal_states_match(&native, &tmon, "before row resize");

        for resized in [test_size(8, 6), test_size(8, 2), test_size(8, 4)] {
            native.resize(resized);
            tmon.resize(size(resized));
            assert_terminal_states_match(
                &native,
                &tmon,
                &format!("row resize to {}x{}", resized.cols, resized.rows),
            );
        }
    }

    #[test]
    fn repeated_output_and_reflow_resizes_match_the_alacritty_engine() {
        let initial = test_size(10, 4);
        let mut native = termy_terminal_ui::Terminal::new_display(initial, None);
        let tmon = tmon::Terminal::new_display(size(initial), tmon::Config::default());
        let output = "one two three four\r\nfive 界 six seven\r\neight nine ten";
        native.feed_output(output.as_bytes());
        tmon.feed_output(output.as_bytes());
        assert_terminal_states_match(&native, &tmon, "before repeated resize trace");

        for (index, resized) in [
            test_size(6, 3),
            test_size(14, 6),
            test_size(8, 2),
            test_size(12, 5),
            test_size(5, 4),
            test_size(10, 4),
        ]
        .into_iter()
        .enumerate()
        {
            native.resize(resized);
            tmon.resize(size(resized));
            assert_terminal_states_match(
                &native,
                &tmon,
                &format!("repeated resize trace {index} after resize to {resized:?}"),
            );

            let output = format!("\r\nstep-{index} α界 tail");
            native.feed_output(output.as_bytes());
            tmon.feed_output(output.as_bytes());
            assert_terminal_states_match(
                &native,
                &tmon,
                &format!("repeated resize trace {index} after output"),
            );
        }
    }

    #[test]
    fn wide_character_after_a_sparse_wrapped_row_reflows_like_alacritty() {
        let initial = test_size(12, 2);
        let resized = test_size(5, 2);
        let mut native = termy_terminal_ui::Terminal::new_display(initial, None);
        let tmon = tmon::Terminal::new_display(size(initial), tmon::Config::default());
        let output = "word 界\x1b[12G α界 ";
        native.feed_output(output.as_bytes());
        tmon.feed_output(output.as_bytes());
        assert_terminal_states_match(&native, &tmon, "before sparse wide reflow");

        native.resize(resized);
        tmon.resize(size(resized));
        assert_terminal_states_match(&native, &tmon, "after sparse wide reflow");
    }

    #[test]
    fn deterministic_reflow_stress_matches_the_alacritty_engine() {
        let mut random = 0xa076_1d64_78bd_642f_u64;
        for trace in 0..16 {
            let initial = test_size(10, 4);
            let mut native = termy_terminal_ui::Terminal::new_display(initial, None);
            let tmon = tmon::Terminal::new_display(size(initial), tmon::Config::default());
            let mut operations = Vec::new();

            for step in 0..64 {
                random ^= random << 13;
                random ^= random >> 7;
                random ^= random << 17;
                let value = random;

                if value.is_multiple_of(7) {
                    let resized =
                        test_size(4 + (value >> 8) as u16 % 13, 2 + (value >> 24) as u16 % 6);
                    native.resize(resized);
                    tmon.resize(size(resized));
                    operations.push(format!("resize {resized:?}"));
                    assert_terminal_states_match(
                        &native,
                        &tmon,
                        &format!(
                            "reflow stress trace {trace}, step {step}, resize {resized:?}, operations {operations:?}"
                        ),
                    );
                    continue;
                }

                let output = match value % 12 {
                    0 => vec![b'a' + value as u8 % 26],
                    1 => b"word ".to_vec(),
                    2 => "界".as_bytes().to_vec(),
                    3 => "α界 ".as_bytes().to_vec(),
                    4 => b"\r".to_vec(),
                    5 => b"\n".to_vec(),
                    6 => b"\r\n".to_vec(),
                    7 => b"\x08".to_vec(),
                    8 => b"  ".to_vec(),
                    9 => b"\x1b[31;1mX\x1b[0m".to_vec(),
                    10 => b"tail".to_vec(),
                    _ => "\u{301}".as_bytes().to_vec(),
                };
                native.feed_output(&output);
                tmon.feed_output(&output);
                operations.push(format!("output {output:?}"));
                assert_terminal_states_match(
                    &native,
                    &tmon,
                    &format!(
                        "reflow stress trace {trace}, step {step}, output {output:?}, operations {operations:?}"
                    ),
                );
            }
        }
    }

    #[test]
    fn osc8_hyperlink_ranges_match_the_alacritty_engine() {
        let native_size = test_size(12, 4);
        let native = termy_terminal_ui::Terminal::new_display(native_size, None);
        let tmon = tmon::Terminal::new_display(size(native_size), tmon::Config::default());
        let output = b"x\x1b]8;id=same;https://example.com/docs\x1b\\linked\x1b]8;;\x1b\\ y";
        native.feed_output(output);
        tmon.feed_output(output);

        let native_link = native.hyperlink_at(0, 3).unwrap();
        let tmon_link = tmon.hyperlink_at(0, 3).unwrap();
        assert_eq!(
            (tmon_link.start_col, tmon_link.end_col, tmon_link.target),
            (
                native_link.start_col,
                native_link.end_col,
                native_link.target
            )
        );
    }
}
