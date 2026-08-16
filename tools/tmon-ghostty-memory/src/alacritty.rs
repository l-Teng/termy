use alacritty_terminal::{
    event::VoidListener,
    grid::Dimensions,
    index::Column,
    term::{
        Config as AlacrittyConfig, Term,
        cell::{Cell as AlacrittyCell, Flags},
    },
    vte::ansi::{self, Color as AlacrittyColor, NamedColor},
};

use super::*;

#[derive(Clone, Copy)]
struct BenchmarkSize;

impl Dimensions for BenchmarkSize {
    fn total_lines(&self) -> usize {
        usize::from(ROWS)
    }

    fn screen_lines(&self) -> usize {
        usize::from(ROWS)
    }

    fn columns(&self) -> usize {
        usize::from(COLS)
    }
}

pub(super) struct DirectAlacritty {
    terminal: Term<VoidListener>,
    parser: ansi::Processor,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct AlacrittyState {
    pub(super) history: usize,
    pub(super) cursor_row: usize,
    pub(super) cursor_col: usize,
}

impl DirectAlacritty {
    pub(super) fn new() -> Self {
        let terminal = Term::new(
            AlacrittyConfig {
                scrolling_history: SCROLLBACK_LIMIT,
                ..AlacrittyConfig::default()
            },
            &BenchmarkSize,
            VoidListener,
        );
        Self {
            terminal,
            parser: ansi::Processor::new(),
        }
    }

    pub(super) fn feed(&mut self, bytes: &[u8]) {
        self.parser.advance(&mut self.terminal, bytes);
    }

    pub(super) fn state(&self) -> Result<AlacrittyState, String> {
        let point = self.terminal.grid().cursor.point;
        Ok(AlacrittyState {
            history: self.terminal.history_size(),
            cursor_row: usize::try_from(point.line.0)
                .map_err(|_| format!("alacritty cursor row is negative: {}", point.line.0))?,
            cursor_col: point.column.0,
        })
    }

    pub(super) fn semantic_digest(&self) -> Result<u64, String> {
        let total_rows = self.terminal.total_lines();
        let cols = self.terminal.columns();
        if cols != usize::from(COLS) {
            return Err(format!(
                "alacritty digest expected {} columns, got {cols}",
                COLS
            ));
        }
        let first_line = self.terminal.topmost_line();
        let mut hash = Fnv1a64::new();
        hash.write_u64(total_rows as u64);
        hash.write_u16(COLS);
        hash.write_u64(total_rows as u64);

        for row_offset in full_row_offsets(total_rows) {
            hash.write_u64(row_offset as u64);
            let line = first_line
                + i32::try_from(row_offset)
                    .map_err(|_| "alacritty row offset overflowed i32".to_string())?;
            let row = &self.terminal.grid()[line];
            if row.len() != cols {
                return Err(format!(
                    "alacritty digest expected {cols} cells on line {}, saw {}",
                    line.0,
                    row.len()
                ));
            }
            for col in 0..cols {
                hash_alacritty_cell(&mut hash, &row[Column(col)]);
            }
        }

        Ok(hash.finish())
    }
}

fn hash_alacritty_cell(hash: &mut Fnv1a64, cell: &AlacrittyCell) {
    let role = if cell.flags.contains(Flags::LEADING_WIDE_CHAR_SPACER) {
        2
    } else if cell.flags.contains(Flags::WIDE_CHAR_SPACER) {
        1
    } else {
        0
    };
    hash.write_u8(role);

    hash.write_u64(1 + cell.zerowidth().map_or(0, <[char]>::len) as u64);
    hash.write_u32(cell.c as u32);
    if let Some(zerowidth) = cell.zerowidth() {
        for character in zerowidth {
            hash.write_u32(*character as u32);
        }
    }

    hash_alacritty_color(hash, Some(cell.fg));
    hash_alacritty_color(hash, Some(cell.bg));
    hash_alacritty_color(hash, cell.underline_color());
    hash.write_u8(u8::from(cell.flags.contains(Flags::BOLD)));
    hash.write_u8(u8::from(cell.flags.contains(Flags::DIM)));
    hash.write_u8(u8::from(cell.flags.contains(Flags::ITALIC)));
    hash.write_u8(u8::from(cell.flags.contains(Flags::INVERSE)));
    hash.write_u8(u8::from(cell.flags.contains(Flags::HIDDEN)));
    hash.write_u8(u8::from(cell.flags.contains(Flags::STRIKEOUT)));
    hash.write_u32(alacritty_underline_style_code(cell.flags));
}

fn hash_alacritty_color(hash: &mut Fnv1a64, color: Option<AlacrittyColor>) {
    match color {
        None
        | Some(AlacrittyColor::Named(
            NamedColor::Foreground
            | NamedColor::Background
            | NamedColor::Cursor
            | NamedColor::BrightForeground
            | NamedColor::DimForeground,
        )) => hash.write_u8(0),
        Some(AlacrittyColor::Named(named)) => {
            hash.write_u8(1);
            hash.write_u8(named_color_index(named));
        }
        Some(AlacrittyColor::Indexed(index)) => {
            hash.write_u8(1);
            hash.write_u8(index);
        }
        Some(AlacrittyColor::Spec(color)) => {
            hash.write_u8(2);
            hash.write_u8(color.r);
            hash.write_u8(color.g);
            hash.write_u8(color.b);
        }
    }
}

const fn named_color_index(color: NamedColor) -> u8 {
    match color {
        NamedColor::Black => 0,
        NamedColor::Red => 1,
        NamedColor::Green => 2,
        NamedColor::Yellow => 3,
        NamedColor::Blue => 4,
        NamedColor::Magenta => 5,
        NamedColor::Cyan => 6,
        NamedColor::White => 7,
        NamedColor::BrightBlack => 8,
        NamedColor::BrightRed => 9,
        NamedColor::BrightGreen => 10,
        NamedColor::BrightYellow => 11,
        NamedColor::BrightBlue => 12,
        NamedColor::BrightMagenta => 13,
        NamedColor::BrightCyan => 14,
        NamedColor::BrightWhite => 15,
        NamedColor::DimBlack => 0,
        NamedColor::DimRed => 1,
        NamedColor::DimGreen => 2,
        NamedColor::DimYellow => 3,
        NamedColor::DimBlue => 4,
        NamedColor::DimMagenta => 5,
        NamedColor::DimCyan => 6,
        NamedColor::DimWhite => 7,
        NamedColor::Foreground
        | NamedColor::Background
        | NamedColor::Cursor
        | NamedColor::BrightForeground
        | NamedColor::DimForeground => 0,
    }
}

const fn alacritty_underline_style_code(flags: Flags) -> u32 {
    if flags.contains(Flags::DOUBLE_UNDERLINE) {
        2
    } else if flags.contains(Flags::UNDERCURL) {
        3
    } else if flags.contains(Flags::DOTTED_UNDERLINE) {
        4
    } else if flags.contains(Flags::DASHED_UNDERLINE) {
        5
    } else if flags.contains(Flags::UNDERLINE) {
        1
    } else {
        0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn direct_adapter_uses_requested_geometry_and_history_limit() {
        let mut terminal = DirectAlacritty::new();
        terminal.feed(b"hello");

        assert_eq!(terminal.terminal.columns(), usize::from(COLS));
        assert_eq!(terminal.terminal.screen_lines(), usize::from(ROWS));
        assert_eq!(terminal.terminal.history_size(), 0);
        assert_eq!(terminal.state().unwrap().cursor_col, 5);
    }
}
