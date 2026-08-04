use crate::grid::{Charset, Grid, Rgb};
use crate::{
    ClipboardRequest, ClipboardTarget, Osc52, Size, decode_base64,
    graphics::{GraphicsCommand, GraphicsRenderPlacement, GraphicsState, MAX_COMMAND_BYTES},
};
use std::time::{Duration, Instant};

const MAX_CSI_PARAMS: usize = 32;
const MAX_OSC_BYTES: usize = 64 * 1024;
const MAX_DCS_BYTES: usize = 64 * 1024;
const MAX_SYNC_BYTES: usize = 2 * 1024 * 1024;
const SYNC_TIMEOUT: Duration = Duration::from_millis(150);
const SYNC_ESCAPE_LEN: usize = 8;
const BSU_CSI: &[u8; SYNC_ESCAPE_LEN] = b"\x1b[?2026h";
const ESU_CSI: &[u8; SYNC_ESCAPE_LEN] = b"\x1b[?2026l";
const TITLE_STACK_MAX_DEPTH: usize = 4096;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct QueryColors {
    pub ansi: [Rgb; 16],
    pub foreground: Rgb,
    pub background: Rgb,
    pub cursor: Option<Rgb>,
}

impl Default for QueryColors {
    fn default() -> Self {
        Self {
            ansi: [
                Rgb {
                    r: 0x00,
                    g: 0x00,
                    b: 0x00,
                },
                Rgb {
                    r: 0xcd,
                    g: 0x00,
                    b: 0x00,
                },
                Rgb {
                    r: 0x00,
                    g: 0xcd,
                    b: 0x00,
                },
                Rgb {
                    r: 0xcd,
                    g: 0xcd,
                    b: 0x00,
                },
                Rgb {
                    r: 0x00,
                    g: 0x00,
                    b: 0xee,
                },
                Rgb {
                    r: 0xcd,
                    g: 0x00,
                    b: 0xcd,
                },
                Rgb {
                    r: 0x00,
                    g: 0xcd,
                    b: 0xcd,
                },
                Rgb {
                    r: 0xe5,
                    g: 0xe5,
                    b: 0xe5,
                },
                Rgb {
                    r: 0x7f,
                    g: 0x7f,
                    b: 0x7f,
                },
                Rgb {
                    r: 0xff,
                    g: 0x00,
                    b: 0x00,
                },
                Rgb {
                    r: 0x00,
                    g: 0xff,
                    b: 0x00,
                },
                Rgb {
                    r: 0xff,
                    g: 0xff,
                    b: 0x00,
                },
                Rgb {
                    r: 0x5c,
                    g: 0x5c,
                    b: 0xff,
                },
                Rgb {
                    r: 0xff,
                    g: 0x00,
                    b: 0xff,
                },
                Rgb {
                    r: 0x00,
                    g: 0xff,
                    b: 0xff,
                },
                Rgb {
                    r: 0xff,
                    g: 0xff,
                    b: 0xff,
                },
            ],
            foreground: Rgb {
                r: 0xe5,
                g: 0xe5,
                b: 0xe5,
            },
            background: Rgb {
                r: 0x1e,
                g: 0x1e,
                b: 0x1e,
            },
            cursor: None,
        }
    }
}

