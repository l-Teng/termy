use alacritty_terminal::{
    event::EventListener,
    grid::{Dimensions, Scroll},
    index::{Column, Line},
    term::{
        Config as TermConfig, LineDamageBounds, Term, TermDamage, TermMode, cell::Flags,
        color::Colors,
    },
    vte::ansi::{
        Color as AnsiColor, CursorShape, CursorStyle as AnsiCursorStyle, Handler, NamedColor,
        Rgb as AnsiRgb,
    },
};

use crate::{
    DetectedLink, DetectedViewportLink, TerminalColor, TerminalCursorState, TerminalCursorStyle,
    TerminalDamageSnapshot, TerminalDirtySpan, TerminalKeyboardMode, TerminalMouseMode,
    TerminalOptions, TerminalPalette, TerminalRenderCell, TerminalRenderColor,
    TerminalUnderlineStyle,
};

pub(crate) type EngineTerm<T> = Term<T>;
pub(crate) type EngineTermConfig = TermConfig;

pub(crate) fn term_config(options: TerminalOptions) -> EngineTermConfig {
    let shape = match options.default_cursor_style {
        TerminalCursorStyle::Line => CursorShape::Beam,
        TerminalCursorStyle::Block => CursorShape::Block,
    };
    TermConfig {
        scrolling_history: options
            .scrollback_history
            .min(crate::MAX_TERMINAL_SCROLLBACK_HISTORY),
        default_cursor_style: AnsiCursorStyle {
            shape,
            blinking: false,
        },
        kitty_keyboard: true,
        ..TermConfig::default()
    }
}

fn terminal_cursor_style_from_shape(shape: CursorShape) -> Option<TerminalCursorStyle> {
    match shape {
        CursorShape::Hidden => None,
        CursorShape::Block | CursorShape::HollowBlock => Some(TerminalCursorStyle::Block),
        CursorShape::Underline | CursorShape::Beam => Some(TerminalCursorStyle::Line),
    }
}

pub(crate) fn cursor_state<T: EventListener>(term: &EngineTerm<T>) -> Option<TerminalCursorState> {
    let cursor = term.renderable_content().cursor;
    let style = terminal_cursor_style_from_shape(cursor.shape)?;
    let row = usize::try_from(cursor.point.line.0).ok()?;
    Some(TerminalCursorState {
        col: cursor.point.column.0,
        row,
        style,
    })
}

pub(crate) fn cursor_position<T: EventListener>(term: &EngineTerm<T>) -> (usize, usize) {
    let cursor = term.renderable_content().cursor;
    let row = usize::try_from(cursor.point.line.0).ok().unwrap_or(0);
    (cursor.point.column.0, row)
}

pub(crate) fn mouse_mode(mode: TermMode) -> TerminalMouseMode {
    TerminalMouseMode {
        enabled: mode.intersects(TermMode::MOUSE_MODE) && !mode.contains(TermMode::VI),
        report_click: mode.contains(TermMode::MOUSE_REPORT_CLICK),
        report_drag: mode.contains(TermMode::MOUSE_DRAG),
        report_motion: mode.contains(TermMode::MOUSE_MOTION),
        sgr_encoding: mode.contains(TermMode::SGR_MOUSE),
        utf8_encoding: mode.contains(TermMode::UTF8_MOUSE),
    }
}

pub(crate) fn keyboard_mode(mode: TermMode) -> TerminalKeyboardMode {
    TerminalKeyboardMode::from_flags(
        mode.contains(TermMode::APP_CURSOR),
        mode.contains(TermMode::DISAMBIGUATE_ESC_CODES),
        mode.contains(TermMode::REPORT_EVENT_TYPES),
        mode.contains(TermMode::REPORT_ALTERNATE_KEYS),
        mode.contains(TermMode::REPORT_ALL_KEYS_AS_ESC),
        mode.contains(TermMode::REPORT_ASSOCIATED_TEXT),
    )
}

