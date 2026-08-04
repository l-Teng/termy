use std::collections::{HashMap, HashSet, VecDeque};
use std::num::NonZeroU32;

use crate::unicode_width::character_width;

const MAX_GRID_DIMENSION: u16 = 4096;
const MAX_SCROLLBACK_LINES: usize = 1_000_000;
const COMBINING_PRUNE_INTERVAL: u32 = 1 << 18;
const INLINE_COMBINING_BYTES: usize = 16;
const POOLED_COMBINING_BIT: u32 = 1 << 31;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Combining<'a> {
    Character(char),
    Text(&'a str),
}

impl Combining<'_> {
    pub fn append_to(self, output: &mut String) {
        match self {
            Self::Character(character) => output.push(character),
            Self::Text(text) => output.push_str(text),
        }
    }

    pub fn to_owned_string(self) -> String {
        match self {
            Self::Character(character) => character.to_string(),
            Self::Text(text) => text.to_string(),
        }
    }
}

#[derive(Clone, Debug)]
enum CombiningText {
    Inline {
        bytes: [u8; INLINE_COMBINING_BYTES],
        len: u8,
    },
    Heap(String),
}

impl CombiningText {
    fn from_char(character: char) -> Self {
        let mut bytes = [0; INLINE_COMBINING_BYTES];
        let len = character.encode_utf8(&mut bytes).len();
        Self::Inline {
            bytes,
            len: len as u8,
        }
    }

    fn appended(&self, character: char) -> Self {
        let mut encoded = [0; 4];
        let encoded = character.encode_utf8(&mut encoded).as_bytes();
        if let Self::Inline { bytes, len } = self {
            let len = usize::from(*len);
            if len + encoded.len() <= INLINE_COMBINING_BYTES {
                let mut combined = *bytes;
                combined[len..len + encoded.len()].copy_from_slice(encoded);
                return Self::Inline {
                    bytes: combined,
                    len: (len + encoded.len()) as u8,
                };
            }
        }

        let mut combined = String::with_capacity(self.as_str().len() + encoded.len());
        combined.push_str(self.as_str());
        combined.push(character);
        Self::Heap(combined)
    }