impl QueryColors {
    fn indexed(self, index: u8) -> Rgb {
        match index {
            0..=15 => self.ansi[usize::from(index)],
            16..=231 => {
                let index = index - 16;
                let component = |value: u8| if value == 0 { 0 } else { 55 + value * 40 };
                Rgb {
                    r: component((index / 36) % 6),
                    g: component((index / 6) % 6),
                    b: component(index % 6),
                }
            }
            232..=255 => {
                let gray = 8 + (index - 232) * 10;
                Rgb {
                    r: gray,
                    g: gray,
                    b: gray,
                }
            }
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Progress {
    #[default]
    Clear,
    InProgress(u8),
    Error(u8),
    Indeterminate,
    Warning(u8),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum ParsedEvent {
    Title(String),
    ResetTitle,
    Bell,
    ClipboardStore(String),
    ClipboardLoad(ClipboardRequest),
    ShellPromptStart,
    ShellCommandStart,
    ShellCommandExecuting,
    ShellCommandFinished(Option<i32>),
    Progress(Progress),
    WorkingDirectory(String),
}

#[derive(Debug, Default)]
pub(crate) struct ParseOutput {
    pub(crate) events: Vec<ParsedEvent>,
    pub(crate) replies: Vec<u8>,
    pub(crate) synchronized_update_active: bool,
    pub(crate) synchronized_update_deadline: Option<Instant>,
    pub(crate) synchronized_update_refreshed: bool,
    pub(crate) synchronized_update_committed: bool,
    pub(crate) unsynchronized_activity: bool,
}

impl ParseOutput {
    fn append(&mut self, mut other: Self) {
        self.events.append(&mut other.events);
        self.replies.append(&mut other.replies);
        self.synchronized_update_active = other.synchronized_update_active;
        self.synchronized_update_deadline = other.synchronized_update_deadline;
        self.synchronized_update_refreshed |= other.synchronized_update_refreshed;
        self.synchronized_update_committed |= other.synchronized_update_committed;
        self.unsynchronized_activity |= other.unsynchronized_activity;
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum State {
    #[default]
    Ground,
    Escape,
    EscapeIntermediate,
    Csi,
    Osc,
    Dcs,
    ApcStart,
    Kitty,
    KittyEscape,
    IgnoreString,
}

#[derive(Debug, Default)]
struct Utf8Decoder {
    bytes: [u8; 4],
    len: usize,
    expected: usize,
}

impl Utf8Decoder {
    fn reset(&mut self) {
        self.len = 0;
        self.expected = 0;
    }

    fn is_pending(&self) -> bool {
        self.expected != 0
    }

    fn start(&mut self, byte: u8) -> bool {
        self.reset();
        self.expected = match byte {
            0xc2..=0xdf => 2,
            0xe0..=0xef => 3,
            0xf0..=0xf4 => 4,
            _ => return false,
        };
        self.bytes[0] = byte;
        self.len = 1;
        true
    }

    fn push_continuation(&mut self, byte: u8) -> Option<char> {
        if !byte.is_ascii() && (byte & 0xc0) == 0x80 && self.len < self.expected {
            self.bytes[self.len] = byte;
            self.len += 1;
            if self.len == self.expected {
                let value = std::str::from_utf8(&self.bytes[..self.len])
                    .ok()
                    .and_then(|text| text.chars().next())
                    .unwrap_or('\u{fffd}');
                self.reset();
                return Some(value);
            }
        }
        None
    }
}

#[derive(Debug)]
pub(crate) struct Parser {
    state: State,
    params: [u16; MAX_CSI_PARAMS],
    separators: [u8; MAX_CSI_PARAMS],
    param_count: usize,
    private_marker: u8,
    intermediate: u8,
    csi_has_params: bool,
    csi_intermediate_count: u8,
    csi_ignored: bool,
    osc: Vec<u8>,
    osc_oversized: bool,
    dcs: Vec<u8>,
    dcs_oversized: bool,
    kitty: Vec<u8>,
    kitty_oversized: bool,
    utf8: Utf8Decoder,
    last_printed: Option<char>,
    active_charset: usize,
    single_shift: Option<usize>,
    designating_charset: Option<u8>,
    escape_intermediate: u8,
    escape_intermediate_count: u8,
    escape_ignored: bool,
    query_colors: QueryColors,
    osc52: Osc52,
    synchronized_update_started: Option<Instant>,
    synchronized_update_buffer: Vec<u8>,
    title: Option<String>,
    title_stack: Vec<Option<String>>,
    graphics: GraphicsState,
    size: Size,
}

impl Default for Parser {
    fn default() -> Self {
        Self {
            state: State::Ground,
            params: [0; MAX_CSI_PARAMS],
            separators: [0; MAX_CSI_PARAMS],
            param_count: 0,
            private_marker: 0,
            intermediate: 0,
            csi_has_params: false,
            csi_intermediate_count: 0,
            csi_ignored: false,
            osc: Vec::with_capacity(256),
            osc_oversized: false,
            dcs: Vec::with_capacity(128),
            dcs_oversized: false,
            kitty: Vec::with_capacity(4096),
            kitty_oversized: false,
            utf8: Utf8Decoder::default(),
            last_printed: None,
            active_charset: 0,
            single_shift: None,
            designating_charset: None,
            escape_intermediate: 0,
            escape_intermediate_count: 0,
            escape_ignored: false,
            query_colors: QueryColors::default(),
            osc52: Osc52::default(),
            synchronized_update_started: None,
            synchronized_update_buffer: Vec::new(),
            title: None,
            title_stack: Vec::with_capacity(8),
            graphics: GraphicsState::default(),
            size: Size::default(),
        }
    }
}

impl Parser {
    pub(crate) fn set_query_colors(&mut self, colors: QueryColors) {
        self.query_colors = colors;
    }

    pub(crate) fn set_size(&mut self, size: Size) {
        self.size = size;
    }

    pub(crate) fn set_osc52(&mut self, osc52: Osc52) {
        self.osc52 = osc52;
    }

    pub(crate) fn graphics_placements(&self, grid: &Grid) -> Vec<GraphicsRenderPlacement> {
        self.graphics.render_placements(grid)
    }

    pub(crate) fn sync_grid_effects(&mut self, grid: &mut Grid) -> bool {
        let mut changed = false;
        while let Some(effect) = grid.pop_effect() {
            changed |= self.graphics.apply_grid_effect(effect);
        }
        if changed {
            grid.mark_full_damage();
        }
        changed
    }

    pub(crate) fn advance(&mut self, grid: &mut Grid, bytes: &[u8]) -> ParseOutput {
        let mut output = ParseOutput::default();
        if self
            .synchronized_update_started
            .is_some_and(|started| started.elapsed() >= SYNC_TIMEOUT)
        {
            output.append(self.stop_synchronized_update(grid));
        }

        let mut offset = 0;
        let mut candidate_sequence_start = None;
        while offset < bytes.len() {
            if self.synchronized_update_started.is_some() {
                if self
                    .synchronized_update_buffer
                    .len()
                    .saturating_add(bytes.len() - offset)
                    >= MAX_SYNC_BYTES - 1
                {
                    output.append(self.stop_synchronized_update(grid));
                    continue;
                }

                let new_bytes = bytes.len() - offset;
                self.synchronized_update_buffer
                    .extend_from_slice(&bytes[offset..]);
                offset = bytes.len();
                if let Some(bsu_offset) = self.scan_synchronized_update(new_bytes, &mut output) {
                    output.append(self.commit_synchronized_update(grid, bsu_offset));
                }
                continue;
            }

            if self.state == State::Ground && !self.utf8.is_pending() {
                let consumed = self.advance_ground_text(grid, &bytes[offset..], &mut output);
                if consumed > 0 {
                    offset += consumed;
                    continue;
                }
            }
            if self.state == State::Ground && bytes[offset] == 0x1b {
                candidate_sequence_start = Some(offset);
            }
            let was_synchronized = self.synchronized_update_started.is_some();
            self.advance_byte(grid, bytes[offset], &mut output);
            offset += 1;
            if !was_synchronized && self.synchronized_update_started.is_some() {
                output.unsynchronized_activity |= candidate_sequence_start
                    .is_some_and(|start| start > 0)
                    || !output.events.is_empty()
                    || !output.replies.is_empty();
            }
        }
        self.sync_grid_effects(grid);
        output.synchronized_update_active = self.synchronized_update_started.is_some();
        output.synchronized_update_deadline = self
            .synchronized_update_started
            .map(|started| started + SYNC_TIMEOUT);
        output
    }

    /// Commit a pending synchronized update immediately.
    ///
    /// The timeout and capacity paths deliberately use the same replay path as
    /// an exact ESU. This keeps grid changes, events, and protocol replies on a
    /// single commit boundary.
    pub(crate) fn stop_synchronized_update(&mut self, grid: &mut Grid) -> ParseOutput {
        if self.synchronized_update_started.is_none() {
            return ParseOutput::default();
        }
        self.commit_synchronized_update(grid, None)
    }

    /// Clear read-boundary state after persistence hydration.
    ///
    /// Completed semantic state lives in the grid, graphics store, title stack,
    /// and charset selection and is intentionally retained. Incomplete control
    /// strings, UTF-8, single-shift, and synchronized bytes belong to the replay
    /// parser and must not leak into the subsequent live PTY stream.
    pub(crate) fn reset_transient_state_after_hydration(&mut self) {
        self.state = State::Ground;
        self.params.fill(0);
        self.separators.fill(0);
        self.param_count = 0;
        self.private_marker = 0;
        self.intermediate = 0;
        self.csi_has_params = false;
        self.csi_intermediate_count = 0;
        self.csi_ignored = false;
        self.osc.clear();
        self.osc_oversized = false;
        self.dcs.clear();
        self.dcs_oversized = false;
        self.kitty.clear();
        self.kitty_oversized = false;
        self.utf8.reset();
        self.last_printed = None;
        self.single_shift = None;
        self.designating_charset = None;
        self.reset_escape_intermediates();
        self.synchronized_update_started = None;
        self.synchronized_update_buffer.clear();
    }

    /// Scan only newly appended synchronized bytes for exact raw BSU/ESU.
    /// The overlap matches vte's local scan, so an escape split at any byte
    /// boundary is still recognized without rescanning the full 2 MiB buffer.
    fn scan_synchronized_update(
        &mut self,
        new_bytes: usize,
        output: &mut ParseOutput,
    ) -> Option<Option<usize>> {
        let buffer_len = self.synchronized_update_buffer.len();
        let start_offset = (buffer_len - new_bytes).saturating_sub(SYNC_ESCAPE_LEN - 1);
        let end_offset = buffer_len.saturating_sub(SYNC_ESCAPE_LEN - 1);
        let mut bsu_offset = None;

        for offset in (start_offset..end_offset)
            .rev()
            .filter(|&offset| self.synchronized_update_buffer[offset] == 0x1b)
        {
            let escape = &self.synchronized_update_buffer[offset..offset + SYNC_ESCAPE_LEN];
            if escape == BSU_CSI {
                self.synchronized_update_started = Some(Instant::now());
                output.synchronized_update_refreshed = true;
                bsu_offset = Some(offset);
            } else if escape == ESU_CSI {
                return Some(bsu_offset);
            }
        }
        None
    }

    fn commit_synchronized_update(
        &mut self,
        grid: &mut Grid,
        bsu_offset: Option<usize>,
    ) -> ParseOutput {
        let mut buffer = std::mem::take(&mut self.synchronized_update_buffer);
        let replay_end = bsu_offset.unwrap_or(buffer.len());
        let mut output = ParseOutput {
            synchronized_update_committed: true,
            ..ParseOutput::default()
        };
        self.advance_unbuffered(grid, &buffer[..replay_end], &mut output);

        if let Some(offset) = bsu_offset {
            let retained_len = buffer.len() - offset;
            buffer.copy_within(offset.., 0);
            buffer.truncate(retained_len);
            self.synchronized_update_buffer = buffer;
            self.synchronized_update_started = Some(Instant::now());
            output.synchronized_update_refreshed = true;
        } else {
            buffer.clear();
            self.synchronized_update_buffer = buffer;
            self.synchronized_update_started = None;
        }
        self.sync_grid_effects(grid);
        output.synchronized_update_active = self.synchronized_update_started.is_some();
        output.synchronized_update_deadline = self
            .synchronized_update_started
            .map(|started| started + SYNC_TIMEOUT);
        output
    }

    /// Parse bytes without entering the outer synchronized staging loop. This
    /// is used exclusively to replay a committed batch.
    fn advance_unbuffered(&mut self, grid: &mut Grid, bytes: &[u8], output: &mut ParseOutput) {
        let mut offset = 0;
        while offset < bytes.len() {
            if self.state == State::Ground && !self.utf8.is_pending() {
                let consumed = self.advance_ground_text(grid, &bytes[offset..], output);
                if consumed > 0 {
                    offset += consumed;
                    continue;
                }
            }
            self.advance_byte(grid, bytes[offset], output);
            offset += 1;
        }
    }

    fn advance_ground_text(
        &mut self,
        grid: &mut Grid,
        bytes: &[u8],
        output: &mut ParseOutput,
    ) -> usize {
        if self.single_shift.is_none() && grid.charset(self.active_charset) == Charset::Ascii {
            let ascii_len = bytes
                .iter()
                .take_while(|&&byte| (0x20..=0x7e).contains(&byte))
                .count();
            if ascii_len > 0 {
                self.print_ascii(grid, &bytes[..ascii_len]);
                return ascii_len;
            }
        }

        let span_len = bytes
            .iter()
            .position(|byte| *byte < 0x20 || *byte == 0x7f)
            .unwrap_or(bytes.len());
        if span_len == 0 {
            return 0;
        }

        let candidate = &bytes[..span_len];
        let mut consumed = 0;
        while consumed < candidate.len() {
            match std::str::from_utf8(&candidate[consumed..]) {
                Ok(text) => {
                    self.print_text(grid, text);
                    return span_len;
                }
                Err(error) => {
                    let valid_len = error.valid_up_to();
                    if valid_len > 0 {
                        let text = std::str::from_utf8(&candidate[consumed..consumed + valid_len])
                            .expect("the UTF-8 error prefix is always valid");
                        self.print_text(grid, text);
                        consumed += valid_len;
                    }

                    self.advance_byte(grid, candidate[consumed], output);
                    consumed += 1;
                    if self.state != State::Ground {
                        return consumed;
                    }
                    while consumed < candidate.len() && self.utf8.is_pending() {
                        self.advance_byte(grid, candidate[consumed], output);
                        consumed += 1;
                    }
                }
            }
        }
        span_len
    }

    fn print_text(&mut self, grid: &mut Grid, mut text: &str) {
        let ascii_fast_path =
            self.single_shift.is_none() && grid.charset(self.active_charset) == Charset::Ascii;
        while !text.is_empty() {
            if ascii_fast_path {
                let ascii_len = text
                    .as_bytes()
                    .iter()
                    .take_while(|&&byte| (0x20..=0x7e).contains(&byte))
                    .count();
                if ascii_len > 0 {
                    let ascii = &text.as_bytes()[..ascii_len];
                    self.print_ascii(grid, ascii);
                    text = &text[ascii_len..];
                    continue;
                }
            }

            let character = text
                .chars()
                .next()
                .expect("non-empty text always has a first character");
            self.print(grid, character);
            text = &text[character.len_utf8()..];
        }
    }

    fn print_ascii(&mut self, grid: &mut Grid, ascii: &[u8]) {
        let mut consumed_ascii = 0;
        while consumed_ascii < ascii.len() {
            let consumed = grid.put_ascii_run(&ascii[consumed_ascii..]);
            if consumed == 0 {
                self.print(grid, char::from(ascii[consumed_ascii]));
                consumed_ascii += 1;
            } else {
                consumed_ascii += consumed;
                self.last_printed = Some(char::from(ascii[consumed_ascii - 1]));
            }
        }
    }

    fn advance_byte(&mut self, grid: &mut Grid, byte: u8, output: &mut ParseOutput) {
        if matches!(byte, 0x18 | 0x1a) && self.state != State::Ground {
            if self.state == State::Osc {
                self.finish_osc(grid, output, b"\x1b\\");
            }
            self.state = State::Ground;
            self.utf8.reset();
            self.osc.clear();
            self.dcs.clear();
            self.kitty.clear();
            self.osc_oversized = false;
            self.dcs_oversized = false;
            self.kitty_oversized = false;
            return;
        }
        match self.state {
            State::Ground => self.advance_ground(grid, byte, output),
            State::Escape => self.advance_escape(grid, byte, output),
            State::EscapeIntermediate => self.advance_escape_intermediate(grid, byte, output),
            State::Csi => self.advance_csi(grid, byte, output),
            State::Osc => self.advance_osc(grid, byte, output),
            State::Dcs => self.advance_dcs(grid, byte, output),
            State::ApcStart => {
                if byte == b'G' {
                    self.kitty.clear();
                    self.kitty_oversized = false;
                    self.state = State::Kitty;
                } else if byte == 0x1b {
                    self.state = State::Escape;
                } else if byte == 0x9c {
                    self.state = State::Ground;
                } else {
                    self.state = State::IgnoreString;
                }
            }
            State::Kitty => match byte {
                0x1b => {
                    self.state = State::KittyEscape;
                }
                0x9c => {
                    self.finish_kitty(grid, output);
                    self.state = State::Ground;
                }
                _ => self.push_kitty(byte),
            },
            State::KittyEscape => {
                if byte == b'\\' {
                    self.finish_kitty(grid, output);
                    self.state = State::Ground;
                } else {
                    self.push_kitty(0x1b);
                    self.push_kitty(byte);
                    self.state = State::Kitty;
                }
            }
            State::IgnoreString => {
                if byte == 0x1b {
                    self.state = State::Escape;
                } else if byte == 0x9c {
                    self.state = State::Ground;
                }
            }
        }
    }

    fn advance_ground(&mut self, grid: &mut Grid, byte: u8, output: &mut ParseOutput) {
        if self.utf8.is_pending() {
            if (byte & 0xc0) == 0x80 {
                if let Some(character) = self.utf8.push_continuation(byte) {
                    self.print(grid, character);
                }
                return;
            }
            self.utf8.reset();
            self.print(grid, '\u{fffd}');
            self.advance_ground(grid, byte, output);
            return;
        }

        match byte {
            0x00 | 0x18 | 0x7f => {}
            0x07 => output.events.push(ParsedEvent::Bell),
            0x08 => grid.backspace(),
            0x09 => grid.tab(),
            0x0a..=0x0c => grid.line_feed(),
            0x0d => grid.carriage_return(),
            0x0e => self.active_charset = 1,
            0x0f => self.active_charset = 0,
            0x1a => {}
            0x1b => self.state = State::Escape,
            0x20..=0x7e => self.print(grid, char::from(byte)),
            // Termy's native Kitty interceptor accepts the 8-bit APC introducer
            // even though other standalone C1 bytes are ignored in UTF-8 mode.
            0x9f => self.state = State::ApcStart,
            // The PTY stream is UTF-8. Match Alacritty by ignoring standalone
            // C1 bytes instead of interpreting their legacy 8-bit forms.
            0x80..=0x9f => {}
            _ => {
                if !self.utf8.start(byte) {
                    self.print(grid, '\u{fffd}');
                }
            }
        }
    }

    fn print(&mut self, grid: &mut Grid, character: char) {
        // VTE dispatches valid UTF-8 encodings of C1 code points as controls.
        // Alacritty ignores those controls in UTF-8 mode, so they must not be
        // retained as zero-width combining text or become REP's last glyph.
        if ('\u{80}'..='\u{9f}').contains(&character) {
            return;
        }
        self.last_printed = Some(character);
        let charset_index = self.single_shift.take().unwrap_or(self.active_charset);
        let charset = grid.charset(charset_index);
        let character = match charset {
            Charset::DecSpecial => dec_special_character(character),
            Charset::Uk if character == '#' => '£',
            Charset::Ascii | Charset::Uk => character,
        };
        grid.put_char(character);
    }

    fn advance_escape(&mut self, grid: &mut Grid, byte: u8, output: &mut ParseOutput) {
        self.utf8.reset();
        if self.execute_control(grid, byte, output) {
            return;
        }
        match byte {
            b'[' => self.start_csi(),
            b']' => self.start_osc(),
            b'P' => self.start_dcs(),
            b'_' => self.state = State::ApcStart,
            b'X' | b'^' => self.state = State::IgnoreString,
            b'7' => {
                grid.save_cursor();
                self.state = State::Ground;
            }
            b'8' => {
                grid.restore_cursor();
                self.single_shift = None;
                self.state = State::Ground;
            }
            b'D' => {
                grid.line_feed();
                self.state = State::Ground;
            }
            b'E' => {
                grid.next_line();
                self.state = State::Ground;
            }
            b'M' => {
                grid.reverse_index();
                self.state = State::Ground;
            }
            b'c' => {
                grid.reset();
                self.active_charset = 0;
                self.single_shift = None;
                self.title = None;
                self.title_stack.clear();
                output.events.push(ParsedEvent::ResetTitle);
                self.state = State::Ground;
            }
            b'H' => {
                grid.set_tab_stop();
                self.state = State::Ground;
            }
            b'Z' => {
                // DECID is the legacy primary-device-attributes request. It
                // does not inherit a private marker from an earlier CSI.
                output.replies.extend_from_slice(b"\x1b[?6c");
                self.state = State::Ground;
            }
            b'=' => {
                grid.set_keypad_application_mode(true);
                self.state = State::Ground;
            }
            b'>' => {
                grid.set_keypad_application_mode(false);
                self.state = State::Ground;
            }
            b'\\' => self.state = State::Ground,
            b'N' | b'O' => {
                self.single_shift = Some(if byte == b'N' { 2 } else { 3 });
                self.state = State::Ground;
            }
            b'n' | b'o' => {
                self.active_charset = if byte == b'n' { 2 } else { 3 };
                self.state = State::Ground;
            }
            0x20..=0x2f => {
                self.designating_charset = Some(match byte {
                    b'(' => 0,
                    b')' | b'-' => 1,
                    b'*' | b'.' => 2,
                    b'+' | b'/' => 3,
                    _ => {
                        self.escape_intermediate = byte;
                        self.escape_intermediate_count = 1;
                        self.escape_ignored = false;
                        self.state = State::EscapeIntermediate;
                        return;
                    }
                });
                self.escape_intermediate = byte;
                self.escape_intermediate_count = 1;
                self.escape_ignored = false;
                self.state = State::EscapeIntermediate;
            }
            0x1b => {
                self.reset_escape_intermediates();
                self.state = State::Escape;
            }
            0x30..=0x7e => self.state = State::Ground,
            _ => {}
        }
    }

    fn advance_escape_intermediate(&mut self, grid: &mut Grid, byte: u8, output: &mut ParseOutput) {
        if self.execute_control(grid, byte, output) {
            return;
        }
        match byte {
            0x20..=0x2f => {
                self.escape_intermediate_count = self.escape_intermediate_count.saturating_add(1);
                self.escape_ignored = true;
                self.designating_charset = None;
            }
            0x30..=0x7e => {
                if !self.escape_ignored {
                    if let Some(slot) = self.designating_charset.take() {
                        let charset = match byte {
                            b'0' => Some(Charset::DecSpecial),
                            b'A' => Some(Charset::Uk),
                            b'B' => Some(Charset::Ascii),
                            _ => None,
                        };
                        if let Some(charset) = charset {
                            grid.set_charset(usize::from(slot), charset);
                        }
                    } else if self.escape_intermediate == b'#' && byte == b'8' {
                        grid.alignment_test();
                    }
                }
                self.reset_escape_intermediates();
                self.state = State::Ground;
            }
            0x1b => {
                self.reset_escape_intermediates();
                self.state = State::Escape;
            }
            _ => {}
        }
    }

    fn reset_escape_intermediates(&mut self) {
        self.designating_charset = None;
        self.escape_intermediate = 0;
        self.escape_intermediate_count = 0;
        self.escape_ignored = false;
    }

    fn execute_control(&mut self, grid: &mut Grid, byte: u8, output: &mut ParseOutput) -> bool {
        match byte {
            0x00..=0x06 | 0x10..=0x17 | 0x19 | 0x1c..=0x1f | 0x7f => {}
            0x07 => output.events.push(ParsedEvent::Bell),
            0x08 => grid.backspace(),
            0x09 => grid.tab(),
            0x0a..=0x0c => grid.line_feed(),
            0x0d => grid.carriage_return(),
            0x0e => self.active_charset = 1,
            0x0f => self.active_charset = 0,
            _ => return false,
        }
        true
    }

    fn start_csi(&mut self) {
        self.state = State::Csi;
        self.params.fill(0);
        self.separators.fill(0);
        self.param_count = 1;
        self.private_marker = 0;
        self.intermediate = 0;
        self.csi_has_params = false;
        self.csi_intermediate_count = 0;
        self.csi_ignored = false;
    }

    fn advance_csi(&mut self, grid: &mut Grid, byte: u8, output: &mut ParseOutput) {
        if self.csi_ignored {
            match byte {
                0x07 => output.events.push(ParsedEvent::Bell),
                0x08 => grid.backspace(),
                0x09 => grid.tab(),
                0x0a..=0x0c => grid.line_feed(),
                0x0d => grid.carriage_return(),
                0x0e => self.active_charset = 1,
                0x0f => self.active_charset = 0,
                0x1b => self.state = State::Escape,
                0x40..=0x7e => self.state = State::Ground,
                _ => {}
            }
            return;
        }

        match byte {
            0x00 | 0x7f => {}
            0x07 => output.events.push(ParsedEvent::Bell),
            0x08 => grid.backspace(),
            0x09 => grid.tab(),
            0x0a..=0x0c => grid.line_feed(),
            0x0d => grid.carriage_return(),
            0x0e => self.active_charset = 1,
            0x0f => self.active_charset = 0,
            b'0'..=b'9' => {
                if self.csi_intermediate_count != 0 {
                    self.csi_ignored = true;
                    return;
                }
                self.csi_has_params = true;
                let index = self.param_count.saturating_sub(1);
                if index < MAX_CSI_PARAMS {
                    self.params[index] = self.params[index]
                        .saturating_mul(10)
                        .saturating_add(u16::from(byte - b'0'));
                }
            }
            b';' | b':' => {
                if self.csi_intermediate_count != 0 {
                    self.csi_ignored = true;
                    return;
                }
                self.csi_has_params = true;
                if self.param_count < MAX_CSI_PARAMS {
                    self.separators[self.param_count] = byte;
                    self.param_count += 1;
                } else {
                    self.csi_ignored = true;
                }
            }
            b'?' | b'>' | b'<' | b'='
                if !self.csi_has_params
                    && self.csi_intermediate_count == 0
                    && self.private_marker == 0 =>
            {
                self.private_marker = byte;
                self.csi_has_params = true;
            }
            b'?' | b'>' | b'<' | b'=' => {
                self.csi_ignored = true;
            }
            0x20..=0x2f => {
                self.csi_intermediate_count = self.csi_intermediate_count.saturating_add(1);
                if self.csi_intermediate_count == 1 {
                    self.intermediate = byte;
                } else {
                    self.csi_ignored = true;
                }
            }
            0x40..=0x7e => {
                self.dispatch_csi(grid, byte, output);
                self.state = State::Ground;
            }
            0x1b => self.state = State::Escape,
            _ => {}
        }
    }

    fn dispatch_csi(&mut self, grid: &mut Grid, final_byte: u8, output: &mut ParseOutput) {
        if self.intermediate != 0
            && !matches!(
                (final_byte, self.intermediate),
                (b'q', b' ' | b'"') | (b'p', b'$')
            )
        {
            return;
        }
        if self.private_marker != 0
            && !matches!(
                (final_byte, self.private_marker, self.intermediate),
                (b'h' | b'l' | b'J' | b'K' | b'W', b'?', _)
                    | (b'u', b'=' | b'>' | b'<' | b'?', _)
                    | (b'p', b'?', b'$')
                    | (b'c', b'>', _)
            )
        {
            return;
        }
        let first = self.param_or(0, 1) as usize;
        match final_byte {
            b'A' => grid.cursor_up(first),
            b'B' | b'e' => grid.cursor_down(first),
            b'C' | b'a' => grid.cursor_forward(first),
            b'D' => grid.cursor_backward(first),
            b'E' => grid.cursor_next_line(first),
            b'F' => grid.cursor_previous_line(first),
            b'G' | b'`' => grid.set_cursor_col(first.saturating_sub(1)),
            b'd' => grid.set_cursor_row(first.saturating_sub(1)),
            b'H' | b'f' => {
                let row = self.param_or(0, 1) as usize;
                let col = self.param_or(1, 1) as usize;
                grid.set_cursor_position(row.saturating_sub(1), col.saturating_sub(1));
            }
            b'J' => grid.erase_display(self.param(0), self.private_marker == b'?'),
            b'K' => grid.erase_line(self.param(0), self.private_marker == b'?'),
            b'I' => grid.move_forward_tabs(first),
            b'Z' => grid.move_backward_tabs(first),
            b'W' if self.private_marker == b'?' && self.param(0) == 5 => {
                grid.reset_tab_stops();
            }
            b'g' => grid.clear_tab_stop(self.param(0)),
            b'X' => grid.erase_chars(first),
            b'@' => grid.insert_blank_chars(first),
            b'P' => grid.delete_chars(first),
            b'L' => grid.insert_lines(first),
            b'M' => grid.delete_lines(first),
            b'S' => grid.scroll_up(first),
            b'T' => grid.scroll_down(first),
            b'b' => {
                if let Some(character) = self.last_printed {
                    for _ in 0..first.min(4096) {
                        self.print(grid, character);
                    }
                }
            }
            b'm' => self.dispatch_sgr(grid),
            b'h' | b'l' => {
                let enabled = final_byte == b'h';
                let private = self.private_marker == b'?';
                for index in 0..self.param_count {
                    let mode = self.params[index];
                    if private && mode == 2026 {
                        if enabled {
                            self.synchronized_update_started = Some(Instant::now());
                            self.synchronized_update_buffer.clear();
                            output.synchronized_update_refreshed = true;
                            output.unsynchronized_activity |= self.param_count > 1;
                        } else {
                            self.synchronized_update_started = None;
                            self.synchronized_update_buffer.clear();
                        }
                    } else {
                        grid.set_mode(private, mode, enabled);
                    }
                }
            }
            b'r' => {
                let rows = grid.rows();
                let top = self.param_or(0, 1) as usize;
                let bottom = self.param_or(1, rows as u16) as usize;
                if top == 1 && bottom == rows {
                    grid.reset_scroll_region();
                } else {
                    grid.set_scroll_region(top.saturating_sub(1), bottom.saturating_sub(1));
                }
            }
            b's' => {
                grid.save_cursor();
            }
            b'u' if self.private_marker == b'=' => {
                let flags = self.param(0);
                let mode = self.param_or(1, 1);
                grid.set_kitty_keyboard_flags(flags, mode);
            }
            b'u' if self.private_marker == b'>' => {
                grid.push_kitty_keyboard_flags(self.param(0));
            }
            b'u' if self.private_marker == b'<' => {
                grid.pop_kitty_keyboard_flags(self.param_or(0, 1) as usize);
            }
            b'u' if self.private_marker == b'?' => {
                output.replies.extend_from_slice(
                    format!("\x1b[?{}u", grid.kitty_keyboard_report_flags()).as_bytes(),
                );
            }
            b'u' => {
                grid.restore_cursor();
                self.single_shift = None;
            }
            b'q' if self.intermediate == b' ' => grid.set_cursor_style(self.param(0)),
            b'q' if self.intermediate == b'"' => {
                grid.set_character_protection(self.param(0));
            }
            b'p' if self.intermediate == b'$' => {
                let mode = self.param(0);
                let private = self.private_marker == b'?';
                let marker = if private { "?" } else { "" };
                let state = if private && mode == 2026 {
                    if self.synchronized_update_started.is_some() {
                        1
                    } else {
                        2
                    }
                } else {
                    grid.mode_state(private, mode)
                };
                output
                    .replies
                    .extend_from_slice(format!("\x1b[{marker}{mode};{state}$y").as_bytes());
            }
            b't' => match self.param_or(0, 1) {
                14 => {
                    let (height, width) = grid.text_area_size_pixels();
                    output
                        .replies
                        .extend_from_slice(format!("\x1b[4;{height};{width}t").as_bytes());
                }
                18 => output.replies.extend_from_slice(
                    format!("\x1b[8;{};{}t", grid.rows(), grid.cols()).as_bytes(),
                ),
                22 => {
                    if self.title_stack.len() >= TITLE_STACK_MAX_DEPTH {
                        self.title_stack.remove(0);
                    }
                    self.title_stack.push(self.title.clone());
                }
                23 => {
                    if let Some(title) = self.title_stack.pop() {
                        self.title = title.clone();
                        output.events.push(match title {
                            Some(title) => ParsedEvent::Title(title),
                            None => ParsedEvent::ResetTitle,
                        });
                    }
                }
                _ => {}
            },
            b'n' => self.device_status_report(grid, output),
            b'c' if self.param(0) == 0 => self.device_attributes(output),
            _ => {}
        }
    }

    fn device_status_report(&self, grid: &Grid, output: &mut ParseOutput) {
        if self.private_marker != 0 {
            return;
        }
        match self.param(0) {
            5 => output.replies.extend_from_slice(b"\x1b[0n"),
            6 => {
                let (col, row) = grid.cursor_position();
                output
                    .replies
                    .extend_from_slice(format!("\x1b[{};{}R", row + 1, col + 1).as_bytes());
            }
            _ => {}
        }
    }

    fn dispatch_sgr(&self, grid: &mut Grid) {
        let mut normalized = [0u16; MAX_CSI_PARAMS];
        let mut output_len = 0usize;
        let mut index = 0usize;
        while index < self.param_count && output_len < MAX_CSI_PARAMS {
            let code = self.params[index];
            let colon_group = index + 1 < self.param_count && self.separators[index + 1] == b':';
            if !colon_group {
                normalized[output_len] = code;
                output_len += 1;
                index += 1;
                continue;
            }

            let mut group_end = index + 1;
            while group_end < self.param_count && self.separators[group_end] == b':' {
                group_end += 1;
            }
            match code {
                4 => {
                    normalized[output_len] = if self.params.get(index + 1) == Some(&0) {
                        24
                    } else {
                        4
                    };
                    output_len += 1;
                }
                38 | 48 | 58 if index + 1 < group_end => {
                    let mode = self.params[index + 1];
                    match mode {
                        2 => {
                            let values = &self.params[index + 2..group_end];
                            let start = usize::from(values.len() >= 4);
                            if values.len().saturating_sub(start) >= 3
                                && output_len + 5 <= MAX_CSI_PARAMS
                            {
                                normalized[output_len..output_len + 5].copy_from_slice(&[
                                    code,
                                    2,
                                    values[start],
                                    values[start + 1],
                                    values[start + 2],
                                ]);
                                output_len += 5;
                            }
                        }
                        5 if index + 2 < group_end && output_len + 3 <= MAX_CSI_PARAMS => {
                            normalized[output_len..output_len + 3].copy_from_slice(&[
                                code,
                                5,
                                self.params[index + 2],
                            ]);
                            output_len += 3;
                        }
                        _ => {}
                    }
                }
                _ => {
                    normalized[output_len] = code;
                    output_len += 1;
                }
            }
            index = group_end;
        }
        grid.sgr(&normalized[..output_len]);
    }

    fn device_attributes(&self, output: &mut ParseOutput) {
        match self.private_marker {
            b'>' => output.replies.extend_from_slice(b"\x1b[>0;2600;1c"),
            _ => output.replies.extend_from_slice(b"\x1b[?6c"),
        }
    }

    fn param(&self, index: usize) -> u16 {
        self.params.get(index).copied().unwrap_or(0)
    }

    fn param_or(&self, index: usize, default: u16) -> u16 {
        match self.param(index) {
            0 => default,
            value => value,
        }
    }

    fn start_osc(&mut self) {
        self.state = State::Osc;
        self.osc.clear();
        self.osc_oversized = false;
    }

    fn start_dcs(&mut self) {
        self.state = State::Dcs;
        self.dcs.clear();
        self.dcs_oversized = false;
    }

    fn advance_dcs(&mut self, grid: &mut Grid, byte: u8, output: &mut ParseOutput) {
        match byte {
            0x9c => {
                self.finish_dcs(grid, output);
                self.state = State::Ground;
            }
            0x1b => {
                self.finish_dcs(grid, output);
                self.state = State::Escape;
            }
            _ => self.push_dcs(byte),
        }
    }

    fn push_dcs(&mut self, byte: u8) {
        if self.dcs.len() < MAX_DCS_BYTES {
            self.dcs.push(byte);
        } else {
            self.dcs_oversized = true;
        }
    }

    fn push_kitty(&mut self, byte: u8) {
        if self.kitty.len() < MAX_COMMAND_BYTES {
            self.kitty.push(byte);
        } else {
            self.kitty_oversized = true;
        }
    }

    fn finish_kitty(&mut self, grid: &mut Grid, output: &mut ParseOutput) {
        // Apply preceding text scroll/clear effects before anchoring this image. Remaining
        // effects are drained once at the end of the PTY batch instead of once per byte.
        self.sync_grid_effects(grid);
        let command = GraphicsCommand::parse(std::mem::take(&mut self.kitty), self.kitty_oversized);
        let result = self.graphics.apply(command, grid, self.size);
        output.replies.extend(result.replies);
        if result.changed {
            grid.mark_full_damage();
        }
        self.kitty_oversized = false;
    }

    fn finish_dcs(&mut self, grid: &Grid, output: &mut ParseOutput) {
        if std::mem::take(&mut self.dcs_oversized) {
            self.dcs.clear();
            return;
        }
        let payload = self.dcs.as_slice();
        if let Some(request) = payload.strip_prefix(b"$q") {
            let response = match request {
                b"m" => Some(grid.sgr_status()),
                b"r" => {
                    let (top, bottom) = grid.scroll_region_status();
                    Some(format!("{top};{bottom}r"))
                }
                b" q" => Some(format!("{} q", grid.cursor_style_status())),
                b"\"p" => Some("65;1\"p".to_string()),
                b"\"q" => Some(format!("{}\"q", grid.character_protection_status())),
                _ => None,
            };
            output.replies.extend_from_slice(b"\x1bP");
            match response {
                Some(response) => {
                    output.replies.extend_from_slice(b"1$r");
                    output.replies.extend_from_slice(response.as_bytes());
                }
                None => output.replies.extend_from_slice(b"0$r"),
            }
            output.replies.extend_from_slice(b"\x1b\\");
        } else if let Some(requests) = payload.strip_prefix(b"+q") {
            for encoded_name in requests.split(|byte| *byte == b';') {
                if encoded_name.is_empty() {
                    continue;
                }
                let Some(name) = decode_hex_ascii(encoded_name) else {
                    continue;
                };
                let capability = match name.as_str() {
                    "TN" => Some(Some("xterm-256color")),
                    "Co" | "colors" => Some(Some("256")),
                    "RGB" => Some(Some("8/8/8")),
                    "Tc" | "Su" => Some(None),
                    _ => None,
                };
                output.replies.extend_from_slice(b"\x1bP");
                if let Some(value) = capability {
                    output.replies.extend_from_slice(b"1+r");
                    output.replies.extend_from_slice(encoded_name);
                    if let Some(value) = value {
                        output.replies.push(b'=');
                        push_hex_ascii(&mut output.replies, value.as_bytes());
                    }
                } else {
                    output.replies.extend_from_slice(b"0+r");
                    output.replies.extend_from_slice(encoded_name);
                }
                output.replies.extend_from_slice(b"\x1b\\");
            }
        }
    }

    fn advance_osc(&mut self, grid: &mut Grid, byte: u8, output: &mut ParseOutput) {
        match byte {
            0x00..=0x06 | 0x08..=0x17 | 0x19 | 0x1c..=0x1f => {}
            0x07 => {
                self.finish_osc(grid, output, b"\x07");
                self.state = State::Ground;
            }
            0x1b => {
                self.finish_osc(grid, output, b"\x1b\\");
                self.state = State::Escape;
            }
            _ => self.push_osc(byte),
        }
    }

    fn push_osc(&mut self, byte: u8) {
        if self.osc.len() < MAX_OSC_BYTES {
            self.osc.push(byte);
        } else {
            self.osc_oversized = true;
        }
    }

    fn finish_osc(&mut self, grid: &mut Grid, output: &mut ParseOutput, terminator: &[u8]) {
        if std::mem::take(&mut self.osc_oversized) {
            self.osc.clear();
            return;
        }
        let payload = String::from_utf8_lossy(&self.osc);
        let mut fields = payload.split(';');
        let Some(command) = fields.next() else {
            return;
        };

        match command {
            "0" | "2" => {
                let values = fields.collect::<Vec<_>>();
                if values.is_empty() {
                    return;
                }
                let title = values.join(";").trim().to_owned();
                self.title = Some(title.clone());
                output.events.push(ParsedEvent::Title(title));
            }
            "7" => {
                let value = fields.collect::<Vec<_>>().join(";");
                output
                    .events
                    .push(ParsedEvent::WorkingDirectory(osc7_path(&value)));
            }
            "8" => {
                let parameters = fields.next().unwrap_or_default();
                let protocol_id = parameters
                    .split(':')
                    .find_map(|parameter| parameter.strip_prefix("id="));
                let target = fields.collect::<Vec<_>>().join(";");
                grid.set_hyperlink(protocol_id, (!target.is_empty()).then_some(target.as_str()));
            }
            "4" => {
                let values = fields.collect::<Vec<_>>();
                if values.len() % 2 == 0 {
                    for pair in values.chunks_exact(2) {
                        let Some(index) = pair[0].parse::<u8>().ok() else {
                            continue;
                        };
                        if pair[1] == "?" {
                            let color = grid
                                .palette()
                                .indexed(index)
                                .unwrap_or_else(|| self.query_colors.indexed(index));
                            push_color_reply(output, &format!("4;{index}"), color, terminator);
                        } else if let Some(color) = parse_x_color(pair[1]) {
                            grid.set_indexed_color(index, color);
                        }
                    }
                }
            }
            "10" | "11" | "12" => {
                let Some(base) = command.parse::<u8>().ok() else {
                    return;
                };
                for (offset, value) in fields.enumerate() {
                    let code = base.saturating_add(offset as u8);
                    if code > 12 {
                        break;
                    }
                    if value == "?" {
                        let palette = grid.palette();
                        let color = match code {
                            10 => {
                                Some(palette.foreground().unwrap_or(self.query_colors.foreground))
                            }
                            11 => {
                                Some(palette.background().unwrap_or(self.query_colors.background))
                            }
                            12 => palette.cursor(),
                            _ => None,
                        };
                        if let Some(color) = color {
                            push_color_reply(output, &code.to_string(), color, terminator);
                        }
                    } else if let Some(color) = parse_x_color(value) {
                        match code {
                            10 => grid.set_foreground_color(Some(color)),
                            11 => grid.set_background_color(Some(color)),
                            12 => grid.set_cursor_color(Some(color)),
                            _ => {}
                        }
                    }
                }
            }
            "9" => {
                let subtype = fields.next();
                match subtype {
                    Some("4") => {
                        let Some(state) = fields.next().and_then(|value| value.parse().ok()) else {
                            return;
                        };
                        let progress = fields
                            .next()
                            .and_then(|value| value.parse().ok())
                            .unwrap_or(0);
                        output
                            .events
                            .push(ParsedEvent::Progress(progress_state(state, progress)));
                    }
                    Some("9") => {
                        let path = fields.collect::<Vec<_>>().join(";");
                        let path = path.trim().trim_matches('"');
                        if !path.is_empty() {
                            output
                                .events
                                .push(ParsedEvent::WorkingDirectory(path.to_string()));
                        }
                    }
                    _ => {}
                }
            }
            "52" => {
                let Some(selector_field) = fields.next() else {
                    return;
                };
                let selector = selector_field.as_bytes().first().unwrap_or(&b'c');
                let target = match selector {
                    b'c' => ClipboardTarget::Clipboard,
                    b'p' | b's' => ClipboardTarget::Selection,
                    _ => return,
                };
                if let Some(encoded) = fields.next() {
                    if encoded == "?" && self.osc52.allows_paste() {
                        output
                            .events
                            .push(ParsedEvent::ClipboardLoad(ClipboardRequest {
                                target,
                                selector: *selector,
                                bell_terminated: terminator == b"\x07",
                            }));
                    } else if encoded != "?"
                        && self.osc52.allows_copy()
                        && let Some(bytes) = decode_base64(encoded.as_bytes())
                        && let Ok(text) = String::from_utf8(bytes)
                    {
                        output.events.push(ParsedEvent::ClipboardStore(text));
                    }
                }
            }
            "50" => {
                let Some(shape) = fields
                    .next()
                    .and_then(|value| value.strip_prefix("CursorShape="))
                    .and_then(|value| value.as_bytes().first())
                    .and_then(|value| match value {
                        b'0'..=b'2' => Some(u16::from(value - b'0')),
                        _ => None,
                    })
                else {
                    return;
                };
                grid.set_cursor_shape(shape);
            }
            "133" => match fields.next() {
                Some("A") => output.events.push(ParsedEvent::ShellPromptStart),
                Some("B") => output.events.push(ParsedEvent::ShellCommandStart),
                Some("C") => output.events.push(ParsedEvent::ShellCommandExecuting),
                Some("D") => {
                    let exit_code = fields.collect::<Vec<_>>().join(";").parse().ok();
                    output
                        .events
                        .push(ParsedEvent::ShellCommandFinished(exit_code));
                }
                _ => {}
            },
            "104" => {
                let values = fields.collect::<Vec<_>>();
                if values.is_empty() || values.first().is_some_and(|value| value.is_empty()) {
                    grid.reset_indexed_colors();
                } else {
                    for value in values {
                        if let Ok(index) = value.parse::<u8>() {
                            grid.reset_indexed_color(index);
                        }
                    }
                }
            }
            "110" => grid.set_foreground_color(None),
            "111" => grid.set_background_color(None),
            "112" => grid.set_cursor_color(None),
            _ => {}
        }
    }
}

fn push_color_reply(output: &mut ParseOutput, prefix: &str, color: Rgb, terminator: &[u8]) {
    output.replies.extend_from_slice(
        format!(
            "\x1b]{prefix};rgb:{0:02x}{0:02x}/{1:02x}{1:02x}/{2:02x}{2:02x}",
            color.r, color.g, color.b,
        )
        .as_bytes(),
    );
    output.replies.extend_from_slice(terminator);
}

fn parse_x_color(value: &str) -> Option<Rgb> {
    if let Some(value) = value.strip_prefix('#') {
        let component_len = value.len() / 3;
        if component_len == 0 || component_len > 4 || component_len * 3 != value.len() {
            return None;
        }
        let component = |start: usize| {
            let digits = &value[start..start + component_len];
            let parsed = u32::from_str_radix(digits, 16).ok()?;
            let maximum = 16u32.pow(component_len as u32).checked_sub(1)?;
            Some((255 * parsed / maximum) as u8)
        };
        return Some(Rgb {
            r: component(0)?,
            g: component(component_len)?,
            b: component(component_len * 2)?,
        });
    }

    let value = value.strip_prefix("rgb:")?;
    let mut components = value.split('/');
    let scale = |digits: &str| {
        if digits.is_empty() || digits.len() > 4 {
            return None;
        }
        let maximum = 16u32.pow(digits.len() as u32).checked_sub(1)?;
        let parsed = u32::from_str_radix(digits, 16).ok()?;
        Some((255 * parsed / maximum) as u8)
    };
    let color = Rgb {
        r: scale(components.next()?)?,
        g: scale(components.next()?)?,
        b: scale(components.next()?)?,
    };
    components.next().is_none().then_some(color)
}

fn dec_special_character(character: char) -> char {
    match character {
        '_' => ' ',
        '`' => '◆',
        'a' => '▒',
        'b' => '␉',
        'c' => '␌',
        'd' => '␍',
        'e' => '␊',
        'f' => '°',
        'g' => '±',
        'h' => '␤',
        'i' => '␋',
        'j' => '┘',
        'k' => '┐',
        'l' => '┌',
        'm' => '└',
        'n' => '┼',
        'o' => '⎺',
        'p' => '⎻',
        'q' => '─',
        'r' => '⎼',
        's' => '⎽',
        't' => '├',
        'u' => '┤',
        'v' => '┴',
        'w' => '┬',
        'x' => '│',
        'y' => '≤',
        'z' => '≥',
        '{' => 'π',
        '|' => '≠',
        '}' => '£',
        '~' => '·',
        _ => character,
    }
}

fn progress_state(state: u8, progress: u8) -> Progress {
    let progress = progress.min(100);
    match state {
        1 => Progress::InProgress(progress),
        2 => Progress::Error(progress),
        3 => Progress::Indeterminate,
        4 => Progress::Warning(progress),
        _ => Progress::Clear,
    }
}

fn osc7_path(value: &str) -> String {
    value
        .strip_prefix("file://")
        .and_then(|value| value.find('/').map(|index| &value[index..]))
        .unwrap_or(value)
        .to_string()
}

fn hex(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

fn decode_hex_ascii(input: &[u8]) -> Option<String> {
    if input.is_empty() || !input.len().is_multiple_of(2) {
        return None;
    }
    let mut decoded = Vec::with_capacity(input.len() / 2);
    for pair in input.chunks_exact(2) {
        decoded.push((hex(pair[0])? << 4) | hex(pair[1])?);
    }
    String::from_utf8(decoded).ok()
}

fn push_hex_ascii(output: &mut Vec<u8>, input: &[u8]) {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    output.reserve(input.len().saturating_mul(2));
    for byte in input {
        output.push(HEX[usize::from(byte >> 4)]);
        output.push(HEX[usize::from(byte & 0x0f)]);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::grid::{Cell, Color, CursorStyle};

    #[derive(Debug, PartialEq, Eq)]
    struct ReplayResult {
        snapshot: crate::Snapshot,
        lines: Vec<Vec<crate::Cell>>,
        palette: crate::Palette,
        bracketed_paste: bool,
        alternate_screen: bool,
        mouse: crate::MouseMode,
        keyboard: crate::KeyboardMode,
        graphics: Vec<(u32, u32, i32, usize, u32, u32)>,
        events: Vec<ParsedEvent>,
        replies: Vec<u8>,
        synchronized_update_active: bool,
    }

    fn parse(bytes: &[u8]) -> (Grid, ParseOutput) {
        let mut grid = Grid::new(12, 3, 8, CursorStyle::Block);
        grid.set_cell_metrics(9.0, 18.0);
        let mut parser = Parser::default();
        let output = parser.advance(&mut grid, bytes);
        (grid, output)
    }

    fn replay_chunks(bytes: &[u8], chunks: &[usize]) -> ReplayResult {
        let mut grid = Grid::new(12, 4, 8, CursorStyle::Block);
        grid.set_cell_metrics(9.0, 18.0);
        let mut parser = Parser::default();
        let mut events = Vec::new();
        let mut replies = Vec::new();
        let mut synchronized_update_active = false;
        let mut start = 0usize;
        for &end in chunks {
            let output = parser.advance(&mut grid, &bytes[start..end]);
            events.extend(output.events);
            replies.extend(output.replies);
            synchronized_update_active = output.synchronized_update_active;
            start = end;
        }
        if start < bytes.len() || chunks.is_empty() {
            let output = parser.advance(&mut grid, &bytes[start..]);
            events.extend(output.events);
            replies.extend(output.replies);
            synchronized_update_active = output.synchronized_update_active;
        }
        let (first, last) = grid.line_bounds();
        let lines = (first..=last)
            .map(|line| grid.line(line).unwrap().to_vec())
            .collect();
        let graphics = parser
            .graphics_placements(&grid)
            .into_iter()
            .map(|placement| {
                (
                    placement.image_id,
                    placement.placement_id,
                    placement.viewport_row,
                    placement.col,
                    placement.occupied_cols,
                    placement.occupied_rows,
                )
            })
            .collect();
        ReplayResult {
            snapshot: grid.snapshot(),
            lines,
            palette: grid.palette(),
            bracketed_paste: grid.bracketed_paste_mode(),
            alternate_screen: grid.alternate_screen_mode(),
            mouse: grid.mouse_mode(),
            keyboard: grid.keyboard_mode(),
            graphics,
            events,
            replies,
            synchronized_update_active,
        }
    }

    #[test]
    fn parses_text_cursor_motion_and_sgr() {
        let (grid, _) = parse(b"abc\x1b[2D\x1b[31;1mX");
        let row = grid.line(0).unwrap();
        assert_eq!(row[0].character, 'a');
        assert_eq!(row[1].character, 'X');
        assert_eq!(row[1].foreground, Color::Indexed(1));
        assert!(row[1].attributes.bold());
    }

    #[test]
    fn parses_colon_form_truecolor_palette_and_underline_styles() {
        let (grid, _) = parse(b"\x1b[38:2::12:34:56;48:5:200;4:3mX");
        let cell = grid.line(0).unwrap()[0];
        assert_eq!(
            cell.foreground,
            Color::Rgb {
                r: 12,
                g: 34,
                b: 56
            }
        );
        assert_eq!(cell.background, Color::Indexed(200));
        assert!(cell.attributes.underline());
        assert!(
            !cell.attributes.dim(),
            "underline style must not leak into SGR 2"
        );
    }

    #[test]
    fn overflowing_csi_parameters_discards_the_sequence() {
        let mut bytes = b"\x1b[".to_vec();
        for _ in 0..MAX_CSI_PARAMS {
            bytes.extend_from_slice(b"31;");
        }
        bytes.extend_from_slice(b"31mX");

        let (grid, _) = parse(&bytes);
        let cell = grid.line(0).unwrap()[0];
        assert_eq!(cell.character, 'X');
        assert_eq!(cell.foreground, Color::Default);
    }

    #[test]
    fn keeps_utf8_decoder_state_across_reads() {
        let mut grid = Grid::new(8, 2, 0, CursorStyle::Block);
        let mut parser = Parser::default();
        parser.advance(&mut grid, &[0xe2, 0x9d]);
        parser.advance(&mut grid, &[0xaf]);
        assert_eq!(grid.line(0).unwrap()[0].character, '❯');
    }

    #[test]
    fn encoded_c1_controls_are_ignored_without_replacing_the_last_printed_character() {
        let (grid, _) = parse("A\u{85}\u{9b}\x1b[2bB".as_bytes());
        let row = grid.line(0).unwrap();

        assert_eq!(
            row[..4]
                .iter()
                .map(|cell| cell.character)
                .collect::<String>(),
            "AAAB"
        );
        let mut first_combining = None;
        assert!(grid.for_each_line_cell(0, |col, _, combining| {
            if col == 0 {
                first_combining = combining.map(|combining| combining.to_owned_string());
            }
        }));
        assert_eq!(first_combining, None);
    }

    #[test]
    fn ground_text_spans_preserve_invalid_and_split_utf8_behavior() {
        let fixture = b"A\xe2\x28\xa1B\xc0\xafC\xf0\x9f\x99\x82D\xf0\x9f";
        let expected = replay_chunks(fixture, &[]);
        let every_byte = (1..fixture.len()).collect::<Vec<_>>();
        assert_eq!(replay_chunks(fixture, &every_byte), expected);

        let (grid, _) = parse(fixture);
        let rendered = grid.line(0).unwrap()[..11]
            .iter()
            .filter(|cell| !cell.wide_spacer())
            .map(|cell| cell.character)
            .collect::<String>();
        assert_eq!(rendered, "A�(�B��C🙂D");
    }

    #[test]
    fn ground_text_spans_consume_long_malformed_runs_in_one_pass() {
        let mut parser = Parser::default();
        let mut grid = Grid::new(80, 24, 0, CursorStyle::Block);
        let mut output = ParseOutput::default();
        let malformed = vec![0x80; 16 * 1024];

        assert_eq!(
            parser.advance_ground_text(&mut grid, &malformed, &mut output),
            malformed.len()
        );
        assert!(!parser.utf8.is_pending());
    }

    #[test]
    fn long_ascii_spans_in_slow_grid_modes_match_chunked_parsing() {
        for prefix in [b"\x1b[4h".as_slice(), b"\x1b[?7l".as_slice()] {
            let mut fixture = prefix.to_vec();
            fixture.extend(std::iter::repeat_n(b'a', 8 * 1024));
            let chunks = (257..fixture.len()).step_by(257).collect::<Vec<_>>();

            assert_eq!(
                replay_chunks(&fixture, &chunks),
                replay_chunks(&fixture, &[])
            );
        }
    }

    #[test]
    fn osc_ignores_embedded_controls_and_dispatches_before_cancel() {
        let (grid, output) = parse(b"\x1b]2;ti\x01tle\x18X");
        assert_eq!(output.events, vec![ParsedEvent::Title("title".to_string())]);
        assert_eq!(grid.line(0).unwrap()[0].character, 'X');
    }

    #[test]
    fn osc52_defaults_an_empty_selector_to_the_clipboard() {
        let (_, output) = parse(b"\x1b]52;;dG1vbg==\x07");
        assert_eq!(
            output.events,
            vec![ParsedEvent::ClipboardStore("tmon".to_string())]
        );
    }

    #[test]
    fn every_input_split_matches_one_shot_parsing() {
        let mut fixture = "ab界".as_bytes().to_vec();
        fixture.extend_from_slice(
            b"\x1b[31;1mR\x1b[0m\x1b]2;split-safe\x1b\\\
              \x1b]4;3;#123456\x07\x1b]8;;https://example.com\x1b\\link\x1b]8;;\x1b\\\
              \r\nline-2\r\nline-3\r\nline-4\r\nline-5\
              \x1b[2;2H\x1b_Ga=T,f=32,s=1,v=1,i=77,p=3,c=2,r=1,C=1,q=1;/wAA/w==\x1b\\Z\
              \x1bP+q544e;5463\x1b\\\x1b[?2004h\x1b[?1006h\
              \x1b[?2026hbatched\x1b[?2026l\x1b[6n",
        );
        let expected = replay_chunks(&fixture, &[]);
        for split in 0..=fixture.len() {
            assert_eq!(
                replay_chunks(&fixture, &[split]),
                expected,
                "parser state diverged at byte split {split}"
            );
        }
    }

    #[test]
    fn kitty_apc_terminator_is_chunk_safe() {
        let mut grid = Grid::new(12, 4, 8, CursorStyle::Block);
        let mut parser = Parser::default();

        let pending = parser.advance(
            &mut grid,
            b"\x1b_Ga=T,f=32,s=1,v=1,i=77,p=3,c=2,r=1,C=1;/wAA/w==\x1b",
        );
        assert!(pending.replies.is_empty());
        assert!(parser.graphics_placements(&grid).is_empty());
        assert_eq!(parser.state, State::KittyEscape);

        let finished = parser.advance(&mut grid, b"\\");
        assert_eq!(finished.replies, b"\x1b_Gi=77,p=3;OK\x1b\\");
        assert_eq!(parser.graphics_placements(&grid).len(), 1);
        assert_eq!(parser.state, State::Ground);
    }

    #[test]
    fn kitty_apc_accepts_the_native_engines_eight_bit_introducer() {
        let mut parser = Parser::default();
        let mut grid = Grid::new(8, 2, 0, CursorStyle::Block);
        let output = parser.advance(&mut grid, b"\x9fGa=T,f=32,s=1,v=1,i=7;/wAA/w==\x9cZ");

        assert_eq!(grid.line(1).unwrap()[1].character, 'Z');
        assert_eq!(parser.graphics_placements(&grid).len(), 1);
        assert!(output.replies.starts_with(b"\x1b_Gi=7;"));
    }

    #[test]
    fn kitty_apc_preserves_non_terminating_escape_bytes_across_chunks() {
        let mut grid = Grid::new(12, 4, 8, CursorStyle::Block);
        let mut parser = Parser::default();

        let first = parser.advance(
            &mut grid,
            b"\x1b_Ga=T,f=32,s=1,v=1,i=78,C=1,q=1;/wAA/w==\x1b",
        );
        assert!(first.replies.is_empty());
        assert_eq!(parser.state, State::KittyEscape);

        let middle = parser.advance(&mut grid, b"x\x1b");
        assert!(middle.replies.is_empty());
        assert!(parser.graphics_placements(&grid).is_empty());
        assert_eq!(parser.state, State::KittyEscape);

        let finished = parser.advance(&mut grid, b"\\Z");
        assert_eq!(
            finished.replies,
            b"\x1b_Gi=78;EINVAL:invalid base64 payload\x1b\\"
        );
        assert!(parser.graphics_placements(&grid).is_empty());
        assert_eq!(grid.line(0).unwrap()[0].character, 'Z');
        assert_eq!(parser.state, State::Ground);
    }

    #[test]
    fn malformed_random_streams_keep_grid_and_graphics_invariants() {
        let mut grid = Grid::new(31, 7, 16, CursorStyle::Block);
        let mut parser = Parser::default();
        let mut seed = 0x6a09_e667_f3bc_c909u64;
        let mut bytes = Vec::with_capacity(128 * 1024);
        for index in 0..128 * 1024 {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            bytes.push(seed as u8);
            if index % 257 == 0 {
                bytes.extend_from_slice("界\x1b[31mR\x1b[0m".as_bytes());
            }
        }

        for chunk in bytes.chunks(113) {
            parser.advance(&mut grid, chunk);
            let (col, row) = grid.cursor_position();
            assert!(col < grid.cols());
            assert!(row < grid.rows());
            assert!(grid.history_size() <= 16);
            let (first, last) = grid.line_bounds();
            for line_index in first..=last {
                let line = grid.line(line_index).unwrap();
                assert_eq!(line.len(), grid.cols());
                for (index, cell) in line.iter().enumerate() {
                    if cell.wide_spacer() {
                        assert!(index > 0);
                        assert!(!line[index - 1].wide_spacer());
                    }
                }
            }
            assert!(parser.graphics_placements(&grid).len() <= 4096);
        }
    }

    #[test]
    fn parses_alt_screen_and_bracketed_paste_modes() {
        let (grid, _) = parse(b"main\x1b[?1049hX\x1b[?2004h");
        assert!(grid.alternate_screen_mode());
        assert!(grid.bracketed_paste_mode());
        assert_eq!(grid.line(0).unwrap()[4].character, 'X');
    }

    #[test]
    fn emits_title_shell_and_clipboard_events() {
        let (_, output) = parse(b"\x1b]2;hello\x07\x1b]133;C\x07\x1b]52;c;dGVybXk=\x07");
        assert_eq!(
            output.events,
            vec![
                ParsedEvent::Title("hello".to_string()),
                ParsedEvent::ShellCommandExecuting,
                ParsedEvent::ClipboardStore("termy".to_string()),
            ]
        );
    }

    #[test]
    fn custom_osc_events_preserve_semicolons_and_reject_malformed_progress() {
        let (_, output) = parse(
            b"\x1b]7;file://host/tmp/a%20b;c\x07\
              \x1b]9;9;\"/tmp/d;e\"\x07\
              \x1b]9;4;invalid;50\x07\
              \x1b]9;4;1;150\x07\
              \x1b]133;D;7;ignored\x07",
        );
        assert_eq!(
            output.events,
            vec![
                ParsedEvent::WorkingDirectory("/tmp/a%20b;c".to_string()),
                ParsedEvent::WorkingDirectory("/tmp/d;e".to_string()),
                ParsedEvent::Progress(Progress::InProgress(100)),
                ParsedEvent::ShellCommandFinished(None),
            ]
        );
    }

    #[test]
    fn title_osc_trims_text_and_ignores_a_missing_parameter() {
        let (_, output) = parse(b"\x1b]2\x07\x1b]2;  hello world  \x07");
        assert_eq!(
            output.events,
            vec![ParsedEvent::Title("hello world".to_string())]
        );
    }

    #[test]
    fn osc_cursor_shape_updates_the_rendered_cursor() {
        let (grid, _) = parse(b"\x1b]50;CursorShape=1\x07");
        assert_eq!(grid.cursor_state().unwrap().style, CursorStyle::Line);

        let (grid, _) = parse(b"\x1b]50;CursorShape=0\x1b\\");
        assert_eq!(grid.cursor_state().unwrap().style, CursorStyle::Block);
    }

    #[test]
    fn osc_cursor_shape_matches_vte_first_byte_compatibility() {
        let (grid, _) = parse(b"\x1b]50;CursorShape=10\x07");
        assert_eq!(grid.cursor_style_status(), 6);

        let (grid, _) = parse(b"\x1b]50;CursorShape=20\x07");
        assert_eq!(grid.cursor_style_status(), 4);

        let (grid, _) = parse(b"\x1b]50;CursorShape=9\x07");
        assert_eq!(grid.cursor_state().unwrap().style, CursorStyle::Block);
    }

    #[test]
    fn cursor_shape_queries_preserve_blinking_and_underlying_shape() {
        let (grid, output) = parse(
            b"\x1b[3 q\x1bP$q q\x1b\\\
              \x1b[?12l\x1bP$q q\x1b\\\
              \x1b]50;CursorShape=1\x07\x1bP$q q\x1b\\\
              \x1b[?12h\x1b[99 q\x1bP$q q\x1b\\",
        );
        assert_eq!(grid.cursor_state().unwrap().style, CursorStyle::Line);
        assert_eq!(
            output.replies,
            b"\x1bP1$r3 q\x1b\\\x1bP1$r4 q\x1b\\\x1bP1$r6 q\x1b\\\x1bP1$r5 q\x1b\\"
        );
    }

    #[test]
    fn saves_and_restores_window_titles() {
        let (_, output) = parse(b"\x1b]2;A\x07\x1b[22;1t\x1b]2;B\x07\x1b[23;1t");
        assert_eq!(
            output.events,
            vec![
                ParsedEvent::Title("A".to_string()),
                ParsedEvent::Title("B".to_string()),
                ParsedEvent::Title("A".to_string()),
            ]
        );

        let (_, output) = parse(b"\x1b]2;one\x07\x1b[22;2t\x1b]2;two\x07\x1b[23;2t\x1b[23;2t");
        assert_eq!(
            output.events,
            vec![
                ParsedEvent::Title("one".to_string()),
                ParsedEvent::Title("two".to_string()),
                ParsedEvent::Title("one".to_string()),
            ]
        );
    }

    #[test]
    fn window_title_stack_matches_alacritty_depth_limit() {
        let mut grid = Grid::new(12, 3, 8, CursorStyle::Block);
        let mut parser = Parser::default();

        for index in 0..=TITLE_STACK_MAX_DEPTH {
            parser.title = Some(index.to_string());
            parser.advance(&mut grid, b"\x1b[22;1t");
        }

        assert_eq!(parser.title_stack.len(), TITLE_STACK_MAX_DEPTH);
        assert_eq!(
            parser.title_stack.first().and_then(Option::as_deref),
            Some("1")
        );
        assert_eq!(
            parser.title_stack.last().and_then(Option::as_deref),
            Some("4096")
        );
    }

    #[test]
    fn answers_cursor_position_queries() {
        let (_, output) = parse(b"ab\x1b[6n");
        assert_eq!(output.replies, b"\x1b[1;3R");
    }

    #[test]
    fn dec_special_graphics_render_tui_box_drawing() {
        let (grid, _) = parse(b"\x1b(0_`lqk\x1b(B");
        let rendered = grid.line(0).unwrap()[..5]
            .iter()
            .map(|cell| cell.character)
            .collect::<String>();
        assert_eq!(rendered, " ◆┌─┐");
    }

    #[test]
    fn alternate_screen_charset_designation_does_not_leak_to_primary() {
        let (grid, _) = parse(b"\x1b[?1049h\x1b(0x\x1b[?1049lx");
        assert_eq!(grid.line(0).unwrap()[0].character, 'x');

        let (grid, _) = parse(b"\x1b7\x1b(0\x1b[?1049htext\x1b[?1049l\x1b8_x");
        assert_eq!(grid.line(0).unwrap()[0].character, ' ');
        assert_eq!(grid.line(0).unwrap()[1].character, '│');
    }

    #[test]
    fn supports_alignment_test_g2_g3_and_single_shift_charsets() {
        let (grid, _) = parse(b"\x1b#8\x1b[H\x1b*A\x1bN#\x1b+0\x1bOq");
        let row = grid.line(0).unwrap();
        assert_eq!(row[0].character, '£');
        assert_eq!(row[1].character, '─');
        assert!(row[2..].iter().all(|cell| cell.character == 'E'));
        assert!(
            grid.line(1)
                .unwrap()
                .iter()
                .all(|cell| cell.character == 'E')
        );
    }

    #[test]
    fn saved_cursor_and_alternate_screen_preserve_rendition_state() {
        let (grid, _) = parse(b"\x1b[31m\x1b7\x1b[32mG\x1b8R\x1b[?1049hA\x1b[34mB\x1b[?1049lP");
        let primary = grid.line(0).unwrap();
        assert_eq!(primary[0].character, 'R');
        assert_eq!(primary[0].foreground, Color::Indexed(1));
        assert_eq!(primary[1].character, 'P');
        assert_eq!(primary[1].foreground, Color::Indexed(1));
    }

    #[test]
    fn erasures_use_the_current_background_and_keep_wide_pairs_valid() {
        let (grid, _) = parse("\x1b[44mabc\x1b[2K\x1b[49m界\x1b[2DX".as_bytes());
        let row = grid.line(0).unwrap();
        assert!(
            row[..3]
                .iter()
                .all(|cell| cell.background == Color::Indexed(4))
        );
        assert_eq!(row[3].character, 'X');
        assert!(!row[4].wide_spacer());
    }

    #[test]
    fn selective_erase_preserves_protected_narrow_and_wide_cells() {
        let mut grid = Grid::new(12, 3, 0, CursorStyle::Block);
        let mut parser = Parser::default();
        let output = parser.advance(
            &mut grid,
            "\x1b[1\"q\x1b[31mA\x1b[0m\x1b[0\"qB\x1b[1\"q界\x1b[0\"qC\
             \x1b[1;1H\x1b[?2K\x1b[1\"q\x1bP$q\"q\x1b\\"
                .as_bytes(),
        );

        let row = grid.line(0).unwrap();
        assert_eq!(row[0].character, 'A');
        assert!(row[0].protected());
        assert_eq!(row[1].character, ' ');
        assert_eq!(row[2].character, '界');
        assert!(row[2].protected());
        assert!(row[3].wide_spacer());
        assert!(row[3].protected());
        assert_eq!(row[4].character, ' ');
        assert_eq!(output.replies, b"\x1bP1$r1\"q\x1b\\");

        parser.advance(&mut grid, b"\x1b[2K");
        assert!(
            grid.line(0)
                .unwrap()
                .iter()
                .all(|cell| *cell == crate::Cell::default())
        );
    }

    #[test]
    fn erase_to_end_of_line_respects_pending_wrap() {
        let (grid, _) = parse(b"abcdefghijkl\x1b[K");
        assert_eq!(
            grid.line(0)
                .unwrap()
                .iter()
                .map(|cell| cell.character)
                .collect::<String>(),
            "abcdefghijkl"
        );
    }

    #[test]
    fn osc8_hyperlinks_attach_to_cells_and_close_cleanly() {
        let (grid, _) =
            parse(b"\x1b]8;id=docs;https://example.com/reference\x1b\\read\x1b]8;;\x1b\\ plain");

        let link = grid.hyperlink_at(0, 2).expect("OSC 8 link should exist");
        assert_eq!((link.start_col, link.end_col), (0, 3));
        assert_eq!(link.target, "https://example.com/reference");
        assert_eq!(grid.hyperlink_at(0, 5), None);
    }

    #[test]
    fn osc8_ranges_use_the_protocol_id_and_uri_identity() {
        let uri = "https://example.com/same";
        let bytes = format!(
            "\x1b]8;foo=bar:id=one;{uri}\x1b\\A\
             \x1b]8;id=two;{uri}\x1b\\B\
             \x1b]8;;{uri}\x1b\\C\
             \x1b]8;;{uri}\x1b\\D\
             \x1b]8;id=one;{uri}\x1b\\E"
        );
        let (grid, _) = parse(bytes.as_bytes());

        for col in 0..5 {
            let link = grid
                .hyperlink_at(0, col)
                .expect("cell should carry an OSC 8 link");
            assert_eq!((link.start_col, link.end_col), (col, col));
            assert_eq!(link.target, uri);
        }

        let bytes = format!(
            "\x1b]8;id=shared;{uri}\x1b\\A\x1b]8;;\x1b\\\
             \x1b]8;id=shared;{uri}\x1b\\B"
        );
        let (grid, _) = parse(bytes.as_bytes());
        let link = grid.hyperlink_at(0, 0).expect("reopened link should exist");
        assert_eq!((link.start_col, link.end_col), (0, 1));
    }

    #[test]
    fn answers_and_updates_indexed_and_dynamic_color_queries() {
        let (grid, output) = parse(
            b"\x1b]4;1;?\x07\x1b]4;1;#123456\x1b\\\x1b]4;1;?\x1b\\\
              \x1b]10;?\x07\x1b]11;rgb:f/8/0\x07\x1b]11;?\x07\
              \x1b]12;?\x07\x1b]12;#abcdef\x07\x1b]12;?\x07",
        );

        assert_eq!(
            grid.palette().indexed(1),
            Some(Rgb {
                r: 0x12,
                g: 0x34,
                b: 0x56
            })
        );
        assert_eq!(
            grid.palette().background(),
            Some(Rgb {
                r: 0xff,
                g: 0x88,
                b: 0x00
            })
        );
        assert_eq!(
            grid.palette().cursor(),
            Some(Rgb {
                r: 0xab,
                g: 0xcd,
                b: 0xef
            })
        );
        assert_eq!(
            output.replies,
            b"\x1b]4;1;rgb:cdcd/0000/0000\x07\
              \x1b]4;1;rgb:1212/3434/5656\x1b\\\
              \x1b]10;rgb:e5e5/e5e5/e5e5\x07\
              \x1b]11;rgb:ffff/8888/0000\x07\
              \x1b]12;rgb:abab/cdcd/efef\x07"
        );
    }

    #[test]
    fn color_resets_restore_query_fallbacks() {
        let (grid, output) = parse(
            b"\x1b]4;2;#abcdef\x07\x1b]104;2\x07\x1b]4;2;?\x07\
              \x1b]10;#010203\x07\x1b]110\x07\x1b]10;?\x07",
        );

        assert_eq!(grid.palette().indexed(2), None);
        assert_eq!(grid.palette().foreground(), None);
        assert_eq!(
            output.replies,
            b"\x1b]4;2;rgb:0000/cdcd/0000\x07\x1b]10;rgb:e5e5/e5e5/e5e5\x07"
        );
    }

    #[test]
    fn xparsecolor_expands_short_hex_components() {
        assert_eq!(
            parse_x_color("#abc"),
            Some(Rgb {
                r: 0xaa,
                g: 0xbb,
                b: 0xcc,
            })
        );
        let (grid, _) = parse(b"\x1b[4:1mA\x1b[4:0mB");
        assert!(grid.line(0).unwrap()[0].attributes.underline());
        assert!(!grid.line(0).unwrap()[1].attributes.underline());
    }

    #[test]
    fn reports_modes_keyboard_flags_and_text_area_size() {
        let (_, output) = parse(
            b"\x1b[?2004h\x1b[?2004$p\x1b[4$p\x1b[=5u\x1b[?u\
              \x1b[14t\x1b[18t",
        );

        assert_eq!(
            output.replies,
            b"\x1b[?2004;1$y\x1b[4;2$y\x1b[?0u\x1b[4;54;108t\x1b[8;3;12t"
        );
    }

    #[test]
    fn keyboard_mode_query_reports_the_stack_top_like_alacritty() {
        let (grid, output) = parse(b"\x1b[>1u\x1b[=2;2u\x1b[?u");

        let keyboard = grid.keyboard_mode();
        assert!(keyboard.disambiguate_escape_codes);
        assert!(keyboard.report_event_types);
        assert_eq!(output.replies, b"\x1b[?1u");
    }

    #[test]
    fn reports_alacritty_compatible_default_private_modes() {
        let (_, output) = parse(b"\x1b[?7$p\x1b[?25$p\x1b[?1007$p\x1b[?1042$p");

        assert_eq!(
            output.replies,
            b"\x1b[?7;1$y\x1b[?25;1$y\x1b[?1007;1$y\x1b[?1042;1$y"
        );
    }

    #[test]
    fn custom_tab_stops_drive_forward_and_backward_tabulation() {
        let (grid, _) = parse(b"\x1b[3g\x1b[5G\x1bH\r\tX\x1b[2IY\x1b[1ZZ");
        assert_eq!(grid.line(0).unwrap()[4].character, 'X');
        assert_eq!(grid.line(0).unwrap()[11].character, 'Y');
        assert_eq!(grid.line(1).unwrap()[0].character, 'Z');

        let (grid, _) = parse(b"\x1b[3g\x1b[6G\x1b[Z");
        assert_eq!(grid.cursor_position(), (5, 0));
    }

    #[test]
    fn tabs_leave_reflow_markers_and_preserve_pending_wrap_for_csi_motion() {
        let mut parser = Parser::default();
        let mut grid = Grid::new(12, 3, 0, CursorStyle::Block);
        parser.advance(&mut grid, b"a\tb\x1b[3g\r\x1b[5G\x1bH\r\tX\x1b[2IY\x1b[1Zz");

        assert_eq!(grid.line(0).unwrap()[1].character, '\t');
        assert_eq!(grid.line(0).unwrap()[4].character, 'X');
        assert!(grid.line(0).unwrap()[4].wrapped());
        assert_eq!(grid.line(1).unwrap()[0].character, 'z');
    }

    #[test]
    fn tab_does_not_wrap_when_autowrap_is_disabled() {
        let mut parser = Parser::default();
        let mut grid = Grid::new(4, 2, 0, CursorStyle::Block);
        parser.advance(&mut grid, b"\x1b[?7l\x1b[4GX\t");

        assert_eq!(grid.cursor_position(), (3, 0));
        assert_eq!(grid.line(0).unwrap()[3].character, 'X');
        assert!(!grid.line(0).unwrap()[3].wrapped());
    }

    #[test]
    fn newline_mode_is_reported_but_raw_line_feed_preserves_the_column() {
        let mut parser = Parser::default();
        let mut grid = Grid::new(8, 3, 0, CursorStyle::Block);
        let output = parser.advance(&mut grid, b"abc\x1b[20h\nX\x1b[20$p");

        assert_eq!(grid.cursor_position(), (4, 1));
        assert_eq!(grid.line(1).unwrap()[3].character, 'X');
        assert_eq!(output.replies, b"\x1b[20;1$y");
    }

    #[test]
    fn standalone_c1_bytes_are_ignored_in_utf8_mode() {
        let mut parser = Parser::default();
        let mut grid = Grid::new(12, 2, 0, CursorStyle::Block);
        parser.advance(&mut grid, b"abc\x85x\x9b2;3H@\x84y\x8dz");

        assert_eq!(
            grid.line(0).unwrap()[..11]
                .iter()
                .map(|cell| cell.character)
                .collect::<String>(),
            "abcx2;3H@yz"
        );
    }

    #[test]
    fn repeat_without_a_preceding_character_is_a_noop() {
        let mut parser = Parser::default();
        let mut grid = Grid::new(8, 2, 0, CursorStyle::Block);
        parser.advance(&mut grid, b"\x1b[2C\x1b[3b");

        assert_eq!(grid.cursor_position(), (2, 0));
        assert!(
            grid.line(0)
                .unwrap()
                .iter()
                .all(|cell| *cell == Cell::default())
        );
    }

    #[test]
    fn repeat_uses_the_last_character_from_an_ascii_span() {
        let (grid, _) = parse(b"abc\x1b[3b");

        assert_eq!(
            grid.line(0).unwrap()[..6]
                .iter()
                .map(|cell| cell.character)
                .collect::<String>(),
            "abcccc"
        );
    }

    #[test]
    fn disabling_origin_mode_keeps_the_active_cursor_position() {
        let mut parser = Parser::default();
        let mut grid = Grid::new(8, 4, 0, CursorStyle::Block);
        parser.advance(&mut grid, b"\x1b[2;4r\x1b[?6h\x1b[2;3H\x1b[?6l");

        assert_eq!(grid.cursor_position(), (2, 2));
    }

    #[test]
    fn scroll_regions_clamp_the_bottom_and_home_the_cursor() {
        let mut parser = Parser::default();
        let mut grid = Grid::new(8, 4, 0, CursorStyle::Block);
        parser.advance(&mut grid, b"\x1b[4;5H\x1b[2;99r");
        assert_eq!(grid.scroll_region_status(), (2, 4));
        assert_eq!(grid.cursor_position(), (0, 0));

        parser.advance(&mut grid, b"\x1b[3;4H\x1b[r");
        assert_eq!(grid.scroll_region_status(), (1, 4));
        assert_eq!(grid.cursor_position(), (0, 0));
    }

    #[test]
    fn synchronized_updates_expose_batch_state_until_esu() {
        let mut grid = Grid::new(8, 2, 0, CursorStyle::Block);
        let mut parser = Parser::default();

        assert!(
            parser
                .advance(&mut grid, b"\x1b[?2026hframe")
                .synchronized_update_active
        );
        assert!(
            parser
                .advance(&mut grid, b"-body")
                .synchronized_update_active
        );
        assert!(
            !parser
                .advance(&mut grid, b"\x1b[?2026l")
                .synchronized_update_active
        );
    }

    #[test]
    fn synchronized_updates_stage_grid_events_and_replies_atomically() {
        let mut grid = Grid::new(12, 2, 0, CursorStyle::Block);
        let mut parser = Parser::default();

        let staged = parser.advance(&mut grid, b"\x1b[?2026hframe\x07\x1b]2;batched\x07\x1b[6n");
        assert!(staged.synchronized_update_active);
        assert!(staged.events.is_empty());
        assert!(staged.replies.is_empty());
        assert_eq!(grid.line(0).unwrap()[0].character, ' ');

        let committed = parser.advance(&mut grid, ESU_CSI);
        assert!(!committed.synchronized_update_active);
        assert_eq!(
            grid.line(0).unwrap()[..5]
                .iter()
                .map(|cell| cell.character)
                .collect::<String>(),
            "frame"
        );
        assert_eq!(
            committed.events,
            vec![ParsedEvent::Bell, ParsedEvent::Title("batched".to_string())]
        );
        assert_eq!(committed.replies, b"\x1b[1;6R");
    }

    #[test]
    fn synchronized_update_initial_bsu_can_share_a_private_mode_sequence() {
        let mut grid = Grid::new(8, 2, 0, CursorStyle::Block);
        let mut parser = Parser::default();

        let staged = parser.advance(&mut grid, b"\x1b[?2004;2026hX");
        assert!(staged.synchronized_update_active);
        assert_eq!(grid.line(0).unwrap()[0].character, ' ');
        assert!(grid.bracketed_paste_mode());

        parser.advance(&mut grid, ESU_CSI);
        assert_eq!(grid.line(0).unwrap()[0].character, 'X');
    }

    #[test]
    fn exact_synchronized_update_end_is_chunk_safe_at_every_split() {
        for split in 0..=ESU_CSI.len() {
            let mut grid = Grid::new(8, 2, 0, CursorStyle::Block);
            let mut parser = Parser::default();
            parser.advance(&mut grid, b"\x1b[?2026hX");

            let pending = parser.advance(&mut grid, &ESU_CSI[..split]);
            if split < ESU_CSI.len() {
                assert!(pending.synchronized_update_active, "split {split}");
                assert_eq!(grid.line(0).unwrap()[0].character, ' ', "split {split}");
            }
            let committed = parser.advance(&mut grid, &ESU_CSI[split..]);
            assert!(!committed.synchronized_update_active, "split {split}");
            assert_eq!(grid.line(0).unwrap()[0].character, 'X', "split {split}");
        }
    }

    #[test]
    fn synchronized_scanner_requires_exact_raw_end_and_retains_later_bsu() {
        let mut grid = Grid::new(12, 2, 0, CursorStyle::Block);
        let mut parser = Parser::default();

        let pending = parser.advance(&mut grid, b"\x1b[?2026hA\x1b[?2026;25lB");
        assert!(pending.synchronized_update_active);
        assert_eq!(grid.line(0).unwrap()[0].character, ' ');

        let extended = parser.advance(&mut grid, b"\x1b[?2026l\x1b[?2026hC");
        assert!(extended.synchronized_update_active);
        assert_eq!(
            grid.line(0).unwrap()[..2]
                .iter()
                .map(|cell| cell.character)
                .collect::<String>(),
            "AB"
        );

        parser.advance(&mut grid, ESU_CSI);
        assert_eq!(
            grid.line(0).unwrap()[..3]
                .iter()
                .map(|cell| cell.character)
                .collect::<String>(),
            "ABC"
        );
    }

    #[test]
    fn explicit_sync_stop_commits_buffered_protocol_replies() {
        let mut grid = Grid::new(8, 2, 0, CursorStyle::Block);
        let mut parser = Parser::default();
        parser.advance(&mut grid, b"\x1b[?2026hX\x1b[6n");

        let committed = parser.stop_synchronized_update(&mut grid);
        assert!(!committed.synchronized_update_active);
        assert_eq!(grid.line(0).unwrap()[0].character, 'X');
        assert_eq!(committed.replies, b"\x1b[1;2R");
    }

    #[test]
    fn synchronized_update_capacity_flushes_before_exceeding_two_mibibytes() {
        let mut grid = Grid::new(8, 2, 0, CursorStyle::Block);
        let mut parser = Parser::default();
        parser.advance(&mut grid, BSU_CSI);
        let oversized = vec![b'X'; MAX_SYNC_BYTES - 1];

        let flushed = parser.advance(&mut grid, &oversized);
        assert!(!flushed.synchronized_update_active);
        assert!(flushed.synchronized_update_committed);
        assert_eq!(grid.line(0).unwrap()[0].character, 'X');
    }

    #[test]
    fn cancel_and_substitute_abort_incomplete_control_sequences() {
        let (grid, _) = parse(b"\x1b[31\x18mX\x1b[32\x1aY");
        let row = grid.line(0).unwrap();
        assert_eq!(row[0].character, 'm');
        assert_eq!(row[1].character, 'X');
        assert_eq!(row[2].character, 'Y');
        assert!(
            row[..3]
                .iter()
                .all(|cell| cell.foreground == Color::Default)
        );
    }

    #[test]
    fn oversized_osc_and_dcs_payloads_are_discarded() {
        let mut bytes = b"\x1b]2;".to_vec();
        bytes.resize(MAX_OSC_BYTES + 32, b'x');
        bytes.push(0x07);
        bytes.extend_from_slice(b"\x1bP$q");
        bytes.resize(bytes.len() + MAX_DCS_BYTES + 1, b'm');
        bytes.extend_from_slice(b"\x1b\\");

        let (_, output) = parse(&bytes);
        assert!(output.events.is_empty());
        assert!(output.replies.is_empty());
    }

    #[test]
    fn answers_decrqss_and_xtgettcap_queries() {
        let (_, output) = parse(
            b"\x1b[31;1m\x1b[2;3r\
              \x1bP$qm\x1b\\\x1bP$qr\x1b\\\x1bP$q q\x1b\\\
              \x1bP+q544e;436f;5463;5a5a\x1b\\",
        );

        assert_eq!(
            output.replies,
            b"\x1bP1$r1;31m\x1b\\\x1bP1$r2;3r\x1b\\\x1bP1$r2 q\x1b\\\
              \x1bP1+r544e=787465726d2d323536636f6c6f72\x1b\\\
              \x1bP1+r436f=323536\x1b\\\x1bP1+r5463\x1b\\\x1bP0+r5a5a\x1b\\"
        );
    }
}