pub(crate) fn take_damage_snapshot<T: EventListener>(
    term: &mut EngineTerm<T>,
) -> TerminalDamageSnapshot {
    let rows = term.grid().screen_lines();
    let cols = term.grid().columns();
    let snapshot = match term.damage() {
        TermDamage::Full => TerminalDamageSnapshot::Full,
        TermDamage::Partial(damage_iter) => {
            let spans = damage_iter
                .filter_map(|damage| normalized_dirty_span(damage, rows, cols))
                .collect();
            TerminalDamageSnapshot::Partial(spans)
        }
    };
    term.reset_damage();
    snapshot
}

fn normalized_dirty_span(
    damage: LineDamageBounds,
    rows: usize,
    cols: usize,
) -> Option<TerminalDirtySpan> {
    if rows == 0 || cols == 0 || damage.line >= rows {
        return None;
    }
    let left_col = damage.left.saturating_sub(1).min(cols.saturating_sub(1));
    let right_col = damage.right.saturating_add(1).min(cols.saturating_sub(1));
    (left_col <= right_col).then_some(TerminalDirtySpan {
        row: damage.line,
        left_col,
        right_col,
    })
}

pub(crate) fn apply_term_options<T: EventListener>(
    term: &mut EngineTerm<T>,
    options: TerminalOptions,
) {
    let config = term_config(options);
    let shrinks_history = config.scrolling_history < term.grid().history_size();
    term.set_options(config);
    if shrinks_history {
        term.grid_mut().truncate();
    }
}

pub(crate) fn scroll_display<T: EventListener>(term: &mut EngineTerm<T>, delta_lines: i32) -> bool {
    if delta_lines == 0 {
        return false;
    }
    let old_offset = term.grid().display_offset();
    term.scroll_display(Scroll::Delta(delta_lines));
    term.grid().display_offset() != old_offset
}

pub(crate) fn scroll_to_bottom<T: EventListener>(term: &mut EngineTerm<T>) -> bool {
    if term.grid().display_offset() == 0 {
        return false;
    }
    term.scroll_display(Scroll::Bottom);
    true
}

pub(crate) fn clear_scrollback<T: EventListener>(term: &mut EngineTerm<T>) -> bool {
    if term.grid().history_size() == 0 && term.grid().display_offset() == 0 {
        return false;
    }
    term.grid_mut().clear_history();
    true
}

pub(crate) fn scroll_state<T: EventListener>(term: &EngineTerm<T>) -> (usize, usize) {
    let grid = term.grid();
    (grid.display_offset(), grid.history_size())
}

pub(crate) fn palette<T: EventListener>(term: &EngineTerm<T>, revision: u64) -> TerminalPalette {
    let colors = term.colors();
    TerminalPalette {
        indexed: std::array::from_fn(|index| colors[index].map(terminal_color)),
        foreground: colors[NamedColor::Foreground as usize].map(terminal_color),
        background: colors[NamedColor::Background as usize].map(terminal_color),
        cursor: colors[NamedColor::Cursor as usize].map(terminal_color),
        revision,
    }
}

pub(crate) fn palette_changed<T: EventListener>(
    term: &EngineTerm<T>,
    previous: &TerminalPalette,
) -> bool {
    let current = palette(term, previous.revision);
    current.indexed != previous.indexed
        || current.foreground != previous.foreground
        || current.background != previous.background
        || current.cursor != previous.cursor
}

fn terminal_color(rgb: AnsiRgb) -> TerminalColor {
    TerminalColor {
        r: rgb.r,
        g: rgb.g,
        b: rgb.b,
    }
}

pub(crate) fn ansi_rgb(color: TerminalColor) -> AnsiRgb {
    AnsiRgb {
        r: color.r,
        g: color.g,
        b: color.b,
    }
}

pub(crate) fn live_color(colors: &Colors, index: usize) -> Option<TerminalColor> {
    colors[index].map(terminal_color)
}