    fn as_str(&self) -> &str {
        match self {
            Self::Inline { bytes, len } => std::str::from_utf8(&bytes[..usize::from(*len)])
                .expect("inline combining text is encoded from valid characters"),
            Self::Heap(text) => text,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum Charset {
    #[default]
    Ascii,
    DecSpecial,
    Uk,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Color {
    #[default]
    Default,
    Indexed(u8),
    Rgb {
        r: u8,
        g: u8,
        b: u8,
    },
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Rgb {
    pub r: u8,
    pub g: u8,
    pub b: u8,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Palette {
    indexed: [Option<Rgb>; 256],
    foreground: Option<Rgb>,
    background: Option<Rgb>,
    cursor: Option<Rgb>,
}

impl Default for Palette {
    fn default() -> Self {
        Self {
            indexed: [None; 256],
            foreground: None,
            background: None,
            cursor: None,
        }
    }
}

impl Palette {
    pub fn indexed(&self, index: u8) -> Option<Rgb> {
        self.indexed[usize::from(index)]
    }

    pub fn foreground(&self) -> Option<Rgb> {
        self.foreground
    }

    pub fn background(&self) -> Option<Rgb> {
        self.background
    }

    pub fn cursor(&self) -> Option<Rgb> {
        self.cursor
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[repr(transparent)]
pub struct Attributes(u8);

impl Attributes {
    const BOLD: u8 = 1 << 0;
    const DIM: u8 = 1 << 1;
    const ITALIC: u8 = 1 << 2;
    const UNDERLINE: u8 = 1 << 3;
    const INVERSE: u8 = 1 << 4;
    const HIDDEN: u8 = 1 << 5;
    const STRIKETHROUGH: u8 = 1 << 6;

    const fn contains(self, flag: u8) -> bool {
        self.0 & flag != 0
    }

    const fn with_flag(mut self, flag: u8, enabled: bool) -> Self {
        if enabled {
            self.0 |= flag;
        } else {
            self.0 &= !flag;
        }
        self
    }

    fn set_flag(&mut self, flag: u8, enabled: bool) {
        *self = self.with_flag(flag, enabled);
    }

    pub const fn bold(self) -> bool {
        self.contains(Self::BOLD)
    }

    pub fn set_bold(&mut self, enabled: bool) {
        self.set_flag(Self::BOLD, enabled);
    }

    pub const fn with_bold(self, enabled: bool) -> Self {
        self.with_flag(Self::BOLD, enabled)
    }

    pub const fn dim(self) -> bool {
        self.contains(Self::DIM)
    }

    pub fn set_dim(&mut self, enabled: bool) {
        self.set_flag(Self::DIM, enabled);
    }

    pub const fn with_dim(self, enabled: bool) -> Self {
        self.with_flag(Self::DIM, enabled)
    }

    pub const fn italic(self) -> bool {
        self.contains(Self::ITALIC)
    }

    pub fn set_italic(&mut self, enabled: bool) {
        self.set_flag(Self::ITALIC, enabled);
    }

    pub const fn with_italic(self, enabled: bool) -> Self {
        self.with_flag(Self::ITALIC, enabled)
    }

    pub const fn underline(self) -> bool {
        self.contains(Self::UNDERLINE)
    }

    pub fn set_underline(&mut self, enabled: bool) {
        self.set_flag(Self::UNDERLINE, enabled);
    }

    pub const fn with_underline(self, enabled: bool) -> Self {
        self.with_flag(Self::UNDERLINE, enabled)
    }

    pub const fn inverse(self) -> bool {
        self.contains(Self::INVERSE)
    }

    pub fn set_inverse(&mut self, enabled: bool) {
        self.set_flag(Self::INVERSE, enabled);
    }

    pub const fn with_inverse(self, enabled: bool) -> Self {
        self.with_flag(Self::INVERSE, enabled)
    }

    pub const fn hidden(self) -> bool {
        self.contains(Self::HIDDEN)
    }

    pub fn set_hidden(&mut self, enabled: bool) {
        self.set_flag(Self::HIDDEN, enabled);
    }

    pub const fn with_hidden(self, enabled: bool) -> Self {
        self.with_flag(Self::HIDDEN, enabled)
    }

    pub const fn strikethrough(self) -> bool {
        self.contains(Self::STRIKETHROUGH)
    }

    pub fn set_strikethrough(&mut self, enabled: bool) {
        self.set_flag(Self::STRIKETHROUGH, enabled);
    }

    pub const fn with_strikethrough(self, enabled: bool) -> Self {
        self.with_flag(Self::STRIKETHROUGH, enabled)
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[repr(transparent)]
struct CellState(u8);

impl CellState {
    const PROTECTED: u8 = 1 << 0;
    const WIDE_SPACER: u8 = 1 << 1;
    const LEADING_WIDE_SPACER: u8 = 1 << 2;
    const WRAPPED: u8 = 1 << 3;

    const fn contains(self, flag: u8) -> bool {
        self.0 & flag != 0
    }

    fn set(&mut self, flag: u8, enabled: bool) {
        if enabled {
            self.0 |= flag;
        } else {
            self.0 &= !flag;
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Cell {
    pub character: char,
    pub foreground: Color,
    pub background: Color,
    pub attributes: Attributes,
    state: CellState,
    pub hyperlink_id: Option<NonZeroU32>,
    #[doc(hidden)]
    pub combining_id: Option<NonZeroU32>,
}

impl Default for Cell {
    fn default() -> Self {
        Self {
            character: ' ',
            foreground: Color::Default,
            background: Color::Default,
            attributes: Attributes::default(),
            state: CellState::default(),
            hyperlink_id: None,
            combining_id: None,
        }
    }
}

impl Cell {
    pub const fn protected(&self) -> bool {
        self.state.contains(CellState::PROTECTED)
    }

    pub const fn wide_spacer(&self) -> bool {
        self.state.contains(CellState::WIDE_SPACER)
    }

    pub const fn leading_wide_spacer(&self) -> bool {
        self.state.contains(CellState::LEADING_WIDE_SPACER)
    }

    pub const fn wrapped(&self) -> bool {
        self.state.contains(CellState::WRAPPED)
    }

    pub fn set_protected(&mut self, enabled: bool) {
        self.state.set(CellState::PROTECTED, enabled);
    }

    pub fn set_wide_spacer(&mut self, enabled: bool) {
        self.state.set(CellState::WIDE_SPACER, enabled);
    }

    pub fn set_leading_wide_spacer(&mut self, enabled: bool) {
        self.state.set(CellState::LEADING_WIDE_SPACER, enabled);
    }

    pub fn set_wrapped(&mut self, enabled: bool) {
        self.state.set(CellState::WRAPPED, enabled);
    }

    fn take_leading_wide_spacer(&mut self) -> bool {
        let leading_wide_spacer = self.leading_wide_spacer();
        self.set_leading_wide_spacer(false);
        leading_wide_spacer
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum CursorStyle {
    Line,
    #[default]
    Block,
}

const fn cursor_shape_status(style: CursorStyle) -> u16 {
    match style {
        CursorStyle::Line => 6,
        CursorStyle::Block => 2,
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CursorState {
    pub col: usize,
    pub row: usize,
    pub style: CursorStyle,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct MouseMode {
    pub enabled: bool,
    pub report_click: bool,
    pub report_drag: bool,
    pub report_motion: bool,
    pub sgr_encoding: bool,
    pub utf8_encoding: bool,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct KeyboardMode {
    pub application_cursor_keys: bool,
    pub application_keypad: bool,
    pub disambiguate_escape_codes: bool,
    pub report_event_types: bool,
    pub report_alternate_keys: bool,
    pub report_all_keys_as_esc: bool,
    pub report_associated_text: bool,
}

impl KeyboardMode {
    fn kitty_flags(self) -> u16 {
        u16::from(self.disambiguate_escape_codes)
            | (u16::from(self.report_event_types) << 1)
            | (u16::from(self.report_alternate_keys) << 2)
            | (u16::from(self.report_all_keys_as_esc) << 3)
            | (u16::from(self.report_associated_text) << 4)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DirtySpan {
    pub row: usize,
    pub left_col: usize,
    pub right_col: usize,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DamageSnapshot {
    Full,
    Partial(Vec<DirtySpan>),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Snapshot {
    pub cols: u16,
    pub rows: u16,
    pub cells: Vec<Cell>,
    pub cursor: Option<CursorState>,
    pub display_offset: usize,
    pub history_size: usize,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Hyperlink {
    pub start_col: usize,
    pub end_col: usize,
    pub target: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum GridEffect {
    ScrollUp {
        alternate: bool,
        top: usize,
        bottom: usize,
        count: usize,
        recorded_history: bool,
        history_before: usize,
        history_after: usize,
    },
    ScrollDown {
        alternate: bool,
        top: usize,
        bottom: usize,
        count: usize,
        history_size: usize,
    },
    EnteredAlternate,
    ClearViewport {
        alternate: bool,
        history_size: usize,
        rows: usize,
        cols: usize,
    },
    RebaseHistory {
        dropped: usize,
    },
    Reflow {
        alternate: bool,
    },
    Reset,
}

#[derive(Clone, Copy, Debug, Default)]
struct Pen {
    foreground: Color,
    background: Color,
    attributes: Attributes,
    protected: bool,
    hyperlink_id: Option<NonZeroU32>,
}

impl Pen {
    fn cell(self, character: char) -> Cell {
        let mut cell = Cell {
            character,
            foreground: self.foreground,
            background: self.background,
            attributes: self.attributes,
            state: CellState::default(),
            hyperlink_id: self.hyperlink_id,
            combining_id: None,
        };
        cell.set_protected(self.protected);
        cell
    }

    fn blank(self) -> Cell {
        Cell {
            background: self.background,
            ..Cell::default()
        }
    }
}

#[derive(Clone, Debug)]
struct Screen {
    cols: usize,
    rows: usize,
    cells: Vec<Vec<Cell>>,
    cursor_col: usize,
    cursor_row: usize,
    saved_cursor_col: usize,
    saved_cursor_row: usize,
    pen: Pen,
    saved_pen: Pen,
    charsets: [Charset; 4],
    saved_charsets: [Charset; 4],
    wrap_pending: bool,
    saved_wrap_pending: bool,
    scroll_top: usize,
    scroll_bottom: usize,
}

impl Screen {
    fn new(cols: usize, rows: usize) -> Self {
        Self {
            cols,
            rows,
            cells: (0..rows).map(|_| vec![Cell::default(); cols]).collect(),
            cursor_col: 0,
            cursor_row: 0,
            saved_cursor_col: 0,
            saved_cursor_row: 0,
            pen: Pen::default(),
            saved_pen: Pen::default(),
            charsets: [Charset::Ascii; 4],
            saved_charsets: [Charset::Ascii; 4],
            wrap_pending: false,
            saved_wrap_pending: false,
            scroll_top: 0,
            scroll_bottom: rows.saturating_sub(1),
        }
    }

    fn row(&self, row: usize) -> &[Cell] {
        &self.cells[row]
    }

    fn row_mut(&mut self, row: usize) -> &mut [Cell] {
        &mut self.cells[row]
    }

    fn fill(&mut self, cell: Cell) {
        for row in &mut self.cells {
            row.fill(cell);
        }
    }

    fn reset(&mut self) {
        self.fill(Cell::default());
        self.cursor_col = 0;
        self.cursor_row = 0;
        self.saved_cursor_col = 0;
        self.saved_cursor_row = 0;
        self.pen = Pen::default();
        self.saved_pen = Pen::default();
        self.charsets = [Charset::Ascii; 4];
        self.saved_charsets = [Charset::Ascii; 4];
        self.wrap_pending = false;
        self.saved_wrap_pending = false;
        self.scroll_top = 0;
        self.scroll_bottom = self.rows.saturating_sub(1);
    }

    fn save_cursor(&mut self) {
        self.saved_cursor_col = self.cursor_col;
        self.saved_cursor_row = self.cursor_row;
        self.saved_pen = self.pen;
        self.saved_charsets = self.charsets;
        self.saved_wrap_pending = self.wrap_pending;
    }

    fn restore_cursor(&mut self) {
        self.cursor_col = self.saved_cursor_col.min(self.cols.saturating_sub(1));
        self.cursor_row = self.saved_cursor_row.min(self.rows.saturating_sub(1));
        self.pen = self.saved_pen;
        self.charsets = self.saved_charsets;
        self.wrap_pending = self.saved_wrap_pending;
    }

    fn resize(&mut self, cols: usize, rows: usize, bottom_anchor: bool) -> Vec<Vec<Cell>> {
        let old_cols = self.cols;
        let old_rows = self.rows;
        let old_cursor_row = self.cursor_row;
        let mut removed = Vec::new();

        if bottom_anchor && rows < old_rows {
            for row in 0..old_rows - rows {
                removed.push(self.row(row).to_vec());
            }
        }

        let mut cells = (0..rows)
            .map(|_| vec![Cell::default(); cols])
            .collect::<Vec<_>>();
        let copy_cols = old_cols.min(cols);
        let copy_rows = old_rows.min(rows);
        let required_scrolling = if !bottom_anchor && rows < old_rows {
            old_cursor_row.saturating_add(1).saturating_sub(rows)
        } else {
            0
        };
        let (source_row, target_row) = if bottom_anchor {
            (
                old_rows.saturating_sub(copy_rows),
                rows.saturating_sub(copy_rows),
            )
        } else {
            (required_scrolling, 0)
        };

        for offset in 0..copy_rows {
            cells[target_row + offset][..copy_cols]
                .copy_from_slice(&self.cells[source_row + offset][..copy_cols]);
        }

        self.cols = cols;
        self.rows = rows;
        self.cells = cells;
        self.cursor_col = self.cursor_col.min(cols.saturating_sub(1));
        self.cursor_row = if bottom_anchor {
            if rows >= old_rows {
                old_cursor_row.saturating_add(rows - old_rows)
            } else {
                old_cursor_row.saturating_sub(old_rows - rows)
            }
        } else {
            old_cursor_row.min(rows.saturating_sub(1))
        }
        .min(rows.saturating_sub(1));
        self.saved_cursor_col = self.saved_cursor_col.min(cols.saturating_sub(1));
        self.saved_cursor_row = self.saved_cursor_row.min(rows.saturating_sub(1));
        self.scroll_top = 0;
        self.scroll_bottom = rows.saturating_sub(1);
        self.wrap_pending = false;
        removed
    }
}

#[derive(Debug)]
struct Damage {
    full: bool,
    rows: Vec<Option<(usize, usize)>>,
}

impl Damage {
    fn new(rows: usize) -> Self {
        Self {
            full: true,
            rows: vec![None; rows],
        }
    }

    fn mark(&mut self, row: usize, left: usize, right: usize) {
        if self.full || row >= self.rows.len() || left > right {
            return;
        }
        let slot = &mut self.rows[row];
        *slot = Some(slot.map_or((left, right), |(old_left, old_right)| {
            (old_left.min(left), old_right.max(right))
        }));
    }

    fn mark_full(&mut self) {
        self.full = true;
    }

    fn resize(&mut self, rows: usize) {
        self.rows.resize(rows, None);
        self.mark_full();
    }

    fn take(&mut self) -> DamageSnapshot {
        if self.full {
            self.full = false;
            self.rows.fill(None);
            return DamageSnapshot::Full;
        }

        let mut spans = Vec::new();
        for (row, bounds) in self.rows.iter_mut().enumerate() {
            if let Some((left_col, right_col)) = bounds.take() {
                spans.push(DirtySpan {
                    row,
                    left_col,
                    right_col,
                });
            }
        }
        DamageSnapshot::Partial(spans)
    }
}

#[derive(Debug)]
pub(crate) struct Grid {
    primary: Screen,
    alternate: Screen,
    alternate_active: bool,
    history: VecDeque<Vec<Cell>>,
    history_limit: usize,
    display_offset: usize,
    damage: Damage,
    cursor_visible: bool,
    cursor_style: CursorStyle,
    default_cursor_style: CursorStyle,
    cursor_style_explicit: bool,
    cursor_shape_status: u16,
    auto_wrap: bool,
    insert_mode: bool,
    origin_mode: bool,
    line_feed_new_line: bool,
    bracketed_paste: bool,
    cursor_blinking: bool,
    focus_reporting: bool,
    alternate_scroll: bool,
    urgency_hints: bool,
    mouse_mode: MouseMode,
    keyboard_mode: KeyboardMode,
    kitty_keyboard_stack: Vec<u16>,
    inactive_kitty_keyboard_stack: Vec<u16>,
    tab_stops: Vec<bool>,
    cell_width: f32,
    cell_height: f32,
    palette: Palette,
    hyperlinks: HashMap<u32, String>,
    next_hyperlink_id: u32,
    combining: HashMap<NonZeroU32, CombiningText>,
    next_combining_id: u32,
    effects: VecDeque<GridEffect>,
}

impl Grid {
    pub(crate) fn new(
        cols: u16,
        rows: u16,
        history_limit: usize,
        default_cursor_style: CursorStyle,
    ) -> Self {
        let cols = usize::from(cols.clamp(1, MAX_GRID_DIMENSION));
        let rows = usize::from(rows.clamp(1, MAX_GRID_DIMENSION));
        Self {
            primary: Screen::new(cols, rows),
            alternate: Screen::new(cols, rows),
            alternate_active: false,
            history: VecDeque::new(),
            history_limit: history_limit.min(MAX_SCROLLBACK_LINES),
            display_offset: 0,
            damage: Damage::new(rows),
            cursor_visible: true,
            cursor_style: default_cursor_style,
            default_cursor_style,
            cursor_style_explicit: false,
            cursor_shape_status: cursor_shape_status(default_cursor_style),
            auto_wrap: true,
            insert_mode: false,
            origin_mode: false,
            line_feed_new_line: false,
            bracketed_paste: false,
            cursor_blinking: false,
            focus_reporting: false,
            alternate_scroll: true,
            urgency_hints: true,
            mouse_mode: MouseMode::default(),
            keyboard_mode: KeyboardMode::default(),
            kitty_keyboard_stack: Vec::with_capacity(8),
            inactive_kitty_keyboard_stack: Vec::with_capacity(8),
            tab_stops: default_tab_stops(cols),
            cell_width: 1.0,
            cell_height: 1.0,
            palette: Palette::default(),
            hyperlinks: HashMap::new(),
            next_hyperlink_id: 1,
            combining: HashMap::new(),
            next_combining_id: 1,
            effects: VecDeque::with_capacity(8),
        }
    }

    fn active(&self) -> &Screen {
        if self.alternate_active {
            &self.alternate
        } else {
            &self.primary
        }
    }

    fn active_mut(&mut self) -> &mut Screen {
        if self.alternate_active {
            &mut self.alternate
        } else {
            &mut self.primary
        }
    }

    fn pen(&self) -> Pen {
        self.active().pen
    }

    fn pen_mut(&mut self) -> &mut Pen {
        &mut self.active_mut().pen
    }

    pub(crate) fn charset(&self, index: usize) -> Charset {
        self.active().charsets[index.min(3)]
    }

    pub(crate) fn set_charset(&mut self, index: usize, charset: Charset) {
        if let Some(target) = self.active_mut().charsets.get_mut(index) {
            *target = charset;
        }
    }

    pub(crate) fn cols(&self) -> usize {
        self.active().cols
    }

    pub(crate) fn rows(&self) -> usize {
        self.active().rows
    }

    pub(crate) fn cursor_position(&self) -> (usize, usize) {
        let screen = self.active();
        (screen.cursor_col, screen.cursor_row)
    }

    pub(crate) fn render_cursor_position(&self) -> (usize, usize) {
        let screen = self.active();
        let col = if screen.row(screen.cursor_row)[screen.cursor_col].wide_spacer() {
            screen.cursor_col.saturating_sub(1)
        } else {
            screen.cursor_col
        };
        (col, screen.cursor_row)
    }

    pub(crate) fn cursor_state(&self) -> Option<CursorState> {
        self.cursor_visible.then(|| {
            let (col, row) = self.render_cursor_position();
            CursorState {
                col,
                row,
                style: self.cursor_style,
            }
        })
    }

    pub(crate) fn display_offset(&self) -> usize {
        if self.alternate_active {
            0
        } else {
            self.display_offset
        }
    }

    pub(crate) fn history_size(&self) -> usize {
        if self.alternate_active {
            0
        } else {
            self.history.len()
        }
    }

    pub(crate) fn bracketed_paste_mode(&self) -> bool {
        self.bracketed_paste
    }

    pub(crate) fn mouse_mode(&self) -> MouseMode {
        self.mouse_mode
    }

    pub(crate) fn keyboard_mode(&self) -> KeyboardMode {
        self.keyboard_mode
    }

    pub(crate) fn alternate_screen_mode(&self) -> bool {
        self.alternate_active
    }

    pub(crate) fn carriage_return(&mut self) {
        let screen = self.active_mut();
        screen.cursor_col = 0;
        screen.wrap_pending = false;
    }

    pub(crate) fn backspace(&mut self) {
        let screen = self.active_mut();
        if screen.cursor_col > 0 {
            screen.cursor_col -= 1;
            screen.wrap_pending = false;
        }
    }

    pub(crate) fn tab(&mut self) {
        if self.active().wrap_pending {
            if !self.auto_wrap {
                return;
            }
            let (row, col) = {
                let screen = self.active();
                (screen.cursor_row, screen.cursor_col)
            };
            self.active_mut().row_mut(row)[col].set_wrapped(true);
            self.damage.mark(row, col, col);
            self.carriage_return();
            self.line_feed();
            return;
        }

        let (row, col) = {
            let screen = self.active();
            (screen.cursor_row, screen.cursor_col)
        };
        if self.active().row(row)[col].character == ' ' {
            self.active_mut().row_mut(row)[col].character = '\t';
            self.damage.mark(row, col, col);
        }
        self.move_forward_tabs(1);
    }

    pub(crate) fn move_forward_tabs(&mut self, count: usize) {
        let mut col = self.active().cursor_col;
        for _ in 0..count {
            col = self
                .tab_stops
                .iter()
                .enumerate()
                .skip(col.saturating_add(1))
                .find_map(|(index, stop)| stop.then_some(index))
                .unwrap_or_else(|| self.cols().saturating_sub(1));
        }
        self.active_mut().cursor_col = col;
    }

    pub(crate) fn move_backward_tabs(&mut self, count: usize) {
        let mut col = self.active().cursor_col;
        for _ in 0..count {
            col = self
                .tab_stops
                .iter()
                .enumerate()
                .take(col)
                .rev()
                .find_map(|(index, stop)| stop.then_some(index))
                .unwrap_or(col);
        }
        self.active_mut().cursor_col = col;
    }

    pub(crate) fn set_tab_stop(&mut self) {
        let col = self.active().cursor_col;
        if let Some(stop) = self.tab_stops.get_mut(col) {
            *stop = true;
        }
    }

    pub(crate) fn clear_tab_stop(&mut self, mode: u16) {
        match mode {
            0 => {
                let col = self.active().cursor_col;
                if let Some(stop) = self.tab_stops.get_mut(col) {
                    *stop = false;
                }
            }
            3 => self.tab_stops.fill(false),
            _ => {}
        }
    }

    pub(crate) fn reset_tab_stops(&mut self) {
        self.tab_stops = default_tab_stops(self.cols());
    }

    pub(crate) fn line_feed(&mut self) {
        let (cursor_row, scroll_bottom) = {
            let screen = self.active();
            (screen.cursor_row, screen.scroll_bottom)
        };
        if cursor_row == scroll_bottom {
            self.scroll_up(1);
        } else {
            let screen = self.active_mut();
            screen.cursor_row = (screen.cursor_row + 1).min(screen.rows.saturating_sub(1));
        }
    }

    pub(crate) fn next_line(&mut self) {
        self.carriage_return();
        self.line_feed();
    }

    pub(crate) fn reverse_index(&mut self) {
        let (cursor_row, scroll_top) = {
            let screen = self.active();
            (screen.cursor_row, screen.scroll_top)
        };
        if cursor_row == scroll_top {
            self.scroll_down(1);
        } else {
            let screen = self.active_mut();
            screen.cursor_row = screen.cursor_row.saturating_sub(1);
        }
    }

    pub(crate) fn put_ascii_run(&mut self, bytes: &[u8]) -> usize {
        if bytes.is_empty() || self.insert_mode {
            return 0;
        }

        let (row, col, cols, run_len) = {
            let screen = self.active();
            if screen.wrap_pending {
                return 0;
            }
            let row = screen.cursor_row;
            let col = screen.cursor_col;
            let run_len = bytes.len().min(screen.cols.saturating_sub(col));
            let run_len = screen.row(row)[col..col + run_len]
                .iter()
                .position(|cell| {
                    cell.wide_spacer()
                        || cell.leading_wide_spacer()
                        || character_width(cell.character) == 2
                })
                .unwrap_or(run_len);
            (row, col, screen.cols, run_len)
        };
        if run_len == 0 {
            return 0;
        }
        debug_assert!(
            bytes[..run_len]
                .iter()
                .all(|byte| (0x20..=0x7e).contains(byte))
        );

        let pen = self.pen();
        for (cell, byte) in self.active_mut().row_mut(row)[col..col + run_len]
            .iter_mut()
            .zip(&bytes[..run_len])
        {
            *cell = pen.cell(char::from(*byte));
        }

        let end = col + run_len;
        let screen = self.active_mut();
        if end >= cols {
            screen.cursor_col = cols.saturating_sub(1);
            screen.wrap_pending = true;
        } else {
            screen.cursor_col = end;
            screen.wrap_pending = false;
        }
        self.damage.mark(row, col, end - 1);
        run_len
    }

    pub(crate) fn put_char(&mut self, character: char) {
        let width = character_width(character);
        if width == 0 {
            self.put_combining(character);
            return;
        }

        let (wrap_pending, wide_does_not_fit) = {
            let screen = self.active();
            (
                screen.wrap_pending,
                width == 2 && screen.cursor_col + 1 >= screen.cols,
            )
        };
        let should_wrap = wrap_pending || wide_does_not_fit;
        let wrapped_wide_placeholder = wide_does_not_fit && !wrap_pending && self.auto_wrap;
        if should_wrap && self.auto_wrap {
            let (row, col) = {
                let screen = self.active();
                (screen.cursor_row, screen.cursor_col)
            };
            if wide_does_not_fit && !wrap_pending {
                let pen = self.pen();
                let mut spacer = pen.cell(' ');
                spacer.set_leading_wide_spacer(true);
                self.write_cell_at(row, col, spacer);
            }
            self.active_mut().row_mut(row)[col].set_wrapped(true);
            self.damage.mark(row, col, col);
            self.carriage_return();
            self.line_feed();
        } else if width == 2 && self.active().cursor_col + 1 >= self.active().cols {
            self.active_mut().wrap_pending = true;
            return;
        }

        let pen = self.pen();
        let insert_mode = self.insert_mode && !wrapped_wide_placeholder;
        let (row, col, cols) = {
            let screen = self.active();
            (screen.cursor_row, screen.cursor_col, screen.cols)
        };

        if insert_mode && col + width < cols {
            let row_cells = self.active_mut().row_mut(row);
            for source in (col..cols - width).rev() {
                row_cells.swap(source + width, source);
            }
        }

        self.write_cell_at(row, col, pen.cell(character));
        if width == 2 && col + 1 < cols {
            let mut spacer = pen.cell(' ');
            spacer.set_wide_spacer(true);
            self.write_cell_at(row, col + 1, spacer);
        }

        {
            let screen = self.active_mut();
            if col + width >= cols {
                screen.cursor_col = cols.saturating_sub(1);
                screen.wrap_pending = true;
            } else {
                screen.cursor_col += width;
                screen.wrap_pending = false;
            }
        }
        self.damage
            .mark(row, col, col.saturating_add(width - 1).min(cols - 1));
    }

    fn put_combining(&mut self, character: char) {
        let (row, mut col, wrap_pending) = {
            let screen = self.active();
            (screen.cursor_row, screen.cursor_col, screen.wrap_pending)
        };
        if !wrap_pending {
            col = col.saturating_sub(1);
        }
        if self.active().row(row)[col].wide_spacer() {
            col = col.saturating_sub(1);
        }

        let previous_id = self.active().row(row)[col].combining_id;
        let Some(previous_id) = previous_id else {
            self.active_mut().row_mut(row)[col].combining_id = Some(inline_combining_id(character));
            self.damage.mark(row, col, col);
            return;
        };
        let previous = inline_combining_character(previous_id).map_or_else(
            || {
                self.combining
                    .get(&previous_id)
                    .cloned()
                    .expect("pooled combining IDs always resolve to text")
            },
            CombiningText::from_char,
        );
        let text = previous.appended(character);

        if self.combining.len() >= COMBINING_PRUNE_INTERVAL as usize
            && self
                .next_combining_id
                .is_multiple_of(COMBINING_PRUNE_INTERVAL)
        {
            self.prune_combining();
        }
        let id = self.next_available_combining_id();
        self.combining.insert(id, text);
        self.active_mut().row_mut(row)[col].combining_id = Some(id);
        self.damage.mark(row, col, col);
    }

    fn next_available_combining_id(&mut self) -> NonZeroU32 {
        loop {
            let raw = self.next_combining_id.clamp(1, POOLED_COMBINING_BIT - 1);
            self.next_combining_id = if raw == POOLED_COMBINING_BIT - 1 {
                1
            } else {
                raw + 1
            };
            let id = NonZeroU32::new(POOLED_COMBINING_BIT | raw)
                .expect("pooled combining IDs are never zero");
            if !self.combining.contains_key(&id) {
                return id;
            }
        }
    }

    fn prune_combining(&mut self) {
        let mut live = HashSet::new();
        live.extend(
            self.primary
                .cells
                .iter()
                .flatten()
                .filter_map(|cell| cell.combining_id.filter(|id| is_pooled_combining_id(*id))),
        );
        live.extend(
            self.alternate
                .cells
                .iter()
                .flatten()
                .filter_map(|cell| cell.combining_id.filter(|id| is_pooled_combining_id(*id))),
        );
        live.extend(self.history.iter().flat_map(|row| {
            row.iter()
                .filter_map(|cell| cell.combining_id.filter(|id| is_pooled_combining_id(*id)))
        }));
        self.combining.retain(|id, _| live.contains(id));
    }

    fn write_cell_at(&mut self, row: usize, col: usize, cell: Cell) {
        let old = self.active().row(row)[col];
        let overwrites_wide = old.wide_spacer() || character_width(old.character) == 2;
        if overwrites_wide && col <= 1 {
            self.clear_leading_wide_spacer_before(row);
        }

        let row_cells = self.active_mut().row_mut(row);
        if character_width(old.character) == 2 && col + 1 < row_cells.len() {
            row_cells[col + 1].set_wide_spacer(false);
        } else if old.wide_spacer() && col > 0 {
            row_cells[col - 1].character = ' ';
            row_cells[col - 1].combining_id = None;
        }
        row_cells[col] = cell;
    }

    fn clear_leading_wide_spacer_before(&mut self, row: usize) {
        let col = self.cols().saturating_sub(1);
        let changed = if row > 0 {
            let previous = self.active_mut().row_mut(row - 1);
            previous[col].take_leading_wide_spacer()
        } else if !self.alternate_active {
            self.history.back_mut().is_some_and(|previous| {
                previous
                    .get_mut(col)
                    .is_some_and(Cell::take_leading_wide_spacer)
            })
        } else {
            false
        };
        if changed {
            if row > 0 {
                self.damage.mark(row - 1, col, col);
            } else {
                self.damage.mark_full();
            }
        }
    }

    pub(crate) fn cursor_up(&mut self, count: usize) {
        let origin_mode = self.origin_mode;
        let origin_offset = if origin_mode {
            self.active().scroll_top
        } else {
            0
        };
        let screen = self.active_mut();
        let target = screen.cursor_row as i64 - count as i64 + origin_offset as i64;
        let maximum = if origin_mode {
            screen.scroll_bottom
        } else {
            screen.rows.saturating_sub(1)
        };
        screen.cursor_row = target.clamp(0, maximum as i64) as usize;
        screen.wrap_pending = false;
    }

    pub(crate) fn cursor_down(&mut self, count: usize) {
        let origin_mode = self.origin_mode;
        let origin_offset = if origin_mode {
            self.active().scroll_top
        } else {
            0
        };
        let screen = self.active_mut();
        let maximum = if origin_mode {
            screen.scroll_bottom
        } else {
            screen.rows.saturating_sub(1)
        };
        screen.cursor_row = screen
            .cursor_row
            .saturating_add(count)
            .saturating_add(origin_offset)
            .min(maximum);
        screen.wrap_pending = false;
    }

    pub(crate) fn cursor_forward(&mut self, count: usize) {
        let screen = self.active_mut();
        screen.cursor_col = screen
            .cursor_col
            .saturating_add(count)
            .min(screen.cols.saturating_sub(1));
        screen.wrap_pending = false;
    }

    pub(crate) fn cursor_backward(&mut self, count: usize) {
        let screen = self.active_mut();
        screen.cursor_col = screen.cursor_col.saturating_sub(count);
        screen.wrap_pending = false;
    }

    pub(crate) fn cursor_next_line(&mut self, count: usize) {
        self.cursor_down(count);
        self.carriage_return();
    }

    pub(crate) fn cursor_previous_line(&mut self, count: usize) {
        self.cursor_up(count);
        self.carriage_return();
    }

    pub(crate) fn set_cursor_col(&mut self, col: usize) {
        let origin_mode = self.origin_mode;
        let screen = self.active_mut();
        if origin_mode {
            screen.cursor_row = screen
                .cursor_row
                .saturating_add(screen.scroll_top)
                .min(screen.scroll_bottom);
        }
        screen.cursor_col = col.min(screen.cols.saturating_sub(1));
        screen.wrap_pending = false;
    }

    pub(crate) fn set_cursor_row(&mut self, row: usize) {
        let origin_mode = self.origin_mode;
        let screen = self.active_mut();
        screen.cursor_row = if origin_mode {
            screen
                .scroll_top
                .saturating_add(row)
                .min(screen.scroll_bottom)
        } else {
            row.min(screen.rows.saturating_sub(1))
        };
        screen.wrap_pending = false;
    }

    pub(crate) fn set_cursor_position(&mut self, row: usize, col: usize) {
        let origin_mode = self.origin_mode;
        let screen = self.active_mut();
        screen.cursor_row = if origin_mode {
            screen
                .scroll_top
                .saturating_add(row)
                .min(screen.scroll_bottom)
        } else {
            row.min(screen.rows.saturating_sub(1))
        };
        screen.cursor_col = col.min(screen.cols.saturating_sub(1));
        screen.wrap_pending = false;
    }

    pub(crate) fn save_cursor(&mut self) {
        self.active_mut().save_cursor();
    }

    pub(crate) fn restore_cursor(&mut self) {
        self.active_mut().restore_cursor();
    }

    pub(crate) fn set_scroll_region(&mut self, top: usize, bottom: usize) {
        let rows = self.rows();
        let bottom = bottom.min(rows.saturating_sub(1));
        if top >= bottom || top >= rows {
            return;
        }
        let origin_mode = self.origin_mode;
        self.primary.scroll_top = top;
        self.primary.scroll_bottom = bottom;
        self.alternate.scroll_top = top;
        self.alternate.scroll_bottom = bottom;
        let screen = self.active_mut();
        let row = if origin_mode { top } else { 0 };
        screen.cursor_row = row;
        screen.cursor_col = 0;
        screen.wrap_pending = false;
    }

    pub(crate) fn reset_scroll_region(&mut self) {
        let origin_mode = self.origin_mode;
        let primary_bottom = self.primary.rows.saturating_sub(1);
        let alternate_bottom = self.alternate.rows.saturating_sub(1);
        self.primary.scroll_top = 0;
        self.primary.scroll_bottom = primary_bottom;
        self.alternate.scroll_top = 0;
        self.alternate.scroll_bottom = alternate_bottom;
        let screen = self.active_mut();
        screen.cursor_row = if origin_mode { screen.scroll_top } else { 0 };
        screen.cursor_col = 0;
        screen.wrap_pending = false;
    }

    pub(crate) fn erase_display(&mut self, mode: u16, selective: bool) {
        let (row, col, rows, cols) = {
            let screen = self.active();
            (
                screen.cursor_row,
                screen.cursor_col,
                screen.rows,
                screen.cols,
            )
        };
        match mode {
            0 => {
                self.erase_row_range(row, col, cols.saturating_sub(1), selective);
                for target in row + 1..rows {
                    self.erase_row_range(target, 0, cols.saturating_sub(1), selective);
                }
            }
            1 => {
                if row > 1 {
                    for target in 0..row {
                        self.erase_row_range(target, 0, cols.saturating_sub(1), selective);
                    }
                }
                self.erase_row_range(row, 0, col, selective);
            }
            2 if selective => {
                for target in 0..rows {
                    self.erase_row_range(target, 0, cols.saturating_sub(1), true);
                }
            }
            2 => {
                self.effects.push_back(GridEffect::ClearViewport {
                    alternate: self.alternate_active,
                    history_size: self.history_size(),
                    rows,
                    cols,
                });
                let blank = self.pen().blank();
                if self.alternate_active {
                    self.active_mut().fill(blank);
                } else {
                    let positions = (0..rows)
                        .rev()
                        .find(|&target| {
                            self.primary
                                .row(target)
                                .iter()
                                .any(|cell| !cell_is_empty_for_viewport(cell))
                        })
                        .map_or_else(|| usize::from(self.history.is_empty()), |target| target + 1);
                    let wrap_pending = self.primary.wrap_pending;
                    if positions > 0 {
                        self.shift_rows_up(0, rows - 1, positions, true);
                    }
                    let reset_rows = rows.saturating_sub(positions);
                    for row in self.primary.cells.iter_mut().take(reset_rows) {
                        row.fill(blank);
                    }
                    self.primary.wrap_pending = wrap_pending;
                }
                self.damage.mark_full();
            }
            3 if !selective => {
                self.clear_history();
            }
            _ => {}
        }
    }

    pub(crate) fn erase_line(&mut self, mode: u16, selective: bool) {
        let (row, col, cols) = {
            let screen = self.active();
            (screen.cursor_row, screen.cursor_col, screen.cols)
        };
        match mode {
            0 if self.active().wrap_pending => {}
            0 => self.erase_row_range(row, col, cols.saturating_sub(1), selective),
            1 => self.erase_row_range(row, 0, col, selective),
            2 => self.erase_row_range(row, 0, cols.saturating_sub(1), selective),
            _ => {}
        }
    }

    pub(crate) fn erase_chars(&mut self, count: usize) {
        let (row, col, cols) = {
            let screen = self.active();
            (screen.cursor_row, screen.cursor_col, screen.cols)
        };
        self.erase_row_range(row, col, col.saturating_add(count - 1).min(cols - 1), false);
    }

    fn erase_row_range(&mut self, row: usize, mut left: usize, mut right: usize, selective: bool) {
        if left > right || row >= self.rows() {
            return;
        }
        let cols = self.cols();
        if selective {
            let cells = self.active().row(row);
            if cells[left].wide_spacer() {
                left = left.saturating_sub(1);
            }
            if right + 1 < cols && cells[right + 1].wide_spacer() {
                right += 1;
            }
        }
        let blank = self.pen().blank();
        let row_cells = self.active_mut().row_mut(row);
        if selective {
            let mut col = left;
            while col <= right {
                let pair_start = if row_cells[col].wide_spacer() {
                    col.saturating_sub(1)
                } else {
                    col
                };
                let pair_end = if pair_start + 1 < cols && row_cells[pair_start + 1].wide_spacer() {
                    pair_start + 1
                } else {
                    pair_start
                };
                if !row_cells[pair_start..=pair_end].iter().any(Cell::protected) {
                    row_cells[pair_start..=pair_end].fill(blank);
                }
                col = pair_end.saturating_add(1);
            }
        } else {
            row_cells[left..=right].fill(blank);
        }
        self.damage.mark(row, left, right);
    }

    pub(crate) fn insert_blank_chars(&mut self, count: usize) {
        let (row, col, cols) = {
            let screen = self.active();
            (screen.cursor_row, screen.cursor_col, screen.cols)
        };
        let count = count.min(cols.saturating_sub(col));
        if count == 0 {
            return;
        }
        let blank = self.pen().blank();
        let row_cells = self.active_mut().row_mut(row);
        row_cells.copy_within(col..cols - count, col + count);
        row_cells[col..col + count].fill(blank);
        self.damage.mark(row, col, cols - 1);
    }

    pub(crate) fn delete_chars(&mut self, count: usize) {
        let (row, col, cols) = {
            let screen = self.active();
            (screen.cursor_row, screen.cursor_col, screen.cols)
        };
        let count = count.min(cols);
        if count == 0 {
            return;
        }
        let end = col.saturating_add(count).min(cols - 1);
        let num_cells = cols - end;
        let blank = self.pen().blank();
        let row_cells = self.active_mut().row_mut(row);
        for offset in 0..num_cells {
            row_cells.swap(col + offset, end + offset);
        }
        row_cells[cols - count..].fill(blank);
        self.damage.mark(row, col, cols - 1);
    }

    pub(crate) fn insert_lines(&mut self, count: usize) {
        let (row, top, bottom) = {
            let screen = self.active();
            (screen.cursor_row, screen.scroll_top, screen.scroll_bottom)
        };
        if row < top || row > bottom {
            return;
        }
        self.shift_rows_down(row, bottom, count);
    }

    pub(crate) fn delete_lines(&mut self, count: usize) {
        let (row, top, bottom) = {
            let screen = self.active();
            (screen.cursor_row, screen.scroll_top, screen.scroll_bottom)
        };
        if row < top || row > bottom {
            return;
        }
        let record_history = !self.alternate_active && row == 0;
        self.shift_rows_up(row, bottom, count, record_history);
    }

    pub(crate) fn scroll_up(&mut self, count: usize) {
        let (top, bottom) = {
            let screen = self.active();
            (screen.scroll_top, screen.scroll_bottom)
        };
        let record_history = !self.alternate_active && top == 0;
        self.shift_rows_up(top, bottom, count, record_history);
    }

    pub(crate) fn scroll_down(&mut self, count: usize) {
        let (top, bottom) = {
            let screen = self.active();
            (screen.scroll_top, screen.scroll_bottom)
        };
        self.shift_rows_down(top, bottom, count);
    }

    fn shift_rows_up(&mut self, top: usize, bottom: usize, count: usize, record_history: bool) {
        let count = count.min(bottom.saturating_sub(top).saturating_add(1));
        if count == 0 {
            return;
        }
        let cols = self.cols();
        let alternate = self.alternate_active;
        let history_before = self.history.len();
        let blank = self.pen().blank();
        self.active_mut().cells[top..=bottom].rotate_left(count);
        for target in bottom + 1 - count..=bottom {
            let removed = std::mem::take(&mut self.active_mut().cells[target]);
            let mut recycled = if record_history {
                self.push_history(removed).unwrap_or_default()
            } else {
                removed
            };
            if recycled.is_empty() {
                recycled.resize(cols, blank);
            } else {
                recycled.resize(cols, blank);
                recycled.fill(blank);
            }
            self.active_mut().cells[target] = recycled;
        }
        let history_after = self.history.len();
        self.effects.push_back(GridEffect::ScrollUp {
            alternate,
            top,
            bottom,
            count,
            recorded_history: record_history,
            history_before,
            history_after,
        });
        self.damage.mark_full();
    }

    fn shift_rows_down(&mut self, top: usize, bottom: usize, count: usize) {
        let count = count.min(bottom.saturating_sub(top).saturating_add(1));
        if count == 0 {
            return;
        }
        let alternate = self.alternate_active;
        let history_size = self.history.len();
        let blank = self.pen().blank();
        self.active_mut().cells[top..=bottom].rotate_right(count);
        for target in top..top + count {
            self.active_mut().cells[target].fill(blank);
        }
        self.effects.push_back(GridEffect::ScrollDown {
            alternate,
            top,
            bottom,
            count,
            history_size,
        });
        self.damage.mark_full();
    }

    fn push_history(&mut self, mut row: Vec<Cell>) -> Option<Vec<Cell>> {
        if self.history_limit == 0 {
            return Some(row);
        }
        row.resize(self.primary.cols, Cell::default());
        row.truncate(self.primary.cols);
        self.history.push_back(row);
        if self.display_offset > 0 {
            self.display_offset = self.display_offset.saturating_add(1);
        }
        let mut recycled = None;
        while self.history.len() > self.history_limit {
            recycled = self.history.pop_front();
            self.display_offset = self.display_offset.saturating_sub(1);
        }
        self.display_offset = self.display_offset.min(self.history.len());
        recycled
    }

    pub(crate) fn reset(&mut self) {
        self.primary.reset();
        self.alternate.reset();
        self.alternate_active = false;
        self.history.clear();
        self.display_offset = 0;
        self.cursor_visible = true;
        self.cursor_style = self.default_cursor_style;
        self.cursor_style_explicit = false;
        self.cursor_shape_status = cursor_shape_status(self.default_cursor_style);
        self.auto_wrap = true;
        self.insert_mode = false;
        self.origin_mode = false;
        self.line_feed_new_line = false;
        self.bracketed_paste = false;
        self.cursor_blinking = false;
        self.focus_reporting = false;
        self.alternate_scroll = true;
        self.urgency_hints = true;
        self.mouse_mode = MouseMode::default();
        self.keyboard_mode = KeyboardMode::default();
        self.kitty_keyboard_stack.clear();
        self.inactive_kitty_keyboard_stack.clear();
        self.tab_stops = default_tab_stops(self.cols());
        self.palette = Palette::default();
        self.hyperlinks.clear();
        self.next_hyperlink_id = 1;
        self.combining.clear();
        self.next_combining_id = 1;
        self.effects.clear();
        self.effects.push_back(GridEffect::Reset);
        self.damage.mark_full();
    }

    pub(crate) fn palette(&self) -> Palette {
        self.palette
    }

    pub(crate) fn set_indexed_color(&mut self, index: u8, color: Rgb) {
        let slot = &mut self.palette.indexed[usize::from(index)];
        if *slot != Some(color) {
            *slot = Some(color);
            self.damage.mark_full();
        }
    }

    pub(crate) fn reset_indexed_color(&mut self, index: u8) {
        let slot = &mut self.palette.indexed[usize::from(index)];
        if slot.take().is_some() {
            self.damage.mark_full();
        }
    }

    pub(crate) fn reset_indexed_colors(&mut self) {
        if self.palette.indexed.iter().any(Option::is_some) {
            self.palette.indexed.fill(None);
            self.damage.mark_full();
        }
    }

    pub(crate) fn set_foreground_color(&mut self, color: Option<Rgb>) {
        if self.palette.foreground != color {
            self.palette.foreground = color;
            self.damage.mark_full();
        }
    }

    pub(crate) fn set_background_color(&mut self, color: Option<Rgb>) {
        if self.palette.background != color {
            self.palette.background = color;
            self.damage.mark_full();
        }
    }

    pub(crate) fn set_cursor_color(&mut self, color: Option<Rgb>) {
        if self.palette.cursor != color {
            self.palette.cursor = color;
            self.damage.mark_full();
        }
    }

    pub(crate) fn set_mode(&mut self, private: bool, mode: u16, enabled: bool) {
        if !private {
            match mode {
                4 => self.insert_mode = enabled,
                20 => self.line_feed_new_line = enabled,
                _ => {}
            }
            return;
        }

        match mode {
            1 => self.keyboard_mode.application_cursor_keys = enabled,
            3 => self.column_mode_reset(),
            6 => {
                self.origin_mode = enabled;
                if enabled {
                    let screen = self.active_mut();
                    screen.cursor_row = screen.scroll_top;
                    screen.cursor_col = 0;
                    screen.wrap_pending = false;
                }
            }
            7 => self.auto_wrap = enabled,
            12 => {
                self.cursor_blinking = enabled;
                self.cursor_style_explicit = true;
            }
            25 => self.cursor_visible = enabled,
            47 | 1047 => self.set_alternate_screen(enabled, false),
            1048 => {
                if enabled {
                    self.save_cursor();
                } else {
                    self.restore_cursor();
                }
            }
            1049 => self.set_alternate_screen(enabled, true),
            1000 => {
                if enabled {
                    self.mouse_mode.report_drag = false;
                    self.mouse_mode.report_motion = false;
                }
                self.mouse_mode.report_click = enabled;
            }
            1002 => {
                if enabled {
                    self.mouse_mode.report_click = false;
                    self.mouse_mode.report_motion = false;
                }
                self.mouse_mode.report_drag = enabled;
            }
            1003 => {
                if enabled {
                    self.mouse_mode.report_click = false;
                    self.mouse_mode.report_drag = false;
                }
                self.mouse_mode.report_motion = enabled;
            }
            1004 => self.focus_reporting = enabled,
            1005 => {
                if enabled {
                    self.mouse_mode.sgr_encoding = false;
                }
                self.mouse_mode.utf8_encoding = enabled;
            }
            1006 => {
                if enabled {
                    self.mouse_mode.utf8_encoding = false;
                }
                self.mouse_mode.sgr_encoding = enabled;
            }
            1007 => self.alternate_scroll = enabled,
            1042 => self.urgency_hints = enabled,
            2004 => self.bracketed_paste = enabled,
            _ => {}
        }
        self.mouse_mode.enabled = self.mouse_mode.report_click
            || self.mouse_mode.report_drag
            || self.mouse_mode.report_motion;
    }

    fn column_mode_reset(&mut self) {
        self.reset_scroll_region();
        let rows = self.rows();
        let cols = self.cols();
        self.effects.push_back(GridEffect::ClearViewport {
            alternate: self.alternate_active,
            history_size: self.history_size(),
            rows,
            cols,
        });
        let blank = self.pen().blank();
        self.active_mut().fill(blank);
        self.damage.mark_full();
    }

    pub(crate) fn set_keypad_application_mode(&mut self, enabled: bool) {
        self.keyboard_mode.application_keypad = enabled;
    }

    pub(crate) fn alignment_test(&mut self) {
        for row in &mut self.active_mut().cells {
            row.fill(Cell {
                character: 'E',
                ..Cell::default()
            });
        }
        self.damage.mark_full();
    }

    pub(crate) fn mode_state(&self, private: bool, mode: u16) -> u8 {
        let supported = if private {
            match mode {
                1 => Some(self.keyboard_mode.application_cursor_keys),
                3 => None,
                6 => Some(self.origin_mode),
                7 => Some(self.auto_wrap),
                12 => Some(self.cursor_blinking),
                25 => Some(self.cursor_visible),
                47 | 1047 | 1049 => Some(self.alternate_active),
                1000 => Some(self.mouse_mode.report_click),
                1002 => Some(self.mouse_mode.report_drag),
                1003 => Some(self.mouse_mode.report_motion),
                1004 => Some(self.focus_reporting),
                1005 => Some(self.mouse_mode.utf8_encoding),
                1006 => Some(self.mouse_mode.sgr_encoding),
                1007 => Some(self.alternate_scroll),
                1042 => Some(self.urgency_hints),
                2004 => Some(self.bracketed_paste),
                2026 => Some(false),
                _ => return 0,
            }
        } else {
            match mode {
                4 => Some(self.insert_mode),
                20 => Some(self.line_feed_new_line),
                _ => return 0,
            }
        };
        match supported {
            Some(true) => 1,
            Some(false) => 2,
            None => 0,
        }
    }

    fn set_alternate_screen(&mut self, enabled: bool, save_cursor: bool) {
        if enabled == self.alternate_active {
            return;
        }
        if enabled {
            if save_cursor {
                self.primary.save_cursor();
            }
            let cursor_col = self.primary.cursor_col;
            let cursor_row = self.primary.cursor_row;
            let pen = self.primary.pen;
            let charsets = self.primary.charsets;
            let wrap_pending = self.primary.wrap_pending;
            let scroll_top = self.primary.scroll_top;
            let scroll_bottom = self.primary.scroll_bottom;
            self.alternate.fill(pen.blank());
            self.alternate.cursor_col = cursor_col.min(self.alternate.cols.saturating_sub(1));
            self.alternate.cursor_row = cursor_row.min(self.alternate.rows.saturating_sub(1));
            self.alternate.pen = pen;
            self.alternate.charsets = charsets;
            self.alternate.wrap_pending = wrap_pending;
            self.alternate.scroll_top = scroll_top.min(self.alternate.rows.saturating_sub(1));
            self.alternate.scroll_bottom = scroll_bottom.min(self.alternate.rows.saturating_sub(1));
            self.alternate_active = true;
            self.display_offset = 0;
            self.effects.push_back(GridEffect::EnteredAlternate);
        } else {
            self.alternate_active = false;
        }
        std::mem::swap(
            &mut self.kitty_keyboard_stack,
            &mut self.inactive_kitty_keyboard_stack,
        );
        let flags = self.kitty_keyboard_stack.last().copied().unwrap_or(0);
        self.set_kitty_keyboard_flags(flags, 1);
        self.damage.mark_full();
    }

    pub(crate) fn set_cursor_style(&mut self, parameter: u16) {
        let (style, shape_status, blinking, explicit) = match parameter {
            0 => (
                self.default_cursor_style,
                cursor_shape_status(self.default_cursor_style),
                false,
                false,
            ),
            1 | 2 => (CursorStyle::Block, 2, parameter == 1, true),
            3 | 4 => (CursorStyle::Line, 4, parameter == 3, true),
            5 | 6 => (CursorStyle::Line, 6, parameter == 5, true),
            _ => return,
        };
        self.cursor_style = style;
        self.cursor_shape_status = shape_status;
        self.cursor_blinking = blinking;
        self.cursor_style_explicit = explicit;
        self.damage.mark_full();
    }

    pub(crate) fn set_cursor_shape(&mut self, parameter: u16) {
        let (style, shape_status) = match parameter {
            0 => (CursorStyle::Block, 2),
            1 => (CursorStyle::Line, 6),
            2 => (CursorStyle::Line, 4),
            _ => return,
        };
        self.cursor_style = style;
        self.cursor_shape_status = shape_status;
        self.cursor_style_explicit = true;
        self.damage.mark_full();
    }

    pub(crate) fn cursor_style_status(&self) -> u16 {
        self.cursor_shape_status - u16::from(self.cursor_blinking)
    }

    pub(crate) fn set_character_protection(&mut self, parameter: u16) {
        self.pen_mut().protected = parameter == 1;
    }

    pub(crate) fn character_protection_status(&self) -> u16 {
        u16::from(self.pen().protected)
    }

    pub(crate) fn scroll_region_status(&self) -> (usize, usize) {
        let screen = self.active();
        (screen.scroll_top + 1, screen.scroll_bottom + 1)
    }

    pub(crate) fn sgr_status(&self) -> String {
        let mut codes = Vec::with_capacity(12);
        let pen = self.pen();
        let attributes = pen.attributes;
        if attributes.bold() {
            codes.push("1".to_string());
        }
        if attributes.dim() {
            codes.push("2".to_string());
        }
        if attributes.italic() {
            codes.push("3".to_string());
        }
        if attributes.underline() {
            codes.push("4".to_string());
        }
        if attributes.inverse() {
            codes.push("7".to_string());
        }
        if attributes.hidden() {
            codes.push("8".to_string());
        }
        if attributes.strikethrough() {
            codes.push("9".to_string());
        }
        push_sgr_color(&mut codes, pen.foreground, true);
        push_sgr_color(&mut codes, pen.background, false);
        if codes.is_empty() {
            codes.push("0".to_string());
        }
        format!("{}m", codes.join(";"))
    }

    pub(crate) fn set_hyperlink(&mut self, target: Option<&str>) {
        let Some(target) = target.filter(|target| !target.is_empty()) else {
            self.pen_mut().hyperlink_id = None;
            return;
        };

        if let Some(id) = self
            .hyperlinks
            .iter()
            .find_map(|(id, existing)| (existing == target).then_some(*id))
        {
            self.pen_mut().hyperlink_id = NonZeroU32::new(id);
            return;
        }
        if self.hyperlinks.len() >= 4096 {
            self.prune_hyperlinks();
        }
        let id = self.next_hyperlink_id.max(1);
        self.next_hyperlink_id = self.next_hyperlink_id.wrapping_add(1).max(1);
        self.hyperlinks.insert(id, target.to_string());
        self.pen_mut().hyperlink_id = NonZeroU32::new(id);
    }

    fn prune_hyperlinks(&mut self) {
        let mut live = HashSet::new();
        live.extend(
            self.primary
                .cells
                .iter()
                .flatten()
                .filter_map(|cell| cell.hyperlink_id),
        );
        live.extend(
            self.alternate
                .cells
                .iter()
                .flatten()
                .filter_map(|cell| cell.hyperlink_id),
        );
        live.extend(
            self.history
                .iter()
                .flat_map(|row| row.iter().filter_map(|cell| cell.hyperlink_id)),
        );
        for pen in [
            self.primary.pen,
            self.primary.saved_pen,
            self.alternate.pen,
            self.alternate.saved_pen,
        ] {
            if let Some(active) = pen.hyperlink_id {
                live.insert(active);
            }
        }
        self.hyperlinks
            .retain(|id, _| NonZeroU32::new(*id).is_some_and(|id| live.contains(&id)));
    }

    pub(crate) fn set_kitty_keyboard_flags(&mut self, flags: u16, mode: u16) {
        let apply = |current: &mut bool, bit: u16| match mode {
            2 => *current |= flags & bit != 0,
            3 => *current &= flags & bit == 0,
            _ => *current = flags & bit != 0,
        };
        apply(&mut self.keyboard_mode.disambiguate_escape_codes, 1 << 0);
        apply(&mut self.keyboard_mode.report_event_types, 1 << 1);
        apply(&mut self.keyboard_mode.report_alternate_keys, 1 << 2);
        apply(&mut self.keyboard_mode.report_all_keys_as_esc, 1 << 3);
        apply(&mut self.keyboard_mode.report_associated_text, 1 << 4);
    }

    pub(crate) fn kitty_keyboard_flags(&self) -> u16 {
        self.keyboard_mode.kitty_flags()
    }

    pub(crate) fn push_kitty_keyboard_flags(&mut self, flags: u16) {
        if self.kitty_keyboard_stack.len() >= 64 {
            self.kitty_keyboard_stack.remove(0);
        }
        self.kitty_keyboard_stack.push(flags & 0x1f);
        self.set_kitty_keyboard_flags(flags, 1);
    }

    pub(crate) fn pop_kitty_keyboard_flags(&mut self, count: usize) {
        let count = count.max(1).min(self.kitty_keyboard_stack.len());
        for _ in 0..count {
            self.kitty_keyboard_stack.pop();
        }
        let flags = self.kitty_keyboard_stack.last().copied().unwrap_or(0);
        self.set_kitty_keyboard_flags(flags, 1);
    }

    pub(crate) fn sgr(&mut self, params: &[u16]) {
        if params.is_empty() {
            let protected = self.pen().protected;
            *self.pen_mut() = Pen {
                protected,
                ..Pen::default()
            };
            return;
        }

        let pen = self.pen_mut();
        let mut index = 0;
        while index < params.len() {
            let code = params[index];
            match code {
                0 => {
                    let protected = pen.protected;
                    *pen = Pen {
                        protected,
                        ..Pen::default()
                    };
                }
                1 => pen.attributes.set_bold(true),
                2 => pen.attributes.set_dim(true),
                3 => pen.attributes.set_italic(true),
                4 => pen.attributes.set_underline(true),
                7 => pen.attributes.set_inverse(true),
                8 => pen.attributes.set_hidden(true),
                9 => pen.attributes.set_strikethrough(true),
                21 => pen.attributes.set_bold(false),
                22 => {
                    pen.attributes.set_bold(false);
                    pen.attributes.set_dim(false);
                }
                23 => pen.attributes.set_italic(false),
                24 => pen.attributes.set_underline(false),
                27 => pen.attributes.set_inverse(false),
                28 => pen.attributes.set_hidden(false),
                29 => pen.attributes.set_strikethrough(false),
                30..=37 => pen.foreground = Color::Indexed((code - 30) as u8),
                38 => {
                    if let Some(color) = extended_color(params, &mut index) {
                        pen.foreground = color;
                    }
                }
                39 => pen.foreground = Color::Default,
                40..=47 => pen.background = Color::Indexed((code - 40) as u8),
                48 => {
                    if let Some(color) = extended_color(params, &mut index) {
                        pen.background = color;
                    }
                }
                49 => pen.background = Color::Default,
                58 => {
                    let _ = extended_color(params, &mut index);
                }
                59 => {}
                90..=97 => pen.foreground = Color::Indexed((code - 90 + 8) as u8),
                100..=107 => pen.background = Color::Indexed((code - 100 + 8) as u8),
                _ => {}
            }
            index += 1;
        }
    }

    pub(crate) fn resize(&mut self, cols: u16, rows: u16) {
        let cols = usize::from(cols.clamp(1, MAX_GRID_DIMENSION));
        let rows = usize::from(rows.clamp(1, MAX_GRID_DIMENSION));
        if self.primary.cols == cols && self.primary.rows == rows {
            return;
        }

        if self.primary.cols != cols {
            self.reflow_primary(cols, rows);
            self.alternate.resize(cols, rows, false);
            self.effects
                .push_back(GridEffect::Reflow { alternate: false });
            self.effects
                .push_back(GridEffect::Reflow { alternate: true });
        } else {
            self.resize_primary_rows(rows);
            let alternate_old_rows = self.alternate.rows;
            let alternate_cursor_row = self.alternate.cursor_row;
            self.alternate.resize(cols, rows, false);
            if rows < alternate_old_rows {
                let count = alternate_cursor_row.saturating_add(1).saturating_sub(rows);
                if count > 0 {
                    self.effects.push_back(GridEffect::ScrollUp {
                        alternate: true,
                        top: 0,
                        bottom: alternate_old_rows.saturating_sub(1),
                        count,
                        recorded_history: false,
                        history_before: 0,
                        history_after: 0,
                    });
                }
            }
        }
        self.display_offset = self.display_offset.min(self.history.len());
        let old_tab_count = self.tab_stops.len();
        self.tab_stops.resize(cols, false);
        for col in old_tab_count..cols {
            if col != 0 && col % 8 == 0 {
                self.tab_stops[col] = true;
            }
        }
        self.damage.resize(rows);
    }

    fn resize_primary_rows(&mut self, rows: usize) {
        let old_rows = self.primary.rows;
        if rows == old_rows {
            return;
        }
        let cols = self.primary.cols;
        if rows > old_rows {
            let added = rows - old_rows;
            let pulled = added.min(self.history.len());
            let pulled_rows = self.history.split_off(self.history.len() - pulled);
            let mut cells = Vec::with_capacity(rows);
            cells.extend(pulled_rows);
            cells.append(&mut self.primary.cells);
            cells.resize_with(rows, || vec![Cell::default(); cols]);
            self.primary.cells = cells;
            self.primary.rows = rows;
            self.primary.cursor_row = self
                .primary
                .cursor_row
                .saturating_add(pulled)
                .min(rows.saturating_sub(1));
            self.primary.saved_cursor_row = self
                .primary
                .saved_cursor_row
                .saturating_add(pulled)
                .min(rows.saturating_sub(1));
            self.primary.scroll_top = 0;
            self.primary.scroll_bottom = rows.saturating_sub(1);
            self.display_offset = self
                .display_offset
                .saturating_sub(added)
                .min(self.history.len());
            return;
        }

        let required_scrolling = self
            .primary
            .cursor_row
            .saturating_add(1)
            .saturating_sub(rows);
        let history_before = self.history.len();
        let removed = self
            .primary
            .cells
            .drain(..required_scrolling)
            .collect::<Vec<_>>();
        for row in removed {
            let _ = self.push_history(row);
        }
        let history_after = self.history.len();
        self.primary.cells.truncate(rows);
        self.primary.rows = rows;
        self.primary.cursor_row = self.primary.cursor_row.min(rows.saturating_sub(1));
        self.primary.saved_cursor_row = self.primary.saved_cursor_row.min(rows.saturating_sub(1));
        self.primary.scroll_top = 0;
        self.primary.scroll_bottom = rows.saturating_sub(1);
        if required_scrolling > 0 {
            self.effects.push_back(GridEffect::ScrollUp {
                alternate: false,
                top: 0,
                bottom: old_rows.saturating_sub(1),
                count: required_scrolling,
                recorded_history: self.history_limit > 0,
                history_before,
                history_after,
            });
        }
    }

    fn reflow_primary(&mut self, cols: usize, rows: usize) {
        let old_cols = self.primary.cols;
        let cursor_row = self.primary.cursor_row;
        let cursor_col = self.primary.cursor_col;
        let old_wrap_pending = self.primary.wrap_pending;
        let last_meaningful_row = (0..self.primary.rows)
            .rev()
            .find(|&row| {
                self.primary
                    .row(row)
                    .iter()
                    .any(|cell| *cell != Cell::default())
            })
            .unwrap_or(0)
            .max(cursor_row);

        let old_history = std::mem::take(&mut self.history);
        let history_rows = old_history.len();
        let screen_rows = std::mem::take(&mut self.primary.cells)
            .into_iter()
            .take(last_meaningful_row + 1)
            .collect::<Vec<_>>();
        let mut output = VecDeque::with_capacity(history_rows.saturating_add(screen_rows.len()));
        let mut logical = Vec::with_capacity(old_cols.saturating_mul(2));
        let mut logical_cursor = None;
        let mut mapped_cursor = None;

        for (physical_row, row) in old_history.into_iter().chain(screen_rows).enumerate() {
            let continues_logical_line = !logical.is_empty();
            let wrapped = row.last().is_some_and(Cell::wrapped);
            let is_cursor_row = physical_row == history_rows.saturating_add(cursor_row);
            let meaningful_used = if wrapped {
                old_cols
            } else {
                row.iter()
                    .rposition(|cell| *cell != Cell::default())
                    .map_or(0, |col| col + 1)
            };
            let mut used = meaningful_used;
            if is_cursor_row {
                let cursor_starts_logical_line = logical.is_empty();
                let cursor_on_empty_continuation = !logical.is_empty()
                    && meaningful_used == 0
                    && cursor_col == 0
                    && !old_wrap_pending;
                let cursor_offset = cursor_col.saturating_add(usize::from(old_wrap_pending));
                // Alacritty trims default trailing cells from a soft-wrapped continuation when
                // the cursor lands exactly on the new boundary. Otherwise the empty cells still
                // participate in cursor reflow, including when the cursor fits inside the row.
                let cursor_offset =
                    if cols < old_cols && !logical.is_empty() && cursor_offset == cols {
                        cursor_offset.min(meaningful_used)
                    } else {
                        cursor_offset
                    };
                used = used.max(cursor_offset);
                let retained_offset = row
                    .iter()
                    .take(cursor_offset.min(used))
                    .filter(|cell| !cell.leading_wide_spacer())
                    .count();
                logical_cursor = Some((
                    logical.len().saturating_add(retained_offset),
                    cursor_on_empty_continuation,
                    cursor_starts_logical_line,
                ));
            }
            for mut cell in row.into_iter().take(used) {
                if cell.leading_wide_spacer() {
                    continue;
                }
                cell.set_wrapped(false);
                logical.push(cell);
            }
            if !wrapped {
                let trailing_empty_continuation = continues_logical_line && meaningful_used == 0;
                let cursor = flush_reflow_line(
                    &mut logical,
                    cols,
                    &mut output,
                    logical_cursor.take(),
                    cols < old_cols,
                    trailing_empty_continuation,
                );
                if cursor.is_some() {
                    mapped_cursor = cursor;
                }
            }
        }
        if !logical.is_empty() {
            let cursor = flush_reflow_line(
                &mut logical,
                cols,
                &mut output,
                logical_cursor.take(),
                cols < old_cols,
                false,
            );
            if cursor.is_some() {
                mapped_cursor = cursor;
            }
        }

        if output.is_empty() {
            output.push_back(vec![Cell::default(); cols]);
        }
        let mut mapped_cursor = mapped_cursor.unwrap_or((output.len().saturating_sub(1), 0, false));
        let mut split = output.len().saturating_sub(rows);
        if mapped_cursor.0 < split {
            split = mapped_cursor.0;
            let keep = split.saturating_add(rows);
            output.truncate(keep);
        }

        self.history = output.drain(..split).collect();
        while self.history.len() > self.history_limit {
            self.history.pop_front();
        }
        let screen_row_count = output.len().min(rows);
        let cursor_row = mapped_cursor
            .0
            .saturating_sub(split)
            .min(rows.saturating_sub(1));
        mapped_cursor.1 = mapped_cursor.1.min(cols.saturating_sub(1));

        let mut cells = output.into_iter().take(rows).collect::<Vec<_>>();
        cells.resize_with(rows, || vec![Cell::default(); cols]);
        self.primary.cols = cols;
        self.primary.rows = rows;
        self.primary.cells = cells;
        self.primary.cursor_row = cursor_row.min(screen_row_count.max(1).saturating_sub(1));
        self.primary.cursor_col = mapped_cursor.1;
        self.primary.saved_cursor_row = self.primary.saved_cursor_row.min(rows.saturating_sub(1));
        self.primary.saved_cursor_col = self.primary.saved_cursor_col.min(cols.saturating_sub(1));
        self.primary.wrap_pending = mapped_cursor.2;
        self.primary.scroll_top = 0;
        self.primary.scroll_bottom = rows.saturating_sub(1);
    }

    pub(crate) fn set_cell_metrics(&mut self, width: f32, height: f32) {
        self.cell_width = finite_metric(width);
        self.cell_height = finite_metric(height);
    }

    pub(crate) fn text_area_size_pixels(&self) -> (u32, u32) {
        let width = (self.cols() as f32 * self.cell_width)
            .round()
            .clamp(1.0, u32::MAX as f32) as u32;
        let height = (self.rows() as f32 * self.cell_height)
            .round()
            .clamp(1.0, u32::MAX as f32) as u32;
        (height, width)
    }

    pub(crate) fn set_history_limit(&mut self, limit: usize) {
        let history_before = self.history.len();
        self.history_limit = limit.min(MAX_SCROLLBACK_LINES);
        while self.history.len() > self.history_limit {
            self.history.pop_front();
        }
        let dropped = history_before.saturating_sub(self.history.len());
        if dropped > 0 {
            self.effects
                .push_back(GridEffect::RebaseHistory { dropped });
        }
        self.display_offset = self.display_offset.min(self.history.len());
        self.damage.mark_full();
    }

    pub(crate) fn set_default_cursor_style(&mut self, style: CursorStyle) {
        if !self.cursor_style_explicit {
            self.cursor_style = style;
            self.cursor_shape_status = cursor_shape_status(style);
        }
        self.default_cursor_style = style;
        self.damage.mark_full();
    }

    pub(crate) fn scroll_display(&mut self, delta_lines: i32) -> bool {
        if self.alternate_active || delta_lines == 0 {
            return false;
        }
        let old = self.display_offset;
        if delta_lines > 0 {
            self.display_offset = self
                .display_offset
                .saturating_add(delta_lines as usize)
                .min(self.history.len());
        } else {
            self.display_offset = self
                .display_offset
                .saturating_sub(delta_lines.unsigned_abs() as usize);
        }
        if old != self.display_offset {
            self.damage.mark_full();
            true
        } else {
            false
        }
    }

    pub(crate) fn scroll_to_bottom(&mut self) -> bool {
        if self.display_offset == 0 {
            return false;
        }
        self.display_offset = 0;
        self.damage.mark_full();
        true
    }

    pub(crate) fn clear_history(&mut self) -> bool {
        if self.alternate_active {
            return false;
        }
        if self.history.is_empty() && self.display_offset == 0 {
            return false;
        }
        let dropped = self.history.len();
        self.history.clear();
        self.display_offset = 0;
        if dropped > 0 {
            self.effects
                .push_back(GridEffect::RebaseHistory { dropped });
        }
        self.damage.mark_full();
        true
    }

    pub(crate) fn pop_effect(&mut self) -> Option<GridEffect> {
        self.effects.pop_front()
    }

    pub(crate) fn take_damage(&mut self) -> DamageSnapshot {
        self.damage.take()
    }

    pub(crate) fn mark_full_damage(&mut self) {
        self.damage.mark_full();
    }

    pub(crate) fn snapshot(&self) -> Snapshot {
        let mut cells = Vec::with_capacity(self.cols().saturating_mul(self.rows()));
        self.for_each_viewport_cell(|_, _, _, cell, _| cells.push(*cell));
        Snapshot {
            cols: self.cols() as u16,
            rows: self.rows() as u16,
            cells,
            cursor: self.cursor_state(),
            display_offset: self.display_offset(),
            history_size: self.history_size(),
        }
    }

    pub(crate) fn for_each_viewport_cell(
        &self,
        mut visitor: impl FnMut(usize, i32, usize, &Cell, Option<Combining<'_>>),
    ) -> usize {
        let offset = self.display_offset();
        let rows = self.rows();
        let cols = self.cols();
        for row in 0..rows {
            let term_line = row as i32 - offset as i32;
            if let Some(line) = self.line(term_line) {
                for (col, cell) in line.iter().enumerate().take(cols) {
                    visitor(offset, term_line, col, cell, self.combining_text(cell));
                }
            }
        }
        offset
    }

    pub(crate) fn with_viewport_cell<R>(
        &self,
        row: usize,
        col: usize,
        f: impl FnOnce(usize, i32, &Cell) -> R,
    ) -> Option<R> {
        if row >= self.rows() || col >= self.cols() {
            return None;
        }
        let offset = self.display_offset();
        let term_line = row as i32 - offset as i32;
        let cell = self.line(term_line)?.get(col)?;
        Some(f(offset, term_line, cell))
    }

    pub(crate) fn hyperlink_at(&self, row: usize, col: usize) -> Option<Hyperlink> {
        if row >= self.rows() || col >= self.cols() {
            return None;
        }
        let line_index = row as i32 - self.display_offset() as i32;
        let line = self.line(line_index)?;
        let id = line.get(col)?.hyperlink_id?;
        let mut start_col = col;
        while start_col > 0 && line[start_col - 1].hyperlink_id == Some(id) {
            start_col -= 1;
        }
        let mut end_col = col;
        while end_col + 1 < line.len() && line[end_col + 1].hyperlink_id == Some(id) {
            end_col += 1;
        }
        Some(Hyperlink {
            start_col,
            end_col,
            target: self.hyperlinks.get(&id.get())?.clone(),
        })
    }

    pub(crate) fn for_each_viewport_range(
        &self,
        ranges: impl IntoIterator<Item = (usize, usize, usize)>,
        mut visitor: impl FnMut(usize, usize, i32, usize, &Cell, Option<Combining<'_>>),
    ) -> usize {
        let offset = self.display_offset();
        let rows = self.rows();
        let cols = self.cols();
        for (row, left, right) in ranges {
            if row >= rows || left >= cols || left > right {
                continue;
            }
            let term_line = row as i32 - offset as i32;
            let Some(line) = self.line(term_line) else {
                continue;
            };
            let right = right.min(cols.saturating_sub(1));
            for col in left..=right {
                if let Some(cell) = line.get(col) {
                    visitor(row, offset, term_line, col, cell, self.combining_text(cell));
                }
            }
        }
        offset
    }

    pub(crate) fn line_bounds(&self) -> (i32, i32) {
        if self.alternate_active {
            (0, self.rows().saturating_sub(1) as i32)
        } else {
            (
                -(self.history.len() as i32),
                self.rows().saturating_sub(1) as i32,
            )
        }
    }

    pub(crate) fn line(&self, line: i32) -> Option<&[Cell]> {
        if self.alternate_active {
            let row = usize::try_from(line).ok()?;
            return (row < self.alternate.rows).then(|| self.alternate.row(row));
        }
        if line < 0 {
            let index = self.history.len() as i64 + i64::from(line);
            return usize::try_from(index)
                .ok()
                .and_then(|index| self.history.get(index))
                .map(Vec::as_slice);
        }
        let row = usize::try_from(line).ok()?;
        (row < self.primary.rows).then(|| self.primary.row(row))
    }

    pub(crate) fn for_each_line_cell(
        &self,
        line: i32,
        mut visitor: impl FnMut(usize, &Cell, Option<Combining<'_>>),
    ) -> bool {
        let Some(cells) = self.line(line) else {
            return false;
        };
        for (col, cell) in cells.iter().enumerate() {
            visitor(col, cell, self.combining_text(cell));
        }
        true
    }

    fn combining_text(&self, cell: &Cell) -> Option<Combining<'_>> {
        let id = cell.combining_id?;
        inline_combining_character(id)
            .map(Combining::Character)
            .or_else(|| {
                self.combining
                    .get(&id)
                    .map(|text| Combining::Text(text.as_str()))
            })
    }
}

fn inline_combining_id(character: char) -> NonZeroU32 {
    NonZeroU32::new(character as u32 + 1).expect("Unicode scalar values produce nonzero IDs")
}

fn inline_combining_character(id: NonZeroU32) -> Option<char> {
    (id.get() & POOLED_COMBINING_BIT == 0)
        .then(|| char::from_u32(id.get() - 1))
        .flatten()
}

fn is_pooled_combining_id(id: NonZeroU32) -> bool {
    id.get() & POOLED_COMBINING_BIT != 0
}

fn default_tab_stops(cols: usize) -> Vec<bool> {
    (0..cols).map(|col| col % 8 == 0).collect()
}

fn finite_metric(value: f32) -> f32 {
    if value.is_finite() {
        value.max(1.0)
    } else {
        1.0
    }
}

fn cell_is_empty_for_viewport(cell: &Cell) -> bool {
    matches!(cell.character, ' ' | '\t')
        && cell.combining_id.is_none()
        && cell.foreground == Color::Default
        && cell.background == Color::Default
        && !cell.attributes.inverse()
        && !cell.attributes.underline()
        && !cell.attributes.strikethrough()
        && !cell.wide_spacer()
        && !cell.leading_wide_spacer()
        && !cell.wrapped()
}

fn flush_reflow_line(
    logical: &mut Vec<Cell>,
    cols: usize,
    output: &mut VecDeque<Vec<Cell>>,
    cursor_offset: Option<(usize, bool, bool)>,
    shrinking: bool,
    trailing_empty_continuation: bool,
) -> Option<(usize, usize, bool)> {
    let first_output_row = output.len();
    if logical.is_empty() {
        output.push_back(vec![Cell::default(); cols]);
        return cursor_offset.map(|_| (first_output_row, 0, false));
    }

    let mut cursor = None;
    let mut index = 0usize;
    let logical_len = logical.len();
    let mut final_row = first_output_row;
    let mut final_col = 0usize;
    let mut wide_wrap_before_cursor = false;
    while index < logical.len() {
        let row_index = output.len();
        let mut row = vec![Cell::default(); cols];
        let mut col = 0usize;
        while col < cols && index < logical.len() {
            let wide_character_hits_edge = cols > 1
                && col + 1 == cols
                && index + 1 < logical.len()
                && logical[index + 1].wide_spacer()
                && !logical[index].wide_spacer();
            if wide_character_hits_edge {
                wide_wrap_before_cursor |=
                    cursor_offset.is_some_and(|(offset, _, _)| offset > index);
                let mut placeholder = logical[index + 1];
                placeholder.set_wide_spacer(false);
                placeholder.set_leading_wide_spacer(true);
                row[col] = placeholder;
                break;
            }
            let physical_cursor_reflow = shrinking
                && wide_wrap_before_cursor
                && cursor_offset.is_some_and(|(_, _, starts_logical)| starts_logical);
            if !physical_cursor_reflow
                && cursor_offset.is_some_and(|(offset, _, _)| offset == index)
            {
                cursor = Some((row_index, col, false));
            }
            row[col] = logical[index];
            index += 1;
            col += 1;
        }
        if index < logical.len() {
            row[cols.saturating_sub(1)].set_wrapped(true);
        }
        final_row = row_index;
        final_col = col;
        output.push_back(row);
    }
    let preserved_empty_continuation =
        !shrinking && trailing_empty_continuation && final_col == cols;
    if preserved_empty_continuation {
        if let Some(row) = output.back_mut() {
            row[cols.saturating_sub(1)].set_wrapped(true);
        }
        output.push_back(vec![Cell::default(); cols]);
    }
    if cursor.is_none()
        && (shrinking
            && wide_wrap_before_cursor
            && cursor_offset.is_some_and(|(_, _, starts_logical)| starts_logical)
            || cursor_offset.is_some_and(|(offset, _, _)| offset == logical_len))
    {
        cursor = if shrinking
            && wide_wrap_before_cursor
            && cursor_offset.is_some_and(|(_, _, starts_logical)| starts_logical)
        {
            let (offset, force_continuation, _) =
                cursor_offset.expect("cursor offset was checked above");
            let row_offset = offset / cols;
            let col = offset % cols;
            if col == 0 && offset > 0 && !force_continuation {
                Some((
                    first_output_row + row_offset.saturating_sub(1),
                    cols.saturating_sub(1),
                    true,
                ))
            } else {
                let row = first_output_row + row_offset;
                if row == output.len() {
                    if let Some(previous) = output.back_mut() {
                        previous[cols.saturating_sub(1)].set_wrapped(true);
                    }
                    output.push_back(vec![Cell::default(); cols]);
                }
                Some((row, col, false))
            }
        } else if preserved_empty_continuation && cursor_offset.is_some_and(|(_, force, _)| force) {
            Some((final_row + 1, 0, false))
        } else if final_col == cols && cursor_offset.is_some_and(|(_, force, _)| force) {
            if let Some(row) = output.back_mut() {
                row[cols.saturating_sub(1)].set_wrapped(true);
            }
            let row = output.len();
            output.push_back(vec![Cell::default(); cols]);
            Some((row, 0, false))
        } else if final_col == cols {
            Some((final_row, cols.saturating_sub(1), true))
        } else {
            Some((final_row, final_col, false))
        };
    }
    logical.clear();
    cursor
}

fn push_sgr_color(codes: &mut Vec<String>, color: Color, foreground: bool) {
    match color {
        Color::Default => {}
        Color::Indexed(index @ 0..=7) => {
            let base = if foreground { 30 } else { 40 };
            codes.push((base + u16::from(index)).to_string());
        }
        Color::Indexed(index @ 8..=15) => {
            let base = if foreground { 90 } else { 100 };
            codes.push((base + u16::from(index - 8)).to_string());
        }
        Color::Indexed(index) => {
            codes.push(format!("{};5;{index}", if foreground { 38 } else { 48 }));
        }
        Color::Rgb { r, g, b } => {
            codes.push(format!(
                "{};2;{r};{g};{b}",
                if foreground { 38 } else { 48 }
            ));
        }
    }
}

fn extended_color(params: &[u16], index: &mut usize) -> Option<Color> {
    *index = index.saturating_add(1);
    match params.get(*index).copied()? {
        5 => {
            *index = index.saturating_add(1);
            Some(Color::Indexed(u8::try_from(*params.get(*index)?).ok()?))
        }
        2 => {
            *index = index.saturating_add(1);
            let r = u8::try_from(*params.get(*index)?).ok()?;
            *index = index.saturating_add(1);
            let g = u8::try_from(*params.get(*index)?).ok()?;
            *index = index.saturating_add(1);
            let b = u8::try_from(*params.get(*index)?).ok()?;
            Some(Color::Rgb { r, g, b })
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn grid(cols: u16, rows: u16) -> Grid {
        Grid::new(cols, rows, 3, CursorStyle::Block)
    }

    #[test]
    fn cells_remain_compact_with_sparse_combining_storage() {
        assert_eq!(std::mem::size_of::<Attributes>(), 1);
        assert_eq!(std::mem::size_of::<Cell>(), 24);
    }

    #[test]
    fn packed_attribute_and_cell_state_flags_are_independent() {
        let attributes = [
            (Attributes::default().with_bold(true), Attributes::BOLD),
            (Attributes::default().with_dim(true), Attributes::DIM),
            (Attributes::default().with_italic(true), Attributes::ITALIC),
            (
                Attributes::default().with_underline(true),
                Attributes::UNDERLINE,
            ),
            (
                Attributes::default().with_inverse(true),
                Attributes::INVERSE,
            ),
            (Attributes::default().with_hidden(true), Attributes::HIDDEN),
            (
                Attributes::default().with_strikethrough(true),
                Attributes::STRIKETHROUGH,
            ),
        ];
        for (attributes, expected) in attributes {
            assert_eq!(attributes.0, expected);
        }

        let mut attributes = Attributes::default();
        attributes.set_bold(true);
        attributes.set_dim(true);
        attributes.set_italic(true);
        attributes.set_underline(true);
        attributes.set_inverse(true);
        attributes.set_hidden(true);
        attributes.set_strikethrough(true);
        assert!(attributes.bold());
        assert!(attributes.dim());
        assert!(attributes.italic());
        assert!(attributes.underline());
        assert!(attributes.inverse());
        assert!(attributes.hidden());
        assert!(attributes.strikethrough());
        attributes.set_bold(false);
        assert!(!attributes.bold());
        assert!(attributes.dim());

        let state_flags: [(fn(&mut Cell), u8); 4] = [
            (
                |cell: &mut Cell| cell.set_protected(true),
                CellState::PROTECTED,
            ),
            (
                |cell: &mut Cell| cell.set_wide_spacer(true),
                CellState::WIDE_SPACER,
            ),
            (
                |cell: &mut Cell| cell.set_leading_wide_spacer(true),
                CellState::LEADING_WIDE_SPACER,
            ),
            (|cell: &mut Cell| cell.set_wrapped(true), CellState::WRAPPED),
        ];
        for (set, expected) in state_flags {
            let mut cell = Cell::default();
            set(&mut cell);
            assert_eq!(cell.state.0, expected);
        }
    }

    #[test]
    fn ascii_runs_match_scalar_writes_across_wrap_and_scrollback() {
        let mut fast = Grid::new(5, 2, 4, CursorStyle::Block);
        let mut scalar = Grid::new(5, 2, 4, CursorStyle::Block);
        assert_eq!(fast.take_damage(), DamageSnapshot::Full);
        assert_eq!(scalar.take_damage(), DamageSnapshot::Full);
        let bytes = b"abcdefghijklmnop";

        let mut offset = 0;
        while offset < bytes.len() {
            let consumed = fast.put_ascii_run(&bytes[offset..]);
            if consumed == 0 {
                fast.put_char(char::from(bytes[offset]));
                offset += 1;
            } else {
                offset += consumed;
            }
        }
        for byte in bytes {
            scalar.put_char(char::from(*byte));
        }

        assert_eq!(fast.primary.cells, scalar.primary.cells);
        assert_eq!(fast.history, scalar.history);
        assert_eq!(fast.cursor_position(), scalar.cursor_position());
        assert_eq!(fast.primary.wrap_pending, scalar.primary.wrap_pending);
        assert_eq!(fast.take_damage(), scalar.take_damage());
    }

    #[test]
    fn ascii_runs_stop_before_cells_that_need_wide_cleanup() {
        let mut grid = Grid::new(6, 2, 0, CursorStyle::Block);
        grid.set_cursor_position(0, 2);
        grid.put_char('界');
        grid.set_cursor_position(0, 0);

        assert_eq!(grid.put_ascii_run(b"abcdef"), 2);
        assert_eq!(grid.line(0).unwrap()[0].character, 'a');
        assert_eq!(grid.line(0).unwrap()[1].character, 'b');
        assert_eq!(grid.line(0).unwrap()[2].character, '界');
        assert!(grid.line(0).unwrap()[3].wide_spacer());
    }

    #[test]
    fn combining_characters_attach_to_the_previous_cell() {
        let mut grid = grid(4, 2);
        grid.put_char('e');
        grid.put_char('\u{301}');
        grid.put_char('\u{308}');

        let cell = &grid.line(0).unwrap()[0];
        assert_eq!(cell.character, 'e');
        assert_eq!(
            grid.combining_text(cell).map(Combining::to_owned_string),
            Some("\u{301}\u{308}".to_string())
        );
        assert_eq!(grid.cursor_position(), (1, 0));
    }

    #[test]
    fn long_combining_sequences_spill_out_of_inline_storage_without_data_loss() {
        let mut grid = grid(4, 2);
        grid.put_char('e');
        let combining = "\u{301}\u{302}\u{303}\u{304}\u{305}\u{306}\u{307}\u{308}\u{309}";
        for character in combining.chars() {
            grid.put_char(character);
        }

        let cell = &grid.line(0).unwrap()[0];
        assert_eq!(
            grid.combining_text(cell).map(Combining::to_owned_string),
            Some(combining.to_string())
        );
    }

    #[test]
    fn combining_characters_attach_to_the_lead_cell_of_wide_text() {
        let mut grid = grid(4, 2);
        grid.put_char('界');
        grid.put_char('\u{301}');

        let line = grid.line(0).unwrap();
        assert_eq!(
            grid.combining_text(&line[0])
                .map(Combining::to_owned_string),
            Some("\u{301}".to_string())
        );
        assert_eq!(grid.combining_text(&line[1]), None);
    }

    #[test]
    fn overwriting_a_cell_drops_its_combining_characters() {
        let mut grid = grid(4, 2);
        grid.put_char('e');
        grid.put_char('\u{301}');
        grid.carriage_return();
        grid.put_char('x');

        let cell = &grid.line(0).unwrap()[0];
        assert_eq!(cell.character, 'x');
        assert_eq!(grid.combining_text(cell), None);
    }

    #[test]
    fn overwriting_a_wide_spacer_drops_combining_text_from_its_lead_cell() {
        let mut grid = grid(4, 2);
        grid.put_char('界');
        grid.put_char('\u{301}');
        grid.backspace();
        grid.put_char('x');

        let line = grid.line(0).unwrap();
        assert_eq!(line[0].character, ' ');
        assert_eq!(grid.combining_text(&line[0]), None);
        assert_eq!(line[1].character, 'x');
    }

    #[test]
    fn live_default_cursor_changes_do_not_override_an_explicit_shape() {
        let mut grid = grid(3, 2);
        grid.set_cursor_style(2);
        grid.set_default_cursor_style(CursorStyle::Line);
        assert_eq!(grid.cursor_state().unwrap().style, CursorStyle::Block);
        assert_eq!(grid.cursor_style_status(), 2);

        grid.set_cursor_style(0);
        grid.set_default_cursor_style(CursorStyle::Block);
        assert_eq!(grid.cursor_state().unwrap().style, CursorStyle::Block);
        assert_eq!(grid.cursor_style_status(), 2);
    }

    #[test]
    fn wrapping_moves_full_rows_into_bounded_history() {
        let mut grid = grid(3, 2);
        for character in "abcdefghijklmnop".chars() {
            grid.put_char(character);
        }
        assert_eq!(grid.history_size(), 3);
        assert_eq!(grid.cursor_position(), (1, 1));
        assert_eq!(grid.line(-3).unwrap()[0].character, 'd');
    }

    #[test]
    fn alternate_screen_does_not_touch_primary_content() {
        let mut grid = grid(6, 2);
        for character in "main".chars() {
            grid.put_char(character);
        }
        grid.sgr(&[32, 44]);
        grid.set_mode(true, 1049, true);
        assert_eq!(grid.cursor_position(), (4, 0));
        assert_eq!(grid.line(0).unwrap()[0].background, Color::Indexed(4));
        grid.put_char('x');
        assert_eq!(grid.line(0).unwrap()[4].character, 'x');
        assert_eq!(grid.line(0).unwrap()[4].foreground, Color::Indexed(2));
        assert_eq!(grid.line(0).unwrap()[4].background, Color::Indexed(4));
        grid.set_mode(true, 1049, false);
        assert_eq!(grid.line(0).unwrap()[0].character, 'm');
        assert_eq!(grid.cursor_position(), (4, 0));
    }

    #[test]
    fn damage_is_scoped_after_initial_full_frame() {
        let mut grid = grid(8, 2);
        assert_eq!(grid.take_damage(), DamageSnapshot::Full);
        grid.put_char('x');
        assert_eq!(
            grid.take_damage(),
            DamageSnapshot::Partial(vec![DirtySpan {
                row: 0,
                left_col: 0,
                right_col: 0,
            }])
        );
    }

    #[test]
    fn full_damage_discards_stale_partial_spans_before_the_next_frame() {
        let mut damage = Damage::new(2);
        assert_eq!(damage.take(), DamageSnapshot::Full);
        damage.mark(1, 2, 4);
        damage.mark_full();
        assert_eq!(damage.take(), DamageSnapshot::Full);
        assert_eq!(damage.take(), DamageSnapshot::Partial(Vec::new()));
    }

    #[test]
    fn width_resize_reflows_soft_wrapped_content_and_cursor() {
        let mut grid = Grid::new(5, 3, 16, CursorStyle::Block);
        for character in "abcdefghij".chars() {
            grid.put_char(character);
        }

        grid.resize(3, 4);

        let rendered = (0..4)
            .map(|line| {
                grid.line(line)
                    .unwrap()
                    .iter()
                    .map(|cell| cell.character)
                    .collect::<String>()
            })
            .collect::<Vec<_>>();
        assert_eq!(rendered, ["abc", "def", "ghi", "j  "]);
        assert_eq!(grid.cursor_position(), (1, 3));
        assert!(grid.line(0).unwrap()[2].wrapped());
        assert!(grid.line(1).unwrap()[2].wrapped());
        assert!(grid.line(2).unwrap()[2].wrapped());
        assert!(!grid.line(3).unwrap()[2].wrapped());
    }

    #[test]
    fn width_resize_keeps_wide_character_and_spacer_together() {
        let mut grid = Grid::new(5, 3, 16, CursorStyle::Block);
        for character in "abc界".chars() {
            grid.put_char(character);
        }

        grid.resize(4, 3);

        assert_eq!(
            grid.line(0).unwrap()[..3]
                .iter()
                .map(|cell| cell.character)
                .collect::<String>(),
            "abc"
        );
        assert!(grid.line(0).unwrap()[3].leading_wide_spacer());
        assert!(grid.line(0).unwrap()[3].wrapped());
        assert_eq!(grid.line(1).unwrap()[0].character, '界');
        assert!(grid.line(1).unwrap()[1].wide_spacer());

        grid.resize(8, 3);
        assert_eq!(grid.line(0).unwrap()[3].character, '界');
        assert!(grid.line(0).unwrap()[4].wide_spacer());
    }

    #[test]
    fn rendered_cursor_uses_the_leading_cell_of_a_wide_character() {
        let mut grid = Grid::new(4, 2, 0, CursorStyle::Block);
        grid.set_cursor_col(2);
        grid.put_char('界');

        assert_eq!(grid.cursor_position(), (3, 0));
        assert_eq!(grid.render_cursor_position(), (2, 0));
        assert_eq!(grid.cursor_state().unwrap().col, 2);
    }

    #[test]
    fn wide_character_at_the_margin_styles_the_wrapped_placeholder() {
        let mut grid = Grid::new(4, 2, 0, CursorStyle::Block);
        grid.sgr(&[31, 44]);
        grid.set_cursor_col(3);
        grid.put_char('界');

        let placeholder = grid.line(0).unwrap()[3];
        assert_eq!(placeholder.character, ' ');
        assert_eq!(placeholder.foreground, Color::Indexed(1));
        assert_eq!(placeholder.background, Color::Indexed(4));
        assert!(placeholder.leading_wide_spacer());
        assert!(placeholder.wrapped());
        assert_eq!(grid.line(1).unwrap()[0].character, '界');
        assert!(grid.line(1).unwrap()[1].wide_spacer());
    }

    #[test]
    fn insert_mode_rotates_and_clears_a_displaced_wide_spacer_like_alacritty() {
        let mut grid = Grid::new(12, 2, 0, CursorStyle::Block);
        grid.set_cursor_col(9);
        grid.put_char('界');
        grid.put_char('q');
        grid.set_cursor_col(0);
        grid.set_mode(false, 4, true);

        grid.put_char('a');
        grid.put_char('b');

        assert_eq!(grid.line(0).unwrap()[0].character, ' ');
        assert_eq!(grid.line(0).unwrap()[1].character, 'b');
    }

    #[test]
    fn deleting_past_the_margin_clears_the_requested_tail_width() {
        let mut grid = Grid::new(12, 2, 0, CursorStyle::Block);
        grid.set_cursor_col(8);
        for character in "abc".chars() {
            grid.put_char(character);
        }

        grid.delete_chars(3);

        assert_eq!(
            grid.line(0).unwrap()[8..12]
                .iter()
                .map(|cell| cell.character)
                .collect::<String>(),
            "a   "
        );
    }

    #[test]
    fn saved_cursor_restores_pending_wrap() {
        let mut grid = Grid::new(3, 2, 0, CursorStyle::Block);
        for character in "abc".chars() {
            grid.put_char(character);
        }
        grid.save_cursor();
        grid.put_char('d');
        grid.restore_cursor();
        grid.put_char('e');

        assert_eq!(grid.line(0).unwrap()[2].character, 'c');
        assert_eq!(grid.line(1).unwrap()[0].character, 'e');
    }

    #[test]
    fn explicit_line_feed_keeps_pending_wrap() {
        let mut grid = Grid::new(3, 3, 0, CursorStyle::Block);
        for character in "abc".chars() {
            grid.put_char(character);
        }

        grid.line_feed();
        assert_eq!(grid.cursor_position(), (2, 1));
        grid.put_char('d');

        assert_eq!(grid.line(0).unwrap()[2].character, 'c');
        assert_eq!(grid.line(2).unwrap()[0].character, 'd');
    }

    #[test]
    fn backspace_in_a_single_column_keeps_pending_wrap() {
        let mut grid = Grid::new(1, 2, 0, CursorStyle::Block);
        grid.put_char('a');

        grid.backspace();
        grid.put_char('b');

        assert_eq!(grid.line(0).unwrap()[0].character, 'a');
        assert_eq!(grid.line(1).unwrap()[0].character, 'b');
    }

    #[test]
    fn clearing_the_primary_viewport_moves_visible_rows_into_history() {
        let mut grid = Grid::new(4, 3, 8, CursorStyle::Block);
        grid.put_char('a');
        grid.set_cursor_position(2, 0);
        grid.put_char('b');

        grid.erase_display(2, false);

        assert_eq!(grid.history_size(), 3);
        assert_eq!(grid.line(-3).unwrap()[0].character, 'a');
        assert_eq!(grid.line(-1).unwrap()[0].character, 'b');
        assert!(
            grid.line(0)
                .unwrap()
                .iter()
                .all(|cell| cell.character == ' ')
        );
    }

    #[test]
    fn clearing_an_empty_primary_viewport_seeds_scrollback_once() {
        let mut grid = Grid::new(4, 3, 8, CursorStyle::Block);

        grid.erase_display(2, false);
        assert_eq!(grid.history_size(), 1);

        grid.erase_display(2, false);
        assert_eq!(grid.history_size(), 1);
    }

    #[test]
    fn scroll_region_survives_alternate_screen_entry() {
        let mut grid = Grid::new(4, 4, 0, CursorStyle::Block);
        grid.set_scroll_region(1, 2);

        grid.set_mode(true, 1049, true);

        assert_eq!(grid.scroll_region_status(), (2, 3));
    }

    #[test]
    fn clearing_history_on_the_alternate_screen_keeps_primary_scrollback() {
        let mut grid = Grid::new(4, 2, 8, CursorStyle::Block);
        grid.put_char('a');
        grid.next_line();
        grid.next_line();
        assert_eq!(grid.history_size(), 1);

        grid.set_mode(true, 1049, true);
        grid.erase_display(3, false);
        grid.set_mode(true, 1049, false);

        assert_eq!(grid.history_size(), 1);
        assert_eq!(grid.line(-1).unwrap()[0].character, 'a');
    }

    #[test]
    fn alternate_saved_cursor_survives_screen_reentry() {
        let mut grid = Grid::new(4, 4, 0, CursorStyle::Block);
        grid.set_mode(true, 1049, true);
        grid.set_cursor_position(1, 2);
        grid.save_cursor();
        grid.set_mode(true, 1049, false);

        grid.set_cursor_position(3, 3);
        grid.set_mode(true, 1049, true);
        grid.restore_cursor();

        assert_eq!(grid.cursor_position(), (2, 1));
    }

    #[test]
    fn row_growth_pulls_recent_history_before_adding_blank_rows() {
        let mut grid = Grid::new(8, 2, 8, CursorStyle::Block);
        for line in ["one", "two", "three"] {
            for character in line.chars() {
                grid.put_char(character);
            }
            grid.next_line();
        }
        assert_eq!(grid.history_size(), 2);

        grid.resize(8, 4);

        assert_eq!(grid.history_size(), 0);
        assert_eq!(grid.cursor_position(), (0, 3));
        let rendered = (0..4)
            .map(|line| {
                grid.line(line)
                    .unwrap()
                    .iter()
                    .map(|cell| cell.character)
                    .collect::<String>()
            })
            .collect::<Vec<_>>();
        assert_eq!(rendered, ["one     ", "two     ", "three   ", "        "]);
    }

    #[test]
    fn row_shrink_scrolls_only_enough_to_keep_the_cursor_visible() {
        let mut grid = Grid::new(4, 4, 2, CursorStyle::Block);
        for (row, character) in ['a', 'b', 'c', 'd'].into_iter().enumerate() {
            grid.set_cursor_position(row, 0);
            grid.put_char(character);
        }
        grid.set_cursor_position(2, 1);

        grid.resize(4, 2);

        assert_eq!(grid.history_size(), 1);
        assert_eq!(grid.line(-1).unwrap()[0].character, 'a');
        assert_eq!(grid.line(0).unwrap()[0].character, 'b');
        assert_eq!(grid.line(1).unwrap()[0].character, 'c');
        assert_eq!(grid.cursor_position(), (1, 1));
    }

    #[test]
    fn row_shrink_without_scrollback_discards_scrolled_rows() {
        let mut grid = Grid::new(4, 4, 0, CursorStyle::Block);
        for (row, character) in ['a', 'b', 'c', 'd'].into_iter().enumerate() {
            grid.set_cursor_position(row, 0);
            grid.put_char(character);
        }
        grid.set_cursor_position(3, 0);

        grid.resize(4, 2);

        assert_eq!(grid.history_size(), 0);
        assert_eq!(grid.line(0).unwrap()[0].character, 'c');
        assert_eq!(grid.line(1).unwrap()[0].character, 'd');
        assert_eq!(grid.cursor_position(), (0, 1));
    }

    #[test]
    fn deleting_lines_from_the_top_records_primary_scrollback() {
        let mut grid = Grid::new(4, 2, 4, CursorStyle::Block);
        grid.put_char('a');
        grid.set_cursor_position(1, 0);
        grid.put_char('b');
        grid.set_cursor_position(0, 0);

        grid.delete_lines(1);

        assert_eq!(grid.history_size(), 1);
        assert_eq!(grid.line(-1).unwrap()[0].character, 'a');
        assert_eq!(grid.line(0).unwrap()[0].character, 'b');
    }

    #[test]
    fn multi_row_shifts_preserve_order_through_history_rollover() {
        let mut grid = Grid::new(2, 4, 3, CursorStyle::Block);
        for (row, character) in ['a', 'b', 'c', 'd'].into_iter().enumerate() {
            grid.primary.cells[row][0].character = character;
        }

        grid.scroll_up(2);
        assert_eq!(grid.history_size(), 2);
        assert_eq!(grid.line(-2).unwrap()[0].character, 'a');
        assert_eq!(grid.line(-1).unwrap()[0].character, 'b');
        assert_eq!(grid.line(0).unwrap()[0].character, 'c');
        assert_eq!(grid.line(1).unwrap()[0].character, 'd');

        grid.primary.cells[2][0].character = 'e';
        grid.primary.cells[3][0].character = 'f';
        grid.scroll_up(2);
        assert_eq!(grid.history_size(), 3);
        assert_eq!(grid.line(-3).unwrap()[0].character, 'b');
        assert_eq!(grid.line(-2).unwrap()[0].character, 'c');
        assert_eq!(grid.line(-1).unwrap()[0].character, 'd');
        assert_eq!(grid.line(0).unwrap()[0].character, 'e');
        assert_eq!(grid.line(1).unwrap()[0].character, 'f');
        assert!(
            grid.line(2)
                .unwrap()
                .iter()
                .all(|cell| *cell == Cell::default())
        );
        assert!(
            grid.line(3)
                .unwrap()
                .iter()
                .all(|cell| *cell == Cell::default())
        );
    }
}
