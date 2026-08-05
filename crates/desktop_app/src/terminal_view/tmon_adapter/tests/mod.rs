use super::*;
use alacritty_terminal::{
    grid::Dimensions,
    index::Line,
    term::cell::{Cell as AlacrittyCell, Flags},
    vte::ansi::{Color as AlacrittyColor, NamedColor, Rgb as AlacrittyRgb},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum RawColor {
    Default,
    Indexed(u8),
    Rgb(u8, u8, u8),
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct SemanticPalette {
    indexed: Vec<Option<(u8, u8, u8)>>,
    foreground: Option<(u8, u8, u8)>,
    background: Option<(u8, u8, u8)>,
    cursor: Option<(u8, u8, u8)>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct SemanticPlacement {
    image_id: u32,
    placement_id: u32,
    viewport_row: i32,
    col: usize,
    display_cols: Option<u32>,
    display_rows: Option<u32>,
    occupied_cols: u32,
    occupied_rows: u32,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct SemanticCell {
    character: char,
    combining: String,
    foreground: RawColor,
    background: RawColor,
    underline_color: Option<RawColor>,
    bold: bool,
    dim: bool,
    italic: bool,
    underline: tmon::UnderlineStyle,
    inverse: bool,
    hidden: bool,
    strikethrough: bool,
    hyperlink: bool,
    wide_spacer: bool,
    wrapped: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum SemanticEvent {
    Title(String),
    ResetTitle,
    Bell,
    Exit,
    ClipboardStore(String),
    ShellPromptStart,
    ShellCommandStart,
    ShellCommandExecuting,
    ShellCommandFinished(Option<i32>),
    Progress(ProgressState),
    WorkingDirectory(String),
}

fn semantic_event(event: TerminalEvent) -> Option<SemanticEvent> {
    Some(match event {
        TerminalEvent::Wakeup => return None,
        TerminalEvent::Title(title) => SemanticEvent::Title(title),
        TerminalEvent::ResetTitle => SemanticEvent::ResetTitle,
        TerminalEvent::Bell => SemanticEvent::Bell,
        TerminalEvent::Exit => SemanticEvent::Exit,
        TerminalEvent::ClipboardStore(text) => SemanticEvent::ClipboardStore(text),
        TerminalEvent::ShellPromptStart => SemanticEvent::ShellPromptStart,
        TerminalEvent::ShellCommandStart => SemanticEvent::ShellCommandStart,
        TerminalEvent::ShellCommandExecuting => SemanticEvent::ShellCommandExecuting,
        TerminalEvent::ShellCommandFinished(code) => SemanticEvent::ShellCommandFinished(code),
        TerminalEvent::Progress(progress) => SemanticEvent::Progress(progress),
        TerminalEvent::WorkingDirectory(path) => SemanticEvent::WorkingDirectory(path),
    })
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

fn native_underline_style(flags: Flags) -> tmon::UnderlineStyle {
    if flags.contains(Flags::DOUBLE_UNDERLINE) {
        tmon::UnderlineStyle::Double
    } else if flags.contains(Flags::UNDERCURL) {
        tmon::UnderlineStyle::Curly
    } else if flags.contains(Flags::DOTTED_UNDERLINE) {
        tmon::UnderlineStyle::Dotted
    } else if flags.contains(Flags::DASHED_UNDERLINE) {
        tmon::UnderlineStyle::Dashed
    } else if flags.contains(Flags::UNDERLINE) {
        tmon::UnderlineStyle::Single
    } else {
        tmon::UnderlineStyle::None
    }
}

fn native_palette(terminal: &termy_terminal_ui::Terminal) -> SemanticPalette {
    terminal.with_term(|term| {
        let colors = term.colors();
        let tuple = |color: alacritty_terminal::vte::ansi::Rgb| (color.r, color.g, color.b);
        SemanticPalette {
            indexed: (0..=255).map(|index| colors[index].map(tuple)).collect(),
            foreground: colors[NamedColor::Foreground as usize].map(tuple),
            background: colors[NamedColor::Background as usize].map(tuple),
            cursor: colors[NamedColor::Cursor as usize].map(tuple),
        }
    })
}

fn tmon_palette(terminal: &tmon::Terminal) -> SemanticPalette {
    let palette = terminal.palette();
    let tuple = |color: tmon::Rgb| (color.r, color.g, color.b);
    SemanticPalette {
        indexed: (0..=255)
            .map(|index| palette.indexed(index).map(tuple))
            .collect(),
        foreground: palette.foreground().map(tuple),
        background: palette.background().map(tuple),
        cursor: palette.cursor().map(tuple),
    }
}

fn semantic_placement(placement: &KittyGraphicsRenderPlacement) -> SemanticPlacement {
    SemanticPlacement {
        image_id: placement.image_id,
        placement_id: placement.placement_id,
        viewport_row: placement.viewport_row,
        col: placement.col,
        display_cols: placement.display_cols,
        display_rows: placement.display_rows,
        occupied_cols: placement.occupied_cols,
        occupied_rows: placement.occupied_rows,
    }
}

fn native_cell(cell: &AlacrittyCell) -> SemanticCell {
    SemanticCell {
        character: cell.c,
        combining: cell.zerowidth().into_iter().flatten().collect::<String>(),
        foreground: alacritty_color(cell.fg),
        background: alacritty_color(cell.bg),
        underline_color: cell.underline_color().map(alacritty_color),
        bold: cell.flags.contains(Flags::BOLD),
        dim: cell.flags.contains(Flags::DIM),
        italic: cell.flags.contains(Flags::ITALIC),
        underline: native_underline_style(cell.flags),
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
        underline_color: cell.underline_color.map(tmon_color),
        bold: cell.attributes.bold(),
        dim: cell.attributes.dim(),
        italic: cell.attributes.italic(),
        underline: cell.attributes.underline_style(),
        inverse: cell.attributes.inverse(),
        hidden: cell.attributes.hidden(),
        strikethrough: cell.attributes.strikethrough(),
        hyperlink: cell.has_hyperlink(),
        wide_spacer: cell.wide_spacer() || cell.leading_wide_spacer(),
        wrapped: cell.wrapped(),
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

#[test]
fn config_preserves_initial_query_colors_and_native_child_environment() {
    let colors = TerminalQueryColors {
        ansi: [AlacrittyRgb { r: 1, g: 2, b: 3 }; 16],
        foreground: AlacrittyRgb { r: 4, g: 5, b: 6 },
        background: AlacrittyRgb { r: 7, g: 8, b: 9 },
        cursor: Some(AlacrittyRgb {
            r: 10,
            g: 11,
            b: 12,
        }),
    };
    let runtime_config = TerminalRuntimeConfig {
        query_colors: colors,
        ..TerminalRuntimeConfig::default()
    };
    let converted = config(None, None, Some(&runtime_config), None).expect("Tmon config");

    assert_eq!(converted.query_colors, query_colors(colors));
    assert_eq!(
        converted
            .environment
            .into_iter()
            .collect::<std::collections::HashMap<_, _>>(),
        terminal_environment_overrides(None, &runtime_config)
    );
}

#[test]
fn config_and_options_match_the_native_scrollback_cap() {
    let terminal_options = TerminalOptions {
        scrollback_history: usize::MAX,
        ..TerminalOptions::default()
    };
    let runtime_config = TerminalRuntimeConfig {
        scrollback_history: usize::MAX,
        ..TerminalRuntimeConfig::default()
    };

    assert_eq!(
        options(terminal_options).scrollback_history,
        terminal_options.term_config().scrolling_history
    );
    assert_eq!(
        config(None, None, Some(&runtime_config), None)
            .expect("Tmon config")
            .scrollback_history,
        MAX_TERMINAL_SCROLLBACK_HISTORY
    );
}

#[test]
fn config_enables_native_parity_clipboard_queries() {
    let converted = config(None, None, None, None).expect("Tmon config");
    assert_eq!(converted.osc52, tmon::Osc52::CopyPaste);

    let terminal = tmon::Terminal::new_display(size(test_size(12, 4)), converted);
    terminal.feed_output(b"\x1b]52;c;?\x07");

    let (events, has_more) = terminal.drain_events();
    assert!(!has_more);
    let request = events
        .into_iter()
        .find_map(|event| match event {
            tmon::Event::ClipboardLoad(request) => Some(request),
            _ => None,
        })
        .expect("OSC 52 clipboard query should reach the desktop adapter");
    assert_eq!(request.target(), tmon::ClipboardTarget::Clipboard);
    assert_eq!(
        request.format_reply("payload"),
        b"\x1b]52;c;cGF5bG9hZA==\x07"
    );
}

fn assert_config_uses_core_launch(
    runtime_config: &TerminalRuntimeConfig,
    launch: Option<&TerminalLaunch>,
) {
    let expected = resolve_terminal_launch(runtime_config, launch).expect("core launch");
    let converted = config(None, None, Some(runtime_config), launch).expect("Tmon config");

    assert_eq!(converted.shell, None);
    assert_eq!(
        converted.launch,
        Some(tmon::Launch::Program {
            program: expected.program,
            args: expected.args,
        })
    );
}

#[test]
fn config_reuses_core_resolution_for_default_and_custom_shells() {
    assert_config_uses_core_launch(&TerminalRuntimeConfig::default(), None);
    assert_config_uses_core_launch(
        &TerminalRuntimeConfig {
            shell: Some("/opt/custom/bin/zsh".to_string()),
            ..TerminalRuntimeConfig::default()
        },
        None,
    );
}

#[test]
fn config_reuses_core_resolution_for_startup_commands_and_typed_programs() {
    let runtime_config = TerminalRuntimeConfig::default();
    let startup = TerminalLaunch::ShellCommand("printf adapter-parity".to_string());
    assert_config_uses_core_launch(&runtime_config, Some(&startup));

    let typed = TerminalLaunch::Program {
        program: "ssh".to_string(),
        args: vec!["--".to_string(), "example.com".to_string()],
    };
    assert_config_uses_core_launch(&runtime_config, Some(&typed));
}

#[test]
fn config_propagates_core_launch_validation_errors() {
    let invalid = TerminalLaunch::Program {
        program: "  ".to_string(),
        args: Vec::new(),
    };
    assert!(config(None, None, None, Some(&invalid)).is_err());
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

fn assert_damage_covers_changes(
    before: &[SemanticCell],
    after: &[SemanticCell],
    damage: &TerminalDamageSnapshot,
    cols: usize,
    rows: usize,
    context: &str,
) {
    assert_eq!(before.len(), after.len(), "{context}: viewport length");
    let TerminalDamageSnapshot::Partial(spans) = damage else {
        return;
    };
    for span in spans {
        assert!(span.row < rows, "{context}: damage row out of bounds");
        assert!(
            span.left_col <= span.right_col,
            "{context}: reversed damage span"
        );
        assert!(
            span.right_col < cols,
            "{context}: damage column out of bounds"
        );
    }
    for (index, (before, after)) in before.iter().zip(after).enumerate() {
        if before == after {
            continue;
        }
        let row = index / cols;
        let col = index % cols;
        assert!(
            spans
                .iter()
                .any(|span| { span.row == row && span.left_col <= col && col <= span.right_col }),
            "{context}: changed cell at row {row}, col {col} is outside damage {spans:?}"
        );
    }
}

fn assert_engines_match(context: &str, bytes: &[u8]) {
    let native_size = test_size(12, 4);
    let native = termy_terminal_ui::Terminal::new_display(native_size, None);
    let tmon = tmon::Terminal::new_display(size(native_size), tmon::Config::default());
    native.feed_output(bytes);
    tmon.feed_output(bytes);
    assert_terminal_states_match(&native, &tmon, context);
}

mod osc_events;
mod render_reflow;
mod vt_parity;
