use compact_str::CompactString;

use crate::runtime::{TerminalCursorState, TerminalDamageSnapshot};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TerminalColor {
    pub r: u8,
    pub g: u8,
    pub b: u8,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TermyColor {
    pub r: u8,
    pub g: u8,
    pub b: u8,
    pub a: u8,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum TerminalRenderColor {
    #[default]
    DefaultForeground,
    DefaultBackground,
    Cursor,
    Indexed(u8),
    DimIndexed(u8),
    BrightForeground,
    DimForeground,
    Rgb(TerminalColor),
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum TerminalUnderlineStyle {
    #[default]
    None,
    Single,
    Double,
    Curly,
    Dotted,
    Dashed,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TerminalRenderText(CompactString);

impl TerminalRenderText {
    pub fn as_str(&self) -> &str {
        self.0.as_str()
    }

    pub(crate) fn from_cell_suffix(base: char, combining: Option<&str>) -> Self {
        let mut text = CompactString::default();
        text.push(base);
        if let Some(combining) = combining {
            text.push_str(combining);
        }
        Self(text)
    }

    #[cfg(test)]
    pub(crate) fn is_heap_allocated(&self) -> bool {
        self.0.is_heap_allocated()
    }
}

impl std::ops::Deref for TerminalRenderText {
    type Target = str;

    fn deref(&self) -> &Self::Target {
        self.as_str()
    }
}

impl PartialEq<str> for TerminalRenderText {
    fn eq(&self, other: &str) -> bool {
        self.as_str() == other
    }
}

impl PartialEq<&str> for TerminalRenderText {
    fn eq(&self, other: &&str) -> bool {
        self.as_str() == *other
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TerminalRenderCell {
    pub text: TerminalRenderText,
    pub foreground: TerminalRenderColor,
    pub background: TerminalRenderColor,
    pub underline_color: Option<TerminalRenderColor>,
    pub bold: bool,
    pub dim: bool,
    pub italic: bool,
    pub underline_style: TerminalUnderlineStyle,
    pub inverse: bool,
    pub hidden: bool,
    pub strikethrough: bool,
    pub hyperlink: bool,
    pub wide_character_spacer: bool,
    pub leading_wide_character_spacer: bool,
    pub line_wrapped: bool,
}

impl Default for TerminalRenderCell {
    fn default() -> Self {
        Self {
            text: TerminalRenderText::default(),
            foreground: TerminalRenderColor::DefaultForeground,
            background: TerminalRenderColor::DefaultBackground,
            underline_color: None,
            bold: false,
            dim: false,
            italic: false,
            underline_style: TerminalUnderlineStyle::None,
            inverse: false,
            hidden: false,
            strikethrough: false,
            hyperlink: false,
            wide_character_spacer: false,
            leading_wide_character_spacer: false,
            line_wrapped: false,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TerminalViewportScrollDirection {
    Up,
    Down,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TerminalViewportScroll {
    pub top: usize,
    pub bottom: usize,
    pub count: usize,
    pub direction: TerminalViewportScrollDirection,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TerminalRenderDamageSnapshot {
    pub damage: TerminalDamageSnapshot,
    pub scrolls: Vec<TerminalViewportScroll>,
    pub generation: u64,
    pub palette_revision: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TerminalViewportMetadata {
    pub cols: u16,
    pub rows: u16,
    pub cursor: Option<TerminalCursorState>,
    pub display_offset: usize,
    pub history_size: usize,
    pub palette_revision: u64,
    pub generation: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TerminalPalette {
    pub indexed: [Option<TerminalColor>; 256],
    pub foreground: Option<TerminalColor>,
    pub background: Option<TerminalColor>,
    pub cursor: Option<TerminalColor>,
    pub revision: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TerminalRenderRead {
    pub metadata: TerminalViewportMetadata,
    pub palette: TerminalPalette,
    pub cells: Vec<TerminalRenderCell>,
    pub update: TerminalRenderDamageSnapshot,
}

/// One viewport cell. Carries no position: full frames are row-major
/// (`index = row * cols + col`) and partial updates list cells in dirty-span
/// order, so position is derived from context on the consuming side.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TermyCell {
    pub char: char,
    pub fg: TermyColor,
    pub bg: TermyColor,
    pub uses_terminal_default_bg: bool,
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
    pub strikethrough: bool,
    pub render_text: bool,
    pub wide_character_spacer: bool,
    pub line_wrapped: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TermyFrame {
    pub cols: u16,
    pub rows: u16,
    pub cells: Vec<TermyCell>,
    pub cursor: Option<TerminalCursorState>,
    pub display_offset: usize,
    pub history_size: usize,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TermyFrameUpdate {
    pub cols: u16,
    pub rows: u16,
    pub cells: Vec<TermyCell>,
    pub cursor: Option<TerminalCursorState>,
    pub display_offset: usize,
    pub history_size: usize,
    pub damage: TerminalDamageSnapshot,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_render_cell_uses_role_correct_terminal_colors() {
        let cell = TerminalRenderCell::default();
        assert_eq!(cell.foreground, TerminalRenderColor::DefaultForeground);
        assert_eq!(cell.background, TerminalRenderColor::DefaultBackground);
    }

    #[test]
    fn common_render_text_stays_inline() {
        let text = TerminalRenderText::from_cell_suffix('x', Some("y"));
        assert_eq!(text.as_str(), "xy");
        assert!(!text.is_heap_allocated());
    }
}
