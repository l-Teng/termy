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

static DEFAULT_CELL: Cell = Cell {
    character: ' ',
    foreground: Color::Default,
    background: Color::Default,
    attributes: Attributes(0),
    underline_color: None,
    state: CellState(0),
    metadata_id: None,
};

impl Default for Cell {
    fn default() -> Self {
        DEFAULT_CELL
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct PlainHistoryCell(u32);

impl PlainHistoryCell {
    const CHARACTER_MASK: u32 = (1 << 21) - 1;
    const STATE_SHIFT: u32 = 21;

    fn from_cell(cell: Cell) -> Self {
        Self(cell.character as u32 | (u32::from(cell.state.0) << Self::STATE_SHIFT))
    }

    fn into_cell(self) -> Cell {
        Cell {
            character: char::from_u32(self.0 & Self::CHARACTER_MASK)
                .expect("packed history stores a valid Unicode scalar"),
            state: CellState((self.0 >> Self::STATE_SHIFT) as u8),
            ..DEFAULT_CELL
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct MetadataHistoryCell {
    character: char,
    metadata_id: Option<NonZeroU32>,
    state: CellState,
}

impl MetadataHistoryCell {
    fn from_cell(cell: Cell) -> Self {
        Self {
            character: cell.character,
            metadata_id: cell.metadata_id,
            state: cell.state,
        }
    }

    fn into_cell(self) -> Cell {
        Cell {
            character: self.character,
            state: self.state,
            metadata_id: self.metadata_id,
            ..DEFAULT_CELL
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
enum HistoryCells {
    Plain(Vec<PlainHistoryCell>),
    Metadata(Vec<MetadataHistoryCell>),
    Dense(Vec<Cell>),
}

impl HistoryCells {
    fn len(&self) -> usize {
        match self {
            Self::Plain(cells) => cells.len(),
            Self::Metadata(cells) => cells.len(),
            Self::Dense(cells) => cells.len(),
        }
    }
}

#[derive(Clone, Copy)]
enum HistoryEncoding {
    Plain,
    Metadata,
    Dense,
}

/// A logical scrollback row with default style and exact-default suffixes omitted.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct HistoryRow {
    cells: HistoryCells,
    width: u16,
    has_pooled_extras: bool,
}

impl HistoryRow {
    fn from_dense(mut dense: Vec<Cell>) -> Self {
        let width = u16::try_from(dense.len()).expect("history rows fit the bounded grid width");
        let stored_len = Self::stored_len_for(&dense, dense.len());
        let (encoding, has_pooled_extras) = Self::encoding(&dense[..stored_len]);
        let cells = match encoding {
            HistoryEncoding::Plain => {
                let mut cells = Vec::with_capacity(stored_len);
                cells.extend(
                    dense[..stored_len]
                        .iter()
                        .copied()
                        .map(PlainHistoryCell::from_cell),
                );
                HistoryCells::Plain(cells)
            }
            HistoryEncoding::Metadata => {
                let mut cells = Vec::with_capacity(stored_len);
                cells.extend(
                    dense[..stored_len]
                        .iter()
                        .copied()
                        .map(MetadataHistoryCell::from_cell),
                );
                HistoryCells::Metadata(cells)
            }
            HistoryEncoding::Dense => {
                dense.truncate(stored_len);
                dense.shrink_to_fit();
                HistoryCells::Dense(dense)
            }
        };
        Self {
            cells,
            width,
            has_pooled_extras,
        }
    }

    fn copy_from_dense(
        dense: &[Cell],
        width: usize,
        stored_len: usize,
        storage: Option<HistoryCells>,
    ) -> Self {
        let width = u16::try_from(width).expect("history rows fit the bounded grid width");
        debug_assert_eq!(stored_len, Self::stored_len_for(dense, usize::from(width)));
        let (encoding, has_pooled_extras) = Self::encoding(&dense[..stored_len]);
        let cells = match encoding {
            HistoryEncoding::Plain => {
                let storage = match storage {
                    Some(HistoryCells::Plain(cells)) => Some(cells),
                    _ => None,
                };
                let mut cells = exact_storage(storage, stored_len);
                cells.extend(
                    dense[..stored_len]
                        .iter()
                        .copied()
                        .map(PlainHistoryCell::from_cell),
                );
                HistoryCells::Plain(cells)
            }
            HistoryEncoding::Metadata => {
                let storage = match storage {
                    Some(HistoryCells::Metadata(cells)) => Some(cells),
                    _ => None,
                };
                let mut cells = exact_storage(storage, stored_len);
                cells.extend(
                    dense[..stored_len]
                        .iter()
                        .copied()
                        .map(MetadataHistoryCell::from_cell),
                );
                HistoryCells::Metadata(cells)
            }
            HistoryEncoding::Dense => {
                let storage = match storage {
                    Some(HistoryCells::Dense(cells)) => Some(cells),
                    _ => None,
                };
                let mut cells = exact_storage(storage, stored_len);
                cells.extend_from_slice(&dense[..stored_len]);
                HistoryCells::Dense(cells)
            }
        };
        Self {
            cells,
            width,
            has_pooled_extras,
        }
    }

    fn stored_len_for(dense: &[Cell], width: usize) -> usize {
        dense
            .iter()
            .take(width)
            .rposition(|cell| *cell != DEFAULT_CELL)
            .map_or(0, |index| index + 1)
    }

    fn encoding(cells: &[Cell]) -> (HistoryEncoding, bool) {
        let mut plain = true;
        let mut dense = false;
        let mut has_pooled_extras = false;
        for cell in cells {
            dense |= cell.foreground != Color::Default
                || cell.background != Color::Default
                || cell.attributes != Attributes(0)
                || cell.underline_color.is_some();
            plain &= cell.metadata_id.is_none();
            has_pooled_extras |= cell.metadata_id.is_some_and(is_pooled_extra_id);
        }
        let encoding = if dense {
            HistoryEncoding::Dense
        } else if plain {
            HistoryEncoding::Plain
        } else {
            HistoryEncoding::Metadata
        };
        (encoding, has_pooled_extras)
    }

    fn len(&self) -> usize {
        usize::from(self.width)
    }

    fn get(&self, index: usize) -> Option<Cell> {
        if index >= self.len() {
            return None;
        }
        Some(match &self.cells {
            HistoryCells::Plain(cells) => cells
                .get(index)
                .map_or(DEFAULT_CELL, |cell| cell.into_cell()),
            HistoryCells::Metadata(cells) => cells
                .get(index)
                .map_or(DEFAULT_CELL, |cell| cell.into_cell()),
            HistoryCells::Dense(cells) => cells.get(index).copied().unwrap_or(DEFAULT_CELL),
        })
    }

    fn for_each(&self, mut visitor: impl FnMut(usize, &Cell)) {
        match &self.cells {
            HistoryCells::Plain(cells) => {
                for (index, cell) in cells.iter().copied().enumerate() {
                    visitor(index, &cell.into_cell());
                }
            }
            HistoryCells::Metadata(cells) => {
                for (index, cell) in cells.iter().copied().enumerate() {
                    visitor(index, &cell.into_cell());
                }
            }
            HistoryCells::Dense(cells) => {
                for (index, cell) in cells.iter().enumerate() {
                    visitor(index, cell);
                }
            }
        }
        for index in self.cells.len()..self.len() {
            visitor(index, &DEFAULT_CELL);
        }
    }

    fn iter(&self) -> RowCellIter<'_> {
        RowView::History(self).iter()
    }

    fn append_to(&self, output: &mut Vec<Cell>, limit: usize) {
        let len = self.len().min(limit);
        let stored = self.cells.len().min(len);
        match &self.cells {
            HistoryCells::Plain(cells) => output.extend(
                cells[..stored]
                    .iter()
                    .copied()
                    .map(PlainHistoryCell::into_cell),
            ),
            HistoryCells::Metadata(cells) => output.extend(
                cells[..stored]
                    .iter()
                    .copied()
                    .map(MetadataHistoryCell::into_cell),
            ),
            HistoryCells::Dense(cells) => output.extend_from_slice(&cells[..stored]),
        }
        output.resize(output.len().saturating_add(len - stored), DEFAULT_CELL);
    }

    fn to_dense(&self) -> Vec<Cell> {
        let mut dense = Vec::with_capacity(self.len());
        self.append_to(&mut dense, self.len());
        dense
    }

    fn into_dense(self) -> Vec<Cell> {
        let width = self.len();
        match self.cells {
            HistoryCells::Plain(cells) => {
                let mut dense = vec![DEFAULT_CELL; width];
                for (target, cell) in dense.iter_mut().zip(cells) {
                    *target = cell.into_cell();
                }
                dense
            }
            HistoryCells::Metadata(cells) => {
                let mut dense = vec![DEFAULT_CELL; width];
                for (target, cell) in dense.iter_mut().zip(cells) {
                    *target = cell.into_cell();
                }
                dense
            }
            HistoryCells::Dense(mut cells) => {
                cells.resize(width, DEFAULT_CELL);
                cells
            }
        }
    }

    fn into_storage(self) -> HistoryCells {
        self.cells
    }

    fn has_pooled_extras(&self) -> bool {
        self.has_pooled_extras
    }

    fn take_leading_wide_spacer(&mut self, index: usize) -> bool {
        let changed = match &mut self.cells {
            HistoryCells::Plain(cells) => cells.get_mut(index).is_some_and(|cell| {
                let changed = cell.into_cell().leading_wide_spacer();
                cell.0 &=
                    !(u32::from(CellState::LEADING_WIDE_SPACER) << PlainHistoryCell::STATE_SHIFT);
                changed
            }),
            HistoryCells::Metadata(cells) => cells.get_mut(index).is_some_and(|cell| {
                let changed = cell.state.contains(CellState::LEADING_WIDE_SPACER);
                cell.state.set(CellState::LEADING_WIDE_SPACER, false);
                changed
            }),
            HistoryCells::Dense(cells) => cells
                .get_mut(index)
                .is_some_and(Cell::take_leading_wide_spacer),
        };
        if changed {
            match &mut self.cells {
                HistoryCells::Plain(cells) => {
                    while cells
                        .last()
                        .is_some_and(|cell| cell.into_cell() == DEFAULT_CELL)
                    {
                        cells.pop();
                    }
                }
                HistoryCells::Metadata(cells) => {
                    while cells
                        .last()
                        .is_some_and(|cell| cell.into_cell() == DEFAULT_CELL)
                    {
                        cells.pop();
                    }
                }
                HistoryCells::Dense(cells) => {
                    while cells.last() == Some(&DEFAULT_CELL) {
                        cells.pop();
                    }
                }
            }
        }
        changed
    }
}

fn exact_storage<T>(storage: Option<Vec<T>>, len: usize) -> Vec<T> {
    let mut storage = storage.unwrap_or_default();
    if storage.capacity() != len {
        Vec::with_capacity(len)
    } else {
        storage.clear();
        storage
    }
}

impl<'a> IntoIterator for &'a HistoryRow {
    type Item = Cell;
    type IntoIter = RowCellIter<'a>;

    fn into_iter(self) -> Self::IntoIter {
        self.iter()
    }
}

/// A non-allocating view over either a live dense row or compact history.
#[derive(Clone, Copy)]
pub(crate) enum RowView<'a> {
    Dense(&'a [Cell]),
    History(&'a HistoryRow),
}

impl<'a> RowView<'a> {
    pub(crate) fn len(self) -> usize {
        match self {
            Self::Dense(cells) => cells.len(),
            Self::History(row) => row.len(),
        }
    }

    pub(crate) fn get(self, index: usize) -> Option<Cell> {
        match self {
            Self::Dense(cells) => cells.get(index).copied(),
            Self::History(row) => row.get(index),
        }
    }

    pub(crate) fn for_each(self, mut visitor: impl FnMut(usize, &Cell)) {
        match self {
            Self::Dense(cells) => {
                for (index, cell) in cells.iter().enumerate() {
                    visitor(index, cell);
                }
            }
            Self::History(row) => row.for_each(visitor),
        }
    }

    pub(crate) fn iter(self) -> RowCellIter<'a> {
        RowCellIter {
            row: self,
            range: 0..self.len(),
        }
    }

    fn append_to(self, output: &mut Vec<Cell>, limit: usize) {
        match self {
            Self::Dense(cells) => output.extend_from_slice(&cells[..cells.len().min(limit)]),
            Self::History(row) => row.append_to(output, limit),
        }
    }

    pub(crate) fn with_dense<R>(self, f: impl FnOnce(&[Cell]) -> R) -> R {
        match self {
            Self::Dense(cells) => f(cells),
            Self::History(row) => match &row.cells {
                HistoryCells::Dense(cells) if cells.len() == row.len() => f(cells),
                _ => f(&row.to_dense()),
            },
        }
    }
}

pub(crate) struct RowCellIter<'a> {
    row: RowView<'a>,
    range: std::ops::Range<usize>,
}

impl Iterator for RowCellIter<'_> {
    type Item = Cell;

    fn next(&mut self) -> Option<Self::Item> {
        self.range.next().and_then(|index| self.row.get(index))
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        self.range.size_hint()
    }
}

impl DoubleEndedIterator for RowCellIter<'_> {
    fn next_back(&mut self) -> Option<Self::Item> {
        self.range.next_back().and_then(|index| self.row.get(index))
    }
}

impl ExactSizeIterator for RowCellIter<'_> {}
impl std::iter::FusedIterator for RowCellIter<'_> {}

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
    history: VecDeque<HistoryRow>,
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
mod screen;
mod view;

#[cfg(test)]
mod test_support;

use screen::Screen;
use view::{inline_hyperlink_metadata_id, is_pooled_extra_id, viewport_link_from_grid_range};

#[cfg(test)]
#[path = "grid_tests.rs"]
mod tests;