pub(crate) fn render_color(color: AnsiColor) -> TerminalRenderColor {
    match color {
        AnsiColor::Spec(rgb) => TerminalRenderColor::Rgb(terminal_color(rgb)),
        AnsiColor::Indexed(index) => TerminalRenderColor::Indexed(index),
        AnsiColor::Named(name) if (name as usize) < 16 => TerminalRenderColor::Indexed(name as u8),
        AnsiColor::Named(NamedColor::Foreground) => TerminalRenderColor::DefaultForeground,
        AnsiColor::Named(NamedColor::Background) => TerminalRenderColor::DefaultBackground,
        AnsiColor::Named(NamedColor::Cursor) => TerminalRenderColor::Cursor,
        AnsiColor::Named(NamedColor::BrightForeground) => TerminalRenderColor::BrightForeground,
        AnsiColor::Named(NamedColor::DimForeground) => TerminalRenderColor::DimForeground,
        AnsiColor::Named(name) => TerminalRenderColor::DimIndexed((name as usize - 259) as u8),
    }
}

fn underline_style(flags: Flags) -> TerminalUnderlineStyle {
    if flags.contains(Flags::DOUBLE_UNDERLINE) {
        TerminalUnderlineStyle::Double
    } else if flags.contains(Flags::UNDERCURL) {
        TerminalUnderlineStyle::Curly
    } else if flags.contains(Flags::DOTTED_UNDERLINE) {
        TerminalUnderlineStyle::Dotted
    } else if flags.contains(Flags::DASHED_UNDERLINE) {
        TerminalUnderlineStyle::Dashed
    } else if flags.contains(Flags::UNDERLINE) {
        TerminalUnderlineStyle::Single
    } else {
        TerminalUnderlineStyle::None
    }
}

pub(crate) fn render_cell(cell: &alacritty_terminal::term::cell::Cell) -> TerminalRenderCell {
    TerminalRenderCell {
        text: cell_text(cell),
        foreground: render_color(cell.fg),
        background: render_color(cell.bg),
        underline_color: cell.underline_color().map(render_color),
        bold: cell.flags.contains(Flags::BOLD),
        dim: cell.flags.contains(Flags::DIM),
        italic: cell.flags.contains(Flags::ITALIC),
        underline_style: underline_style(cell.flags),
        inverse: cell.flags.contains(Flags::INVERSE),
        hidden: cell.flags.contains(Flags::HIDDEN),
        strikethrough: cell.flags.contains(Flags::STRIKEOUT),
        hyperlink: cell.hyperlink().is_some(),
        wide_character_spacer: cell.flags.contains(Flags::WIDE_CHAR_SPACER),
        leading_wide_character_spacer: cell.flags.contains(Flags::LEADING_WIDE_CHAR_SPACER),
        line_wrapped: cell.flags.contains(Flags::WRAPLINE),
    }
}

fn cell_text(cell: &alacritty_terminal::term::cell::Cell) -> String {
    let combining_len = cell.zerowidth().map_or(0, <[char]>::len);
    let mut text = String::with_capacity(cell.c.len_utf8().saturating_add(combining_len * 4));
    text.push(cell.c);
    if let Some(combining) = cell.zerowidth() {
        text.extend(combining);
    }
    text
}

pub(crate) fn visible_render_cells<T: EventListener>(
    term: &EngineTerm<T>,
) -> Vec<TerminalRenderCell> {
    term.renderable_content()
        .display_iter
        .map(|indexed| render_cell(indexed.cell))
        .collect()
}

pub(crate) fn viewport_cells<T: EventListener>(
    term: &EngineTerm<T>,
) -> Vec<(usize, i32, usize, TerminalRenderCell)> {
    let content = term.renderable_content();
    let display_offset = content.display_offset;
    content
        .display_iter
        .map(|indexed| {
            (
                display_offset,
                indexed.point.line.0,
                indexed.point.column.0,
                render_cell(indexed.cell),
            )
        })
        .collect()
}

