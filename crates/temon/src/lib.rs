//! A small experimental terminal engine for Termy.
//!
//! Tmon deliberately uses only the Rust standard library. It owns its VT parser,
//! bounded grid/scrollback, damage tracking, and (on Unix) PTY lifecycle. The
//! desktop app keeps it behind `TERMY_EXPERIMENTAL_TMON_ENGINE=1`; the regular
//! native and tmux engines do not depend on this runtime path.

mod graphics;
mod grid;
mod parser;
#[cfg(unix)]
mod pty;
mod unicode_width;

use std::{
    collections::VecDeque,
    fmt, io,
    path::PathBuf,
    sync::{
        Arc, Mutex, OnceLock,
        atomic::{AtomicBool, AtomicU64, Ordering},
        mpsc,
    },
    time::{Duration, Instant},
};

pub use graphics::GraphicsRenderPlacement;
pub use grid::{
    Attributes, Cell, Color, Combining, CursorState, CursorStyle, DamageSnapshot, DirtySpan,
    Hyperlink, KeyboardMode, MouseMode, Palette, Rgb, Snapshot,
};
pub use parser::{Progress, QueryColors};

use grid::Grid;
use parser::{ParsedEvent, Parser};

const MAX_DRAIN_EVENTS: usize = 2048;
const MAX_QUEUED_EVENTS: usize = 8192;
const MAX_GRID_DIMENSION: u16 = 4096;
const SYNC_WATCHDOG_DELAY: Duration = Duration::from_millis(175);

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Size {
    pub cols: u16,
    pub rows: u16,
    pub cell_width: f32,
    pub cell_height: f32,
}

impl Default for Size {
    fn default() -> Self {
        Self {
            cols: 80,
            rows: 24,
            cell_width: 9.0,
            cell_height: 18.0,
        }
    }
}

impl Size {
    fn clamped(self) -> Self {
        Self {
            cols: self.cols.clamp(1, MAX_GRID_DIMENSION),
            rows: self.rows.clamp(1, MAX_GRID_DIMENSION),
            cell_width: finite_cell_metric(self.cell_width),
            cell_height: finite_cell_metric(self.cell_height),
        }
    }
}

