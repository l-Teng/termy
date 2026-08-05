use std::collections::{HashMap, HashSet, VecDeque};
use std::hash::BuildHasher;
use std::num::NonZeroU32;

use crate::unicode_width::character_width;

const MAX_GRID_DIMENSION: u16 = 4096;
const MAX_SCROLLBACK_LINES: usize = 1_000_000;
const KITTY_KEYBOARD_STACK_MAX_DEPTH: usize = 4096;
const HYPERLINK_PRUNE_MIN_LEN: usize = 4096;
const HYPERLINK_PRUNE_INTERVAL: usize = 1024;
const EXTRA_PRUNE_INTERVAL: u32 = 1 << 18;
const INLINE_COMBINING_BYTES: usize = 16;
const METADATA_TAG_MASK: u32 = 0xc000_0000;
const INLINE_HYPERLINK_TAG: u32 = 0x4000_0000;
const POOLED_EXTRA_TAG: u32 = 0x8000_0000;
const METADATA_VALUE_MASK: u32 = !METADATA_TAG_MASK;
const MAX_SCROLL_DAMAGE_OPS: usize = 128;

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

#[derive(Clone, Debug)]
struct CellExtra {
    combining: CombiningText,
    hyperlink_id: Option<NonZeroU32>,
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

    fn push(&mut self, character: char) {
        let mut encoded = [0; 4];
        let encoded = character.encode_utf8(&mut encoded).as_bytes();
        match self {
            Self::Inline { bytes, len } => {
                let current_len = usize::from(*len);
                if current_len + encoded.len() <= INLINE_COMBINING_BYTES {
                    bytes[current_len..current_len + encoded.len()].copy_from_slice(encoded);
                    *len = (current_len + encoded.len()) as u8;
                    return;
                }

                let mut combined = String::with_capacity(current_len + encoded.len());
                combined.push_str(
                    std::str::from_utf8(&bytes[..current_len])
                        .expect("inline combining text is encoded from valid characters"),
                );
                combined.push(character);
                *self = Self::Heap(combined);
            }
            Self::Heap(text) => text.push(character),
        }
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

#[derive(Clone, Copy, Debug)]
pub struct Palette {
    indexed: [Option<Rgb>; 256],
    foreground: Option<Rgb>,
    background: Option<Rgb>,
    cursor: Option<Rgb>,
    revision: u64,
}

impl Default for Palette {
    fn default() -> Self {
        Self {
            indexed: [None; 256],
            foreground: None,
            background: None,
            cursor: None,
            revision: 0,
        }
    }
}

impl PartialEq for Palette {
    fn eq(&self, other: &Self) -> bool {
        self.indexed == other.indexed
            && self.foreground == other.foreground
            && self.background == other.background
            && self.cursor == other.cursor
    }
}

impl Eq for Palette {}

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

    /// Returns the monotonic rendering revision for live palette overrides.
    ///
    /// Revisions are intentionally excluded from [`PartialEq`]: palette
    /// equality describes visible colors, while this value tracks mutation
    /// history for render-cache invalidation.
    pub fn revision(&self) -> u64 {
        self.revision
    }

    fn bump_revision(&mut self) {
        self.revision = self.revision.saturating_add(1);
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum UnderlineStyle {
    #[default]
    None,
    Single,
    Double,
    Curly,
    Dotted,
    Dashed,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[repr(transparent)]
pub struct Attributes(u16);

impl Attributes {
    const BOLD: u16 = 1 << 0;
    const DIM: u16 = 1 << 1;
    const ITALIC: u16 = 1 << 2;
    const INVERSE: u16 = 1 << 3;
    const HIDDEN: u16 = 1 << 4;
    const STRIKETHROUGH: u16 = 1 << 5;
    const UNDERLINE_SHIFT: u32 = 6;
    const UNDERLINE_MASK: u16 = 0b111 << Self::UNDERLINE_SHIFT;

    const fn contains(self, flag: u16) -> bool {
        self.0 & flag != 0
    }

    const fn with_flag(mut self, flag: u16, enabled: bool) -> Self {
        if enabled {
            self.0 |= flag;
        } else {
            self.0 &= !flag;
        }
        self
    }

    fn set_flag(&mut self, flag: u16, enabled: bool) {
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
        !matches!(self.underline_style(), UnderlineStyle::None)
    }

    pub const fn underline_style(self) -> UnderlineStyle {
        match (self.0 & Self::UNDERLINE_MASK) >> Self::UNDERLINE_SHIFT {
            1 => UnderlineStyle::Single,
            2 => UnderlineStyle::Double,
            3 => UnderlineStyle::Curly,
            4 => UnderlineStyle::Dotted,
            5 => UnderlineStyle::Dashed,
            _ => UnderlineStyle::None,
        }
    }

    pub fn set_underline_style(&mut self, style: UnderlineStyle) {
        *self = self.with_underline_style(style);
    }

    pub const fn with_underline_style(mut self, style: UnderlineStyle) -> Self {
        let encoded = match style {
            UnderlineStyle::None => 0,
            UnderlineStyle::Single => 1,
            UnderlineStyle::Double => 2,
            UnderlineStyle::Curly => 3,
            UnderlineStyle::Dotted => 4,
            UnderlineStyle::Dashed => 5,
        };
        self.0 = (self.0 & !Self::UNDERLINE_MASK) | (encoded << Self::UNDERLINE_SHIFT);
        self
    }

    pub fn set_underline(&mut self, enabled: bool) {
        self.set_underline_style(if enabled {
            UnderlineStyle::Single
        } else {
            UnderlineStyle::None
        });
    }

    pub const fn with_underline(self, enabled: bool) -> Self {
        self.with_underline_style(if enabled {
            UnderlineStyle::Single
        } else {
            UnderlineStyle::None
        })
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
    const HAS_HYPERLINK: u8 = 1 << 4;
    const HAS_COMBINING: u8 = 1 << 5;

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
    pub underline_color: Option<Color>,
    state: CellState,
    metadata_id: Option<NonZeroU32>,
}

impl Default for Cell {
    fn default() -> Self {
        Self {
            character: ' ',
            foreground: Color::Default,
            background: Color::Default,
            attributes: Attributes::default(),
            underline_color: None,
            state: CellState::default(),
            metadata_id: None,
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

    pub const fn has_hyperlink(&self) -> bool {
        self.state.contains(CellState::HAS_HYPERLINK)
    }

    pub const fn has_combining(&self) -> bool {
        self.state.contains(CellState::HAS_COMBINING)
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

    fn set_has_hyperlink(&mut self, enabled: bool) {
        self.state.set(CellState::HAS_HYPERLINK, enabled);
    }

    fn set_has_combining(&mut self, enabled: bool) {
        self.state.set(CellState::HAS_COMBINING, enabled);
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DirtySpan {
    pub row: usize,
    pub left_col: usize,
    pub right_col: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ScrollDirection {
    Up,
    Down,
}

/// An ordered row permutation that a renderer can replay before applying dirty spans.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ScrollDamage {
    /// First affected viewport row, inclusive.
    pub top: usize,
    /// Last affected viewport row, inclusive.
    pub bottom: usize,
    /// Rows inserted at one edge and removed at the opposite edge.
    pub count: usize,
    pub direction: ScrollDirection,
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

/// Coherent terminal state captured while visiting the visible viewport.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ViewportMetadata {
    pub cols: u16,
    pub rows: u16,
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

/// A link range returned by a host-provided text classifier.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LinkMatch {
    pub start_col: usize,
    pub end_col: usize,
    pub target: String,
}

/// A link range clipped to the terminal's visible viewport.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ViewportLink {
    pub start_row: usize,
    pub start_col: usize,
    pub end_row: usize,
    pub end_col: usize,
    pub target: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct GridPosition {
    line: i32,
    col: usize,
}

pub(crate) enum LinkCandidate {
    Resolved(ViewportLink),
    Text(TextLinkCandidate),
}

pub(crate) struct TextLinkCandidate {
    characters: Vec<char>,
    hovered_index: usize,
    start: GridPosition,
    display_offset: usize,
    screen_lines: usize,
    columns: usize,
}

impl TextLinkCandidate {
    pub(crate) fn classifier_input(&self) -> (&[char], usize) {
        (&self.characters, self.hovered_index)
    }

    pub(crate) fn resolve(self, link_match: LinkMatch) -> Option<ViewportLink> {
        if link_match.start_col > self.hovered_index
            || link_match.end_col < self.hovered_index
            || link_match.end_col >= self.characters.len()
        {
            return None;
        }

        let start = self.position_at(link_match.start_col)?;
        let end = self.position_at(link_match.end_col)?;
        viewport_link_from_grid_range(
            start,
            end,
            link_match.target,
            self.display_offset,
            self.screen_lines,
            self.columns,
        )
    }

    fn position_at(&self, index: usize) -> Option<GridPosition> {
        if index >= self.characters.len() {
            return None;
        }
        let linear_col = self.start.col.checked_add(index)?;
        let line_offset = i32::try_from(linear_col / self.columns).ok()?;
        Some(GridPosition {
            line: self.start.line.checked_add(line_offset)?,
            col: linear_col % self.columns,
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct HyperlinkEntry {
    protocol_id: Option<String>,
    target: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum GridEffect {
    ScrollUp {
        alternate: bool,
        top: usize,
        bottom: usize,
        count: usize,
        full_screen_region: bool,
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
    Reset,
}

#[derive(Clone, Copy, Debug, Default)]
struct Pen {
    foreground: Color,
    background: Color,
    attributes: Attributes,
    underline_color: Option<Color>,
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
            underline_color: self.underline_color,
            state: CellState::default(),
            metadata_id: self.hyperlink_id.map(inline_hyperlink_metadata_id),
        };
        cell.set_protected(self.protected);
        cell.set_has_hyperlink(self.hyperlink_id.is_some());
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
        removed
    }
}

#[derive(Debug)]
struct Damage {
    full: bool,
    rows: Vec<Option<(usize, usize)>>,
    scrolls: Vec<ScrollDamage>,
}

impl Damage {
    fn new(rows: usize) -> Self {
        Self {
            full: true,
            rows: vec![None; rows],
            scrolls: Vec::with_capacity(8),
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
        if !self.full {
            self.scrolls.clear();
        }
        self.full = true;
    }

    fn resize(&mut self, rows: usize) {
        self.rows.resize(rows, None);
        self.mark_full();
    }

    fn scroll_up(&mut self, top: usize, bottom: usize, count: usize, cols: usize) {
        self.scroll(top, bottom, count, cols, ScrollDirection::Up);
    }

    fn scroll_down(&mut self, top: usize, bottom: usize, count: usize, cols: usize) {
        self.scroll(top, bottom, count, cols, ScrollDirection::Down);
    }

    fn scroll(
        &mut self,
        top: usize,
        bottom: usize,
        count: usize,
        cols: usize,
        direction: ScrollDirection,
    ) {
        if self.full {
            return;
        }
        let Some(height) = bottom.checked_sub(top).and_then(|span| span.checked_add(1)) else {
            self.mark_full();
            return;
        };
        let count = count.min(height);
        if count == 0 {
            return;
        }
        if bottom >= self.rows.len() || cols == 0 {
            self.mark_full();
            return;
        }

        let region = &mut self.rows[top..=bottom];
        match direction {
            ScrollDirection::Up => {
                region.rotate_left(count);
                for slot in &mut region[height - count..] {
                    *slot = Some((0, cols - 1));
                }
            }
            ScrollDirection::Down => {
                region.rotate_right(count);
                for slot in &mut region[..count] {
                    *slot = Some((0, cols - 1));
                }
            }
        }

        if let Some(last) = self.scrolls.last_mut()
            && last.top == top
            && last.bottom == bottom
            && last.direction == direction
        {
            last.count = last.count.saturating_add(count).min(height);
            return;
        }
        if self.scrolls.len() >= MAX_SCROLL_DAMAGE_OPS {
            self.mark_full();
            return;
        }
        self.scrolls.push(ScrollDamage {
            top,
            bottom,
            count,
            direction,
        });
    }

    fn take_spans(&mut self, display_offset: usize) -> Vec<DirtySpan> {
        let viewport_rows = self.rows.len();
        let mut spans = Vec::new();
        for (screen_row, bounds) in self.rows.iter_mut().enumerate() {
            let Some((left_col, right_col)) = bounds.take() else {
                continue;
            };
            let Some(row) = screen_row.checked_add(display_offset) else {
                continue;
            };
            if row < viewport_rows {
                spans.push(DirtySpan {
                    row,
                    left_col,
                    right_col,
                });
            }
        }
        spans
    }

    fn take(&mut self, display_offset: usize) -> DamageSnapshot {
        if !self.scrolls.is_empty() {
            self.mark_full();
        }
        if self.full {
            self.full = false;
            self.rows.fill(None);
            self.scrolls.clear();
            return DamageSnapshot::Full;
        }

        DamageSnapshot::Partial(self.take_spans(display_offset))
    }

    fn take_render(&mut self, display_offset: usize) -> (DamageSnapshot, Vec<ScrollDamage>) {
        if display_offset != 0 && !self.scrolls.is_empty() {
            self.mark_full();
        }
        if self.full {
            self.full = false;
            self.rows.fill(None);
            self.scrolls.clear();
            return (DamageSnapshot::Full, Vec::new());
        }

        let spans = self.take_spans(display_offset);
        let scrolls = std::mem::take(&mut self.scrolls);
        (DamageSnapshot::Partial(spans), scrolls)
    }

    fn clear(&mut self) {
        self.full = false;
        self.rows.fill(None);
        self.scrolls.clear();
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
    hyperlinks: HashMap<u32, HyperlinkEntry>,
    hyperlink_identities: HashMap<u64, u32>,
    next_hyperlink_id: u32,
    next_hyperlink_prune_len: usize,
    extras: HashMap<NonZeroU32, CellExtra>,
    next_extra_id: u32,
    effects: VecDeque<GridEffect>,
}

mod edit;
mod modes;
mod view;

#[cfg(test)]
use view::is_pooled_extra_id;
use view::{inline_hyperlink_metadata_id, viewport_link_from_grid_range};

#[cfg(test)]
#[path = "grid_tests.rs"]
mod tests;