pub(crate) fn viewport_ranges<T: EventListener>(
    term: &EngineTerm<T>,
    spans: &[TerminalDirtySpan],
) -> Vec<(usize, usize, i32, usize, TerminalRenderCell)> {
    let grid = term.grid();
    let display_offset = grid.display_offset();
    let rows = grid.screen_lines();
    let cols = grid.columns();
    let mut cells = Vec::new();
    for span in spans {
        if span.row >= rows || span.left_col >= cols {
            continue;
        }
        let line = span.row as i32 - display_offset as i32;
        let right = span.right_col.min(cols.saturating_sub(1));
        for col in span.left_col..=right {
            cells.push((
                span.row,
                display_offset,
                line,
                col,
                render_cell(&grid[Line(line)][Column(col)]),
            ));
        }
    }
    cells
}

pub(crate) fn line_bounds<T: EventListener>(term: &EngineTerm<T>) -> (i32, i32) {
    let grid = term.grid();
    (
        -(grid.total_lines().saturating_sub(grid.screen_lines()) as i32),
        grid.screen_lines().saturating_sub(1) as i32,
    )
}

type RenderLineBounds = (i32, i32, usize);
type RenderLineCell = (i32, usize, TerminalRenderCell);

pub(crate) fn line_cells<T: EventListener>(
    term: &EngineTerm<T>,
    requested_first: i32,
    requested_last: i32,
) -> (RenderLineBounds, Vec<RenderLineCell>) {
    let grid = term.grid();
    let (first_line, last_line) = line_bounds(term);
    let columns = grid.columns();
    let mut cells = Vec::new();
    if requested_first <= requested_last {
        let first = requested_first.max(first_line);
        let last = requested_last.min(last_line);
        if first <= last {
            for line in first..=last {
                for col in 0..columns {
                    cells.push((line, col, render_cell(&grid[Line(line)][Column(col)])));
                }
            }
        }
    }
    ((first_line, last_line, columns), cells)
}

pub(crate) fn hyperlink_at<T: EventListener>(
    term: &EngineTerm<T>,
    row: usize,
    col: usize,
) -> Option<DetectedLink> {
    crate::links::hyperlink_at_viewport_cell(term, row, col)
}

pub(crate) fn link_at<T: EventListener>(
    term: &EngineTerm<T>,
    row: usize,
    col: usize,
) -> Option<DetectedViewportLink> {
    crate::links::link_at_viewport_cell(term, row, col)
}

pub(crate) fn bracketed_paste_mode<T: EventListener>(term: &EngineTerm<T>) -> bool {
    term.mode().contains(TermMode::BRACKETED_PASTE)
}

pub(crate) fn alternate_screen_mode<T: EventListener>(term: &EngineTerm<T>) -> bool {
    term.mode().contains(TermMode::ALT_SCREEN)
}

pub(crate) fn move_graphics_cursor<T: EventListener>(
    term: &mut EngineTerm<T>,
    cols: u32,
    rows: u32,
    full_screen_scroll_region: bool,
) -> usize {
    let cols = usize::try_from(cols)
        .unwrap_or(usize::MAX)
        .min(term.grid().columns());
    Handler::move_forward(term, cols);

    let rows = usize::try_from(rows)
        .unwrap_or(usize::MAX)
        .min(term.grid().screen_lines());
    if !full_screen_scroll_region {
        Handler::move_down(term, rows);
        return 0;
    }

    let history_before = term.grid().history_size();
    let mut scrolled_lines = 0usize;
    for _ in 0..rows {
        let line_before = term.grid().cursor.point.line;
        Handler::linefeed(term);
        scrolled_lines += usize::from(term.grid().cursor.point.line == line_before);
    }
    let history_growth = term.grid().history_size().saturating_sub(history_before);
    scrolled_lines.saturating_sub(history_growth)
}