fn finite_cell_metric(value: f32) -> f32 {
    if value.is_finite() {
        value.max(1.0)
    } else {
        1.0
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Launch {
    ShellCommand(String),
    Program { program: String, args: Vec<String> },
}

#[derive(Clone, Debug)]
pub struct Config {
    pub shell: Option<String>,
    pub working_directory: Option<PathBuf>,
    pub environment: Vec<(String, String)>,
    pub launch: Option<Launch>,
    pub scrollback_history: usize,
    pub default_cursor_style: CursorStyle,
    pub osc52: Osc52,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            shell: None,
            working_directory: None,
            environment: Vec::new(),
            launch: None,
            scrollback_history: 1000,
            default_cursor_style: CursorStyle::Block,
            osc52: Osc52::OnlyCopy,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Osc52 {
    Disabled,
    #[default]
    OnlyCopy,
    OnlyPaste,
    CopyPaste,
}

impl Osc52 {
    fn allows_copy(self) -> bool {
        matches!(self, Self::OnlyCopy | Self::CopyPaste)
    }

    fn allows_paste(self) -> bool {
        matches!(self, Self::OnlyPaste | Self::CopyPaste)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ClipboardTarget {
    Clipboard,
    Selection,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ClipboardRequest {
    target: ClipboardTarget,
    selector: u8,
    bell_terminated: bool,
}

impl ClipboardRequest {
    pub fn target(self) -> ClipboardTarget {
        self.target
    }

    pub fn format_reply(self, text: &str) -> Vec<u8> {
        let mut reply = format!(
            "\x1b]52;{};{}",
            char::from(self.selector),
            encode_base64(text.as_bytes())
        )
        .into_bytes();
        if self.bell_terminated {
            reply.push(0x07);
        } else {
            reply.extend_from_slice(b"\x1b\\");
        }
        reply
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TerminalOptions {
    pub scrollback_history: usize,
    pub default_cursor_style: CursorStyle,
}

impl Default for TerminalOptions {
    fn default() -> Self {
        Self {
            scrollback_history: 1000,
            default_cursor_style: CursorStyle::Block,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Event {
    Wakeup,
    Title(String),
    ResetTitle,
    Bell,
    Exit,
    ClipboardStore(String),
    ClipboardLoad(ClipboardRequest),
    ShellPromptStart,
    ShellCommandStart,
    ShellCommandExecuting,
    ShellCommandFinished(Option<i32>),
    Progress(Progress),
    WorkingDirectory(String),
}

#[derive(Clone)]
pub struct WakeupNotifier {
    notify: Arc<dyn Fn() + Send + Sync>,
}

impl WakeupNotifier {
    pub fn new(notify: impl Fn() + Send + Sync + 'static) -> Self {
        Self {
            notify: Arc::new(notify),
        }
    }

    fn notify(&self) {
        (self.notify)();
    }
}

#[derive(Debug)]
pub struct Error {
    inner: io::Error,
}

impl Error {
    #[cfg(not(unix))]
    fn unsupported(message: &'static str) -> Self {
        Self {
            inner: io::Error::new(io::ErrorKind::Unsupported, message),
        }
    }
}

impl fmt::Display for Error {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.inner.fmt(formatter)
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&self.inner)
    }
}

impl From<io::Error> for Error {
    fn from(inner: io::Error) -> Self {
        Self { inner }
    }
}

#[derive(Debug)]
struct Engine {
    parser: Parser,
    grid: Grid,
    size: Size,
}

impl Engine {
    fn new(size: Size, config: &Config) -> Self {
        let mut parser = Parser::default();
        parser.set_size(size);
        parser.set_osc52(config.osc52);
        let mut engine = Self {
            parser,
            grid: Grid::new(
                size.cols,
                size.rows,
                config.scrollback_history,
                config.default_cursor_style,
            ),
            size,
        };
        engine
            .grid
            .set_cell_metrics(size.cell_width, size.cell_height);
        engine
    }

    fn feed(&mut self, bytes: &[u8]) -> (Vec<Event>, Vec<u8>, bool) {
        let parsed = self.parser.advance(&mut self.grid, bytes);
        let events = parsed.events.into_iter().map(Event::from).collect();
        (events, parsed.replies, parsed.synchronized_update_active)
    }
}

impl From<ParsedEvent> for Event {
    fn from(event: ParsedEvent) -> Self {
        match event {
            ParsedEvent::Title(title) => Self::Title(title),
            ParsedEvent::ResetTitle => Self::ResetTitle,
            ParsedEvent::Bell => Self::Bell,
            ParsedEvent::ClipboardStore(text) => Self::ClipboardStore(text),
            ParsedEvent::ClipboardLoad(request) => Self::ClipboardLoad(request),
            ParsedEvent::ShellPromptStart => Self::ShellPromptStart,
            ParsedEvent::ShellCommandStart => Self::ShellCommandStart,
            ParsedEvent::ShellCommandExecuting => Self::ShellCommandExecuting,
            ParsedEvent::ShellCommandFinished(exit_code) => Self::ShellCommandFinished(exit_code),
            ParsedEvent::Progress(progress) => Self::Progress(progress),
            ParsedEvent::WorkingDirectory(path) => Self::WorkingDirectory(path),
        }
    }
}

fn encode_base64(input: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut output = String::with_capacity(input.len().div_ceil(3) * 4);
    for chunk in input.chunks(3) {
        let value = (u32::from(chunk[0]) << 16)
            | (u32::from(*chunk.get(1).unwrap_or(&0)) << 8)
            | u32::from(*chunk.get(2).unwrap_or(&0));
        output.push(char::from(TABLE[((value >> 18) & 63) as usize]));
        output.push(char::from(TABLE[((value >> 12) & 63) as usize]));
        output.push(if chunk.len() > 1 {
            char::from(TABLE[((value >> 6) & 63) as usize])
        } else {
            '='
        });
        output.push(if chunk.len() > 2 {
            char::from(TABLE[(value & 63) as usize])
        } else {
            '='
        });
    }
    output
}

fn decode_base64(input: &[u8]) -> Option<Vec<u8>> {
    let mut output = Vec::with_capacity(input.len().saturating_mul(3) / 4);
    let mut quartet = [0u8; 4];
    let mut quartet_len = 0usize;
    let mut padding = 0usize;

    for &byte in input {
        if matches!(byte, b'\r' | b'\n' | b'\t' | b' ') {
            continue;
        }
        if byte == b'=' {
            padding = padding.saturating_add(1);
            if padding > 2 {
                return None;
            }
            continue;
        }
        if padding != 0 {
            return None;
        }
        let value = match byte {
            b'A'..=b'Z' => byte - b'A',
            b'a'..=b'z' => byte - b'a' + 26,
            b'0'..=b'9' => byte - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            _ => return None,
        };
        quartet[quartet_len] = value;
        quartet_len += 1;
        if quartet_len == 4 {
            output.extend_from_slice(&[
                (quartet[0] << 2) | (quartet[1] >> 4),
                (quartet[1] << 4) | (quartet[2] >> 2),
                (quartet[2] << 6) | quartet[3],
            ]);
            quartet_len = 0;
        }
    }

    match (quartet_len, padding) {
        (0, 0) => {}
        (2, 0 | 2) if quartet[1] & 0x0f == 0 => {
            output.push((quartet[0] << 2) | (quartet[1] >> 4));
        }
        (3, 0 | 1) if quartet[2] & 0x03 == 0 => {
            output.extend_from_slice(&[
                (quartet[0] << 2) | (quartet[1] >> 4),
                (quartet[1] << 4) | (quartet[2] >> 2),
            ]);
        }
        _ => return None,
    }
    Some(output)
}

/// Independent Tmon terminal state and optional PTY session.
pub struct Terminal {
    engine: Arc<Mutex<Engine>>,
    events: Arc<Mutex<VecDeque<Event>>>,
    wakeup_queued: Arc<AtomicBool>,
    wakeup_enabled: Arc<AtomicBool>,
    sync_batch_active: Arc<AtomicBool>,
    sync_generation: Arc<AtomicU64>,
    wakeup_notifier: Option<WakeupNotifier>,
    #[cfg(unix)]
    pty: Option<pty::Pty>,
}

impl Terminal {
    /// Start a terminal backed by an independent PTY process.
    pub fn new(
        size: Size,
        config: Config,
        wakeup_notifier: Option<WakeupNotifier>,
    ) -> Result<Self, Error> {
        let size = size.clamped();
        let engine = Arc::new(Mutex::new(Engine::new(size, &config)));
        let events = Arc::new(Mutex::new(VecDeque::with_capacity(64)));
        let wakeup_queued = Arc::new(AtomicBool::new(false));
        let wakeup_enabled = Arc::new(AtomicBool::new(true));
        let sync_batch_active = Arc::new(AtomicBool::new(false));
        let sync_generation = Arc::new(AtomicU64::new(0));

        #[cfg(unix)]
        let pty = {
            let output_engine = engine.clone();
            let output_events = events.clone();
            let output_wakeup_queued = wakeup_queued.clone();
            let output_enabled = wakeup_enabled.clone();
            let output_notifier = wakeup_notifier.clone();
            let output_sync_active = sync_batch_active.clone();
            let output_sync_generation = sync_generation.clone();

            let exit_events = events.clone();
            let exit_wakeup_queued = wakeup_queued.clone();
            let exit_enabled = wakeup_enabled.clone();
            let exit_notifier = wakeup_notifier.clone();
            let spawn = resolve_spawn_config(config)?;
            Some(pty::Pty::spawn(
                spawn,
                size,
                move |bytes| {
                    process_output(
                        &output_engine,
                        &output_events,
                        &output_wakeup_queued,
                        &output_enabled,
                        output_notifier.as_ref(),
                        &output_sync_active,
                        &output_sync_generation,
                        bytes,
                        true,
                    )
                },
                move || {
                    queue_events(
                        &exit_events,
                        &exit_wakeup_queued,
                        &exit_enabled,
                        exit_notifier.as_ref(),
                        [Event::Exit],
                        false,
                        true,
                    );
                },
            )?)
        };

        #[cfg(not(unix))]
        {
            let _ = config;
            let _ = engine;
            let _ = events;
            let _ = wakeup_queued;
            let _ = wakeup_enabled;
            let _ = sync_batch_active;
            let _ = sync_generation;
            let _ = wakeup_notifier;
            return Err(Error::unsupported(
                "the experimental tmon PTY currently supports Unix hosts only",
            ));
        }

        #[cfg(unix)]
        Ok(Self {
            engine,
            events,
            wakeup_queued,
            wakeup_enabled,
            sync_batch_active,
            sync_generation,
            wakeup_notifier,
            pty,
        })
    }

    /// Construct a parser/grid surface with no child process. Useful for replay and tests.
    pub fn new_display(size: Size, config: Config) -> Self {
        let size = size.clamped();
        Self {
            engine: Arc::new(Mutex::new(Engine::new(size, &config))),
            events: Arc::new(Mutex::new(VecDeque::with_capacity(16))),
            wakeup_queued: Arc::new(AtomicBool::new(false)),
            wakeup_enabled: Arc::new(AtomicBool::new(true)),
            sync_batch_active: Arc::new(AtomicBool::new(false)),
            sync_generation: Arc::new(AtomicU64::new(0)),
            wakeup_notifier: None,
            #[cfg(unix)]
            pty: None,
        }
    }

    pub fn child_pid(&self) -> Option<u32> {
        #[cfg(unix)]
        {
            self.pty.as_ref().map(pty::Pty::child_pid)
        }
        #[cfg(not(unix))]
        {
            None
        }
    }

    pub fn set_wakeup_enabled(&self, enabled: bool) {
        self.wakeup_enabled.store(enabled, Ordering::Release);
        if enabled
            && !self
                .events
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .is_empty()
            && let Some(notifier) = &self.wakeup_notifier
        {
            notifier.notify();
        }
    }

    pub fn write(&self, input: &[u8]) {
        #[cfg(unix)]
        if let Some(pty) = &self.pty {
            let _ = pty.write(input);
        }
    }

    pub fn write_owned(&self, input: Vec<u8>) {
        #[cfg(unix)]
        if let Some(pty) = &self.pty {
            let _ = pty.write_owned(input);
            return;
        }
        let _ = input;
    }

    /// Parse bytes into the grid without writing them to the child PTY.
    pub fn hydrate_output(&self, bytes: &[u8]) {
        if bytes.is_empty() {
            return;
        }
        let mut engine = self
            .engine
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let _ = engine.feed(bytes);
    }

    pub fn feed_output(&self, bytes: &[u8]) {
        let _ = process_output(
            &self.engine,
            &self.events,
            &self.wakeup_queued,
            &self.wakeup_enabled,
            self.wakeup_notifier.as_ref(),
            &self.sync_batch_active,
            &self.sync_generation,
            bytes,
            true,
        );
    }

    pub fn drain_events(&self) -> (Vec<Event>, bool) {
        let mut queue = self
            .events
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let count = queue.len().min(MAX_DRAIN_EVENTS);
        let events: Vec<Event> = queue.drain(..count).collect();
        if events.iter().any(|event| matches!(event, Event::Wakeup)) {
            self.wakeup_queued.store(false, Ordering::Release);
        }
        let has_more = !queue.is_empty();
        (events, has_more)
    }

    pub fn resize(&self, size: Size) {
        let size = size.clamped();
        {
            let mut engine = self
                .engine
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if size == engine.size {
                return;
            }
            let Engine {
                parser,
                grid,
                size: engine_size,
            } = &mut *engine;
            grid.resize(size.cols, size.rows);
            *engine_size = size;
            parser.set_size(size);
            grid.set_cell_metrics(size.cell_width, size.cell_height);
            parser.sync_grid_effects(grid);
        }
        #[cfg(unix)]
        if let Some(pty) = &self.pty {
            let _ = pty.resize(size);
        }
    }

    pub fn nudge_resize(&self) {
        #[cfg(unix)]
        if let Some(pty) = &self.pty {
            let _ = pty.resize(self.size());
        }
    }

    pub fn size(&self) -> Size {
        self.engine
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .size
    }

    pub fn set_options(&self, options: TerminalOptions) {
        let mut engine = self
            .engine
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let Engine { parser, grid, .. } = &mut *engine;
        grid.set_history_limit(options.scrollback_history);
        grid.set_default_cursor_style(options.default_cursor_style);
        parser.sync_grid_effects(grid);
    }

    pub fn set_query_colors(&self, colors: QueryColors) {
        self.engine
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .parser
            .set_query_colors(colors);
    }

    pub fn scroll_display(&self, delta_lines: i32) -> bool {
        self.engine
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .grid
            .scroll_display(delta_lines)
    }

    pub fn scroll_to_bottom(&self) -> bool {
        self.engine
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .grid
            .scroll_to_bottom()
    }

    pub fn clear_scrollback(&self) -> bool {
        let mut engine = self
            .engine
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let Engine { parser, grid, .. } = &mut *engine;
        let changed = grid.clear_history();
        parser.sync_grid_effects(grid);
        changed
    }

    pub fn scroll_state(&self) -> (usize, usize) {
        let engine = self
            .engine
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        (engine.grid.display_offset(), engine.grid.history_size())
    }

    pub fn cursor_state(&self) -> Option<CursorState> {
        self.engine
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .grid
            .cursor_state()
    }

    pub fn cursor_position(&self) -> (usize, usize) {
        self.engine
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .grid
            .render_cursor_position()
    }

    pub fn bracketed_paste_mode(&self) -> bool {
        self.engine
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .grid
            .bracketed_paste_mode()
    }

    pub fn alternate_screen_mode(&self) -> bool {
        self.engine
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .grid
            .alternate_screen_mode()
    }

    pub fn mouse_mode(&self) -> MouseMode {
        self.engine
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .grid
            .mouse_mode()
    }

    pub fn keyboard_mode(&self) -> KeyboardMode {
        self.engine
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .grid
            .keyboard_mode()
    }

    pub fn take_damage_snapshot(&self) -> DamageSnapshot {
        self.engine
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .grid
            .take_damage()
    }

    pub fn snapshot(&self) -> Snapshot {
        self.engine
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .grid
            .snapshot()
    }

    pub fn palette(&self) -> Palette {
        self.engine
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .grid
            .palette()
    }

    pub fn kitty_graphics_placements(&self) -> Vec<GraphicsRenderPlacement> {
        let engine = self
            .engine
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        engine.parser.graphics_placements(&engine.grid)
    }

    pub fn for_each_viewport_cell(
        &self,
        visitor: impl FnMut(usize, i32, usize, &Cell, Option<Combining<'_>>),
    ) -> usize {
        self.engine
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .grid
            .for_each_viewport_cell(visitor)
    }

    pub fn with_viewport_cell<R>(
        &self,
        row: usize,
        col: usize,
        f: impl FnOnce(usize, i32, &Cell) -> R,
    ) -> Option<R> {
        self.engine
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .grid
            .with_viewport_cell(row, col, f)
    }

    pub fn hyperlink_at(&self, row: usize, col: usize) -> Option<Hyperlink> {
        self.engine
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .grid
            .hyperlink_at(row, col)
    }

    pub fn for_each_viewport_range(
        &self,
        ranges: impl IntoIterator<Item = (usize, usize, usize)>,
        visitor: impl FnMut(usize, usize, i32, usize, &Cell, Option<Combining<'_>>),
    ) -> usize {
        self.engine
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .grid
            .for_each_viewport_range(ranges, visitor)
    }

    pub fn line_bounds(&self) -> (i32, i32) {
        self.engine
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .grid
            .line_bounds()
    }

    pub fn with_line_cells<R>(&self, line: i32, f: impl FnOnce(&[Cell]) -> R) -> Option<R> {
        let engine = self
            .engine
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        engine.grid.line(line).map(f)
    }

    pub fn for_each_line_cell(
        &self,
        line: i32,
        visitor: impl FnMut(usize, &Cell, Option<Combining<'_>>),
    ) -> bool {
        self.engine
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .grid
            .for_each_line_cell(line, visitor)
    }
}

#[allow(clippy::too_many_arguments)]
fn process_output(
    engine: &Arc<Mutex<Engine>>,
    events: &Arc<Mutex<VecDeque<Event>>>,
    wakeup_queued: &Arc<AtomicBool>,
    wakeup_enabled: &Arc<AtomicBool>,
    wakeup_notifier: Option<&WakeupNotifier>,
    sync_batch_active: &Arc<AtomicBool>,
    sync_generation: &Arc<AtomicU64>,
    bytes: &[u8],
    queue_wakeup: bool,
) -> Vec<u8> {
    if bytes.is_empty() {
        return Vec::new();
    }
    let (parsed_events, replies, synchronized_update_active) = engine
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .feed(bytes);
    let should_wake = queue_wakeup && !synchronized_update_active;
    if queue_wakeup && synchronized_update_active {
        let was_active = sync_batch_active.swap(true, Ordering::AcqRel);
        if !was_active {
            let generation = sync_generation.fetch_add(1, Ordering::AcqRel) + 1;
            schedule_sync_watchdog(
                events.clone(),
                wakeup_queued.clone(),
                wakeup_enabled.clone(),
                wakeup_notifier.cloned(),
                sync_batch_active.clone(),
                sync_generation.clone(),
                generation,
            );
        }
    } else if queue_wakeup {
        sync_batch_active.store(false, Ordering::Release);
        sync_generation.fetch_add(1, Ordering::AcqRel);
    }
    queue_events(
        events,
        wakeup_queued,
        wakeup_enabled,
        wakeup_notifier,
        parsed_events,
        should_wake,
        should_wake,
    );
    replies
}

fn schedule_sync_watchdog(
    events: Arc<Mutex<VecDeque<Event>>>,
    wakeup_queued: Arc<AtomicBool>,
    wakeup_enabled: Arc<AtomicBool>,
    wakeup_notifier: Option<WakeupNotifier>,
    sync_batch_active: Arc<AtomicBool>,
    sync_generation: Arc<AtomicU64>,
    generation: u64,
) {
    let task = SyncWatchdogTask {
        deadline: Instant::now() + SYNC_WATCHDOG_DELAY,
        events,
        wakeup_queued,
        wakeup_enabled,
        wakeup_notifier,
        sync_batch_active,
        sync_generation,
        generation,
    };
    let unscheduled = match sync_watchdog_sender() {
        Some(sender) => sender.send(task).err().map(|error| error.0),
        None => Some(task),
    };
    if let Some(task) = unscheduled {
        task.sync_batch_active.store(false, Ordering::Release);
        queue_events(
            &task.events,
            &task.wakeup_queued,
            &task.wakeup_enabled,
            task.wakeup_notifier.as_ref(),
            std::iter::empty(),
            true,
            true,
        );
    }
}

struct SyncWatchdogTask {
    deadline: Instant,
    events: Arc<Mutex<VecDeque<Event>>>,
    wakeup_queued: Arc<AtomicBool>,
    wakeup_enabled: Arc<AtomicBool>,
    wakeup_notifier: Option<WakeupNotifier>,
    sync_batch_active: Arc<AtomicBool>,
    sync_generation: Arc<AtomicU64>,
    generation: u64,
}

fn sync_watchdog_sender() -> Option<&'static mpsc::Sender<SyncWatchdogTask>> {
    static WATCHDOG: OnceLock<Option<mpsc::Sender<SyncWatchdogTask>>> = OnceLock::new();
    WATCHDOG
        .get_or_init(|| {
            let (sender, receiver) = mpsc::channel();
            std::thread::Builder::new()
                .name("tmon-sync-watchdog".to_string())
                .spawn(move || sync_watchdog_worker(receiver))
                .ok()
                .map(|_| sender)
        })
        .as_ref()
}

fn sync_watchdog_worker(receiver: mpsc::Receiver<SyncWatchdogTask>) {
    let mut tasks = Vec::<SyncWatchdogTask>::new();
    loop {
        let now = Instant::now();
        let wait = tasks
            .iter()
            .map(|task| task.deadline.saturating_duration_since(now))
            .min()
            .unwrap_or(Duration::from_secs(60 * 60));
        match receiver.recv_timeout(wait) {
            Ok(task) => tasks.push(task),
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => return,
        }

        let now = Instant::now();
        let mut index = 0;
        while index < tasks.len() {
            if tasks[index].deadline > now {
                index += 1;
                continue;
            }
            let task = tasks.swap_remove(index);
            if task.sync_generation.load(Ordering::Acquire) != task.generation
                || !task.sync_batch_active.swap(false, Ordering::AcqRel)
            {
                continue;
            }
            queue_events(
                &task.events,
                &task.wakeup_queued,
                &task.wakeup_enabled,
                task.wakeup_notifier.as_ref(),
                std::iter::empty(),
                true,
                true,
            );
        }
    }
}

fn queue_events(
    events: &Arc<Mutex<VecDeque<Event>>>,
    wakeup_queued: &AtomicBool,
    wakeup_enabled: &Arc<AtomicBool>,
    wakeup_notifier: Option<&WakeupNotifier>,
    incoming: impl IntoIterator<Item = Event>,
    queue_wakeup: bool,
    notify: bool,
) {
    let mut incoming = incoming.into_iter().peekable();
    if incoming.peek().is_none() && queue_wakeup && wakeup_queued.load(Ordering::Acquire) {
        return;
    }

    let changed = {
        let mut queue = events
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut changed = false;
        for event in incoming {
            let is_wakeup = matches!(event, Event::Wakeup);
            if push_bounded_event(&mut queue, event) {
                changed = true;
                if is_wakeup {
                    wakeup_queued.store(true, Ordering::Release);
                }
            }
        }
        if queue_wakeup && !wakeup_queued.swap(true, Ordering::AcqRel) {
            changed |= push_bounded_event(&mut queue, Event::Wakeup);
        }
        changed
    };
    if changed
        && notify
        && wakeup_enabled.load(Ordering::Acquire)
        && let Some(notifier) = wakeup_notifier
    {
        notifier.notify();
    }
}

fn push_bounded_event(queue: &mut VecDeque<Event>, event: Event) -> bool {
    let singleton = matches!(event, Event::Wakeup | Event::Exit);
    if singleton
        && queue.iter().any(|queued| {
            matches!(
                (&event, queued),
                (Event::Wakeup, Event::Wakeup) | (Event::Exit, Event::Exit)
            )
        })
    {
        return false;
    }

    if matches!(
        event,
        Event::Title(_) | Event::ResetTitle | Event::Progress(_) | Event::WorkingDirectory(_)
    ) {
        queue.retain(|queued| !event_supersedes(&event, queued));
    }

    if queue.len() == MAX_QUEUED_EVENTS {
        let removable = queue
            .iter()
            .position(|queued| !event_is_critical(queued))
            .or_else(|| {
                queue
                    .iter()
                    .position(|queued| matches!(queued, Event::ClipboardLoad(_)))
            });
        if let Some(index) = removable {
            queue.remove(index);
        } else {
            // Wakeup and Exit are deduplicated, so a queue containing only those
            // cannot normally reach the cap.
            queue.pop_front();
        }
    }
    queue.push_back(event);
    true
}

fn event_is_critical(event: &Event) -> bool {
    matches!(event, Event::Wakeup | Event::Exit | Event::ClipboardLoad(_))
}

fn event_supersedes(incoming: &Event, queued: &Event) -> bool {
    matches!(
        (incoming, queued),
        (
            Event::Title(_) | Event::ResetTitle,
            Event::Title(_) | Event::ResetTitle
        ) | (Event::Progress(_), Event::Progress(_))
            | (Event::WorkingDirectory(_), Event::WorkingDirectory(_))
    )
}

#[cfg(unix)]
fn resolve_spawn_config(config: Config) -> Result<pty::SpawnConfig, Error> {
    let (program, args) = match config.launch {
        Some(Launch::Program { program, args }) => {
            if program.trim().is_empty() {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "terminal program cannot be empty",
                )
                .into());
            }
            (program, args)
        }
        Some(Launch::ShellCommand(command)) if !command.trim().is_empty() => {
            ("/bin/sh".to_string(), vec!["-c".to_string(), command])
        }
        _ => {
            let program = config
                .shell
                .as_deref()
                .map(str::trim)
                .filter(|shell| !shell.is_empty())
                .map(str::to_string)
                .or_else(|| {
                    std::env::var("SHELL")
                        .ok()
                        .filter(|shell| !shell.trim().is_empty())
                })
                .unwrap_or_else(default_shell);
            let args = login_shell_args(&program);
            (program, args)
        }
    };

    Ok(pty::SpawnConfig {
        program,
        args,
        working_directory: config.working_directory,
        environment: config.environment,
    })
}

#[cfg(unix)]
fn default_shell() -> String {
    #[cfg(target_os = "macos")]
    {
        "/bin/zsh".to_string()
    }
    #[cfg(not(target_os = "macos"))]
    {
        "/bin/bash".to_string()
    }
}

#[cfg(unix)]
fn login_shell_args(program: &str) -> Vec<String> {
    let shell = std::path::Path::new(program)
        .file_name()
        .and_then(|name| name.to_str());
    if !matches!(shell, Some("bash" | "zsh" | "fish")) {
        return Vec::new();
    }
    #[cfg(target_os = "macos")]
    {
        vec!["-i".to_string(), "-l".to_string()]
    }
    #[cfg(not(target_os = "macos"))]
    {
        vec!["-i".to_string()]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_terminal_uses_independent_parser_and_grid() {
        let terminal = Terminal::new_display(
            Size {
                cols: 5,
                rows: 2,
                ..Size::default()
            },
            Config::default(),
        );
        terminal.feed_output(b"ok\x1b[31m!");
        let frame = terminal.snapshot();
        assert_eq!(frame.cells[0].character, 'o');
        assert_eq!(frame.cells[1].character, 'k');
        assert_eq!(frame.cells[2].foreground, Color::Indexed(1));
        assert_eq!(terminal.drain_events().0, vec![Event::Wakeup]);
    }

    #[test]
    fn base64_decoder_accepts_padded_and_unpadded_data_but_rejects_junk() {
        assert_eq!(decode_base64(b"dG1vbg=="), Some(b"tmon".to_vec()));
        assert_eq!(decode_base64(b"dG1vbg"), Some(b"tmon".to_vec()));
        assert_eq!(decode_base64(b"dG1v\n bg=="), Some(b"tmon".to_vec()));
        for invalid in [b"A".as_slice(), b"AA=".as_slice(), b"AA==x", b"===="] {
            assert_eq!(decode_base64(invalid), None);
        }
    }

    #[test]
    fn synchronized_updates_wake_after_a_bounded_timeout() {
        let terminal = Terminal::new_display(Size::default(), Config::default());
        terminal.feed_output(b"\x1b[?2026hframe");
        assert!(terminal.drain_events().0.is_empty());

        std::thread::sleep(SYNC_WATCHDOG_DELAY + Duration::from_millis(75));
        assert_eq!(terminal.drain_events().0, vec![Event::Wakeup]);
    }

    #[test]
    fn synchronized_update_end_invalidates_the_watchdog() {
        let terminal = Terminal::new_display(Size::default(), Config::default());
        terminal.feed_output(b"\x1b[?2026hframe");
        terminal.feed_output(b"\x1b[?2026l");
        assert_eq!(terminal.drain_events().0, vec![Event::Wakeup]);

        std::thread::sleep(SYNC_WATCHDOG_DELAY + Duration::from_millis(75));
        assert!(terminal.drain_events().0.is_empty());
    }

    #[test]
    fn osc52_paste_queries_are_explicitly_gated_and_formatted() {
        let denied = Terminal::new_display(Size::default(), Config::default());
        denied.feed_output(b"\x1b]52;c;?\x07");
        assert_eq!(denied.drain_events().0, vec![Event::Wakeup]);

        let allowed = Terminal::new_display(
            Size::default(),
            Config {
                osc52: Osc52::CopyPaste,
                ..Config::default()
            },
        );
        allowed.feed_output(b"\x1b]52;s;?\x1b\\");
        let (events, _) = allowed.drain_events();
        let request = events
            .into_iter()
            .find_map(|event| match event {
                Event::ClipboardLoad(request) => Some(request),
                _ => None,
            })
            .expect("OSC 52 load request should reach the host");
        assert_eq!(request.target(), ClipboardTarget::Selection);
        assert_eq!(request.format_reply("tmon"), b"\x1b]52;s;dG1vbg==\x1b\\");
    }

    #[test]
    fn live_options_bound_history_without_rebuilding_the_terminal() {
        let terminal = Terminal::new_display(
            Size {
                cols: 2,
                rows: 1,
                ..Size::default()
            },
            Config {
                scrollback_history: 8,
                ..Config::default()
            },
        );
        terminal.feed_output(b"abcdefgh");
        assert!(terminal.scroll_state().1 > 1);
        terminal.set_options(TerminalOptions {
            scrollback_history: 1,
            ..TerminalOptions::default()
        });
        assert_eq!(terminal.scroll_state().1, 1);
    }

    #[test]
    fn sizes_are_clamped_before_allocating_the_grid() {
        let terminal = Terminal::new_display(
            Size {
                cols: 0,
                rows: 0,
                cell_width: f32::NAN,
                cell_height: 0.0,
            },
            Config::default(),
        );
        assert_eq!(terminal.size().cols, 1);
        assert_eq!(terminal.size().rows, 1);
        assert_eq!(terminal.size().cell_width, 1.0);
        assert_eq!(terminal.size().cell_height, 1.0);
    }

    #[test]
    fn bounded_event_queue_retains_lifecycle_events_after_flooding() {
        let events = Arc::new(Mutex::new(VecDeque::from([Event::Wakeup, Event::Exit])));
        let wakeup_queued = AtomicBool::new(true);
        let wakeup_enabled = Arc::new(AtomicBool::new(false));
        let clipboard_request = ClipboardRequest {
            target: ClipboardTarget::Clipboard,
            selector: b'c',
            bell_terminated: false,
        };
        queue_events(
            &events,
            &wakeup_queued,
            &wakeup_enabled,
            None,
            [Event::ClipboardLoad(clipboard_request)]
                .into_iter()
                .chain(std::iter::repeat_n(Event::Bell, MAX_QUEUED_EVENTS + 64))
                .chain([Event::Exit, Event::Wakeup]),
            true,
            false,
        );

        let queue = events
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        assert_eq!(queue.len(), MAX_QUEUED_EVENTS);
        assert_eq!(
            queue
                .iter()
                .filter(|event| matches!(event, Event::Wakeup))
                .count(),
            1
        );
        assert_eq!(
            queue
                .iter()
                .filter(|event| matches!(event, Event::Exit))
                .count(),
            1
        );
        assert_eq!(
            queue
                .iter()
                .filter(|event| matches!(event, Event::ClipboardLoad(_)))
                .count(),
            1
        );
    }

    #[test]
    fn event_queue_coalesces_latest_state_updates() {
        let events = Arc::new(Mutex::new(VecDeque::new()));
        let wakeup_queued = AtomicBool::new(false);
        let wakeup_enabled = Arc::new(AtomicBool::new(false));
        queue_events(
            &events,
            &wakeup_queued,
            &wakeup_enabled,
            None,
            [
                Event::Title("old".to_string()),
                Event::Progress(Progress::InProgress(10)),
                Event::WorkingDirectory("/old".to_string()),
                Event::Title("new".to_string()),
                Event::ResetTitle,
                Event::Progress(Progress::InProgress(90)),
                Event::WorkingDirectory("/new".to_string()),
            ],
            false,
            false,
        );

        let queue = events
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        assert_eq!(
            *queue,
            VecDeque::from([
                Event::ResetTitle,
                Event::Progress(Progress::InProgress(90)),
                Event::WorkingDirectory("/new".to_string()),
            ])
        );
    }

    #[test]
    fn event_queue_notifies_once_until_the_pending_wakeup_is_drained() {
        let events = Arc::new(Mutex::new(VecDeque::new()));
        let wakeup_queued = AtomicBool::new(false);
        let wakeup_enabled = Arc::new(AtomicBool::new(true));
        let notifications = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let notifier_notifications = notifications.clone();
        let notifier = WakeupNotifier::new(move || {
            notifier_notifications.fetch_add(1, Ordering::Relaxed);
        });

        for _ in 0..2 {
            queue_events(
                &events,
                &wakeup_queued,
                &wakeup_enabled,
                Some(&notifier),
                std::iter::empty(),
                true,
                true,
            );
        }
        assert_eq!(notifications.load(Ordering::Relaxed), 1);
        assert_eq!(
            events
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .len(),
            1
        );

        events
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clear();
        wakeup_queued.store(false, Ordering::Release);
        queue_events(
            &events,
            &wakeup_queued,
            &wakeup_enabled,
            Some(&notifier),
            std::iter::empty(),
            true,
            true,
        );
        assert_eq!(notifications.load(Ordering::Relaxed), 2);
    }

    #[test]
    fn kitty_graphics_are_placed_at_the_cursor_when_the_command_arrives() {
        let terminal = Terminal::new_display(
            Size {
                cols: 8,
                rows: 2,
                ..Size::default()
            },
            Config::default(),
        );

        terminal.feed_output(b"A\x1b_Ga=T,f=32,s=1,v=1,i=7,C=1;/wAA/w==\x1b\\B");

        let placements = terminal.kitty_graphics_placements();
        assert_eq!(placements.len(), 1);
        assert_eq!(placements[0].image_id, 7);
        assert_eq!(placements[0].col, 1);
        assert_eq!(placements[0].viewport_row, 0);
        let frame = terminal.snapshot();
        assert_eq!(frame.cells[0].character, 'A');
        assert_eq!(frame.cells[1].character, 'B');
    }

    fn graphics_test_terminal(scrollback_history: usize) -> Terminal {
        Terminal::new_display(
            Size {
                cols: 8,
                rows: 4,
                ..Size::default()
            },
            Config {
                scrollback_history,
                ..Config::default()
            },
        )
    }

    #[test]
    fn kitty_graphics_follow_zero_and_full_scrollback() {
        for history_limit in [0, 1] {
            let terminal = graphics_test_terminal(history_limit);
            if history_limit == 1 {
                terminal.feed_output(b"\x1b[4;1H\n");
                assert_eq!(terminal.scroll_state().1, 1);
            }
            terminal
                .feed_output(b"\x1b[2;1H\x1b_Ga=T,f=32,s=1,v=1,i=10,c=2,r=1,C=1;AQID/w==\x1b\\");
            assert_eq!(terminal.kitty_graphics_placements()[0].viewport_row, 1);

            terminal.feed_output(b"\x1b[4;1H\n");
            assert_eq!(terminal.kitty_graphics_placements()[0].viewport_row, 0);
            terminal.feed_output(b"\n");
            assert!(terminal.kitty_graphics_placements().is_empty());
        }
    }

    #[test]
    fn kitty_graphics_follow_alternate_and_partial_region_scrolls() {
        let terminal = graphics_test_terminal(0);
        terminal.feed_output(
            b"\x1b[?1049h\x1b[2;1H\x1b_Ga=T,f=32,s=1,v=1,i=11,c=2,r=1,C=1;AQID/w==\x1b\\",
        );
        assert_eq!(terminal.kitty_graphics_placements()[0].viewport_row, 1);
        terminal.feed_output(b"\x1b[4;1H\n");
        assert_eq!(terminal.kitty_graphics_placements()[0].viewport_row, 0);
        terminal.feed_output(b"\n");
        assert!(terminal.kitty_graphics_placements().is_empty());

        terminal.feed_output(
            b"\x1b[?1049l\x1b[1;1H\x1b_Ga=T,f=32,s=1,v=1,i=12,c=2,r=1,C=1;AQID/w==\x1b\\",
        );
        terminal.feed_output(b"\x1b[2;3r\x1b[3;1H\n");
        assert_eq!(terminal.kitty_graphics_placements()[0].viewport_row, 0);
    }

    #[test]
    fn kitty_graphics_clear_with_viewport_alt_entry_and_terminal_reset() {
        let terminal = graphics_test_terminal(8);
        terminal.feed_output(b"\x1b[2;1H\x1b_Ga=T,f=32,s=1,v=1,i=13,c=2,r=1,C=1;AQID/w==\x1b\\");
        terminal.feed_output(b"\x1b[?1049h");
        assert!(terminal.kitty_graphics_placements().is_empty());
        terminal.feed_output(b"\x1b[2;1H\x1b_Ga=T,f=32,s=1,v=1,i=14,c=2,r=1,C=1;AQID/w==\x1b\\");
        assert_eq!(terminal.kitty_graphics_placements().len(), 1);
        terminal.feed_output(b"\x1b[2J");
        assert!(terminal.kitty_graphics_placements().is_empty());

        terminal.feed_output(b"\x1b[?1049l");
        assert_eq!(terminal.kitty_graphics_placements()[0].image_id, 13);
        terminal.feed_output(b"\x1bc");
        assert!(terminal.kitty_graphics_placements().is_empty());
    }

    #[test]
    fn kitty_graphics_cursor_advance_tracks_bottom_scrolls() {
        for history_limit in [0, 2, 8] {
            let terminal = graphics_test_terminal(history_limit);
            if history_limit == 2 {
                terminal.feed_output(b"\x1b[4;1H\n\n");
            }
            terminal.feed_output(b"\x1b[4;1H\x1b_Ga=T,f=32,s=1,v=1,i=15,c=2,r=3;AQID/w==\x1b\\");
            let placements = terminal.kitty_graphics_placements();
            assert_eq!(placements.len(), 1);
            assert_eq!(placements[0].viewport_row, 0);
            assert_eq!(terminal.cursor_position(), (2, 3));
        }
    }

    #[test]
    fn kitty_graphics_follow_history_during_row_only_resizes() {
        let terminal = graphics_test_terminal(8);
        terminal.feed_output(b"\x1b[2;1H\x1b_Ga=T,f=32,s=1,v=1,i=16,c=2,r=1,C=1;AQID/w==\x1b\\");
        terminal.feed_output(b"\x1b[4;1H\n\n");
        assert_eq!(terminal.scroll_state().1, 2);
        assert!(terminal.kitty_graphics_placements().is_empty());

        terminal.resize(Size {
            rows: 6,
            ..terminal.size()
        });
        assert_eq!(terminal.scroll_state().1, 0);
        assert_eq!(terminal.kitty_graphics_placements()[0].viewport_row, 1);

        terminal.resize(Size {
            rows: 2,
            ..terminal.size()
        });
        assert_eq!(terminal.scroll_state().1, 4);
        assert!(terminal.kitty_graphics_placements().is_empty());
    }

    #[test]
    fn width_reflow_clears_placements_but_keeps_reusable_image_data() {
        let terminal = graphics_test_terminal(8);
        terminal.feed_output(b"\x1b_Ga=T,f=32,s=1,v=1,i=17,c=2,r=1,C=1,q=1;AQID/w==\x1b\\");
        assert_eq!(terminal.kitty_graphics_placements().len(), 1);

        terminal.resize(Size {
            cols: 5,
            ..terminal.size()
        });
        assert!(terminal.kitty_graphics_placements().is_empty());

        terminal.feed_output(b"\x1b_Ga=p,i=17,c=1,r=1,C=1,q=1\x1b\\");
        assert_eq!(terminal.kitty_graphics_placements().len(), 1);
        assert_eq!(terminal.kitty_graphics_placements()[0].image_id, 17);
    }

    #[cfg(unix)]
    #[test]
    fn unix_pty_streams_child_output_into_the_tmon_grid() {
        use std::{sync::mpsc, time::Duration};

        let (notify_tx, notify_rx) = mpsc::channel();
        let terminal = Terminal::new(
            Size {
                cols: 16,
                rows: 2,
                ..Size::default()
            },
            Config {
                launch: Some(Launch::ShellCommand("printf tmon-pty".to_string())),
                ..Config::default()
            },
            Some(WakeupNotifier::new(move || {
                let _ = notify_tx.send(());
            })),
        )
        .expect("tmon should start a Unix PTY");

        let mut exited = false;
        for _ in 0..20 {
            let _ = notify_rx.recv_timeout(Duration::from_millis(100));
            let (events, _) = terminal.drain_events();
            exited |= events.contains(&Event::Exit);
            let text = terminal
                .snapshot()
                .cells
                .iter()
                .map(|cell| cell.character)
                .collect::<String>();
            if exited && text.contains("tmon-pty") {
                return;
            }
        }
        panic!("PTY output or exit event did not arrive in time");
    }

    #[cfg(unix)]
    #[test]
    fn unix_pty_processes_owned_input_through_the_writer_thread() {
        use std::{sync::mpsc, time::Duration};

        let (notify_tx, notify_rx) = mpsc::channel();
        let terminal = Terminal::new(
            Size {
                cols: 32,
                rows: 3,
                ..Size::default()
            },
            Config {
                launch: Some(Launch::ShellCommand(
                    "IFS= read -r line; printf 'reply:%s' \"$line\"".to_string(),
                )),
                ..Config::default()
            },
            Some(WakeupNotifier::new(move || {
                let _ = notify_tx.send(());
            })),
        )
        .expect("tmon should start an interactive Unix PTY");

        terminal.write_owned(b"hello\n".to_vec());
        for _ in 0..20 {
            let _ = notify_rx.recv_timeout(Duration::from_millis(100));
            let (events, _) = terminal.drain_events();
            let text = terminal
                .snapshot()
                .cells
                .iter()
                .map(|cell| cell.character)
                .collect::<String>();
            if events.contains(&Event::Exit) {
                assert!(
                    text.contains("reply:hello"),
                    "PTY reply was missing: {text:?}"
                );
                return;
            }
        }
        panic!("PTY input reply or exit event did not arrive in time");
    }

    #[cfg(unix)]
    #[test]
    fn unix_pty_honors_the_configured_working_directory() {
        use std::{sync::mpsc, time::Duration};

        let working_directory = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .canonicalize()
            .expect("manifest directory should exist");
        let (notify_tx, notify_rx) = mpsc::channel();
        let terminal = Terminal::new(
            Size {
                cols: 256,
                rows: 2,
                ..Size::default()
            },
            Config {
                working_directory: Some(working_directory.clone()),
                launch: Some(Launch::ShellCommand("pwd".to_string())),
                ..Config::default()
            },
            Some(WakeupNotifier::new(move || {
                let _ = notify_tx.send(());
            })),
        )
        .expect("tmon should start a Unix PTY");

        for _ in 0..20 {
            let _ = notify_rx.recv_timeout(Duration::from_millis(100));
            let (events, _) = terminal.drain_events();
            let text = terminal
                .snapshot()
                .cells
                .iter()
                .map(|cell| cell.character)
                .collect::<String>();
            if events.contains(&Event::Exit) {
                assert!(
                    text.contains(&working_directory.to_string_lossy().to_string()),
                    "PTY reported a different working directory: {text:?}"
                );
                return;
            }
        }
        panic!("PTY working-directory output or exit event did not arrive in time");
    }

    #[cfg(unix)]
    #[test]
    fn unix_pty_does_not_run_the_program_when_chdir_fails() {
        use std::{sync::mpsc, time::Duration};

        let missing_directory = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join(format!(".missing-tmon-cwd-{}", std::process::id()));
        assert!(!missing_directory.exists());
        let (notify_tx, notify_rx) = mpsc::channel();
        let terminal = Terminal::new(
            Size::default(),
            Config {
                working_directory: Some(missing_directory),
                launch: Some(Launch::ShellCommand("printf should-not-run".to_string())),
                ..Config::default()
            },
            Some(WakeupNotifier::new(move || {
                let _ = notify_tx.send(());
            })),
        )
        .expect("forking the PTY should succeed before the child chdir");

        for _ in 0..20 {
            let _ = notify_rx.recv_timeout(Duration::from_millis(100));
            let (events, _) = terminal.drain_events();
            if events.contains(&Event::Exit) {
                let text = terminal
                    .snapshot()
                    .cells
                    .iter()
                    .map(|cell| cell.character)
                    .collect::<String>();
                assert!(!text.contains("should-not-run"));
                return;
            }
        }
        panic!("PTY exit event did not arrive after a failed child chdir");
    }
}
