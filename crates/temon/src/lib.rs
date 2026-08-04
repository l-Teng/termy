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

#[cfg(unix)]
use std::sync::Weak;
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
    Hyperlink, KeyboardMode, LinkMatch, MouseMode, Palette, Rgb, Snapshot, ViewportLink,
};
pub use parser::{Progress, QueryColors};

use grid::{Grid, LinkCandidate};
use parser::{ParsedEvent, Parser};

const MAX_DRAIN_EVENTS: usize = 2048;
const MAX_QUEUED_EVENTS: usize = 8192;
const MAX_BUFFERED_PROTOCOL_REPLY_BYTES: usize = 2 * 1024 * 1024;
const MAX_GRID_DIMENSION: u16 = 4096;
const SYNC_WATCHDOG_DELAY: Duration = Duration::from_millis(175);

type ProtocolReplySink = Arc<dyn Fn(Vec<u8>) + Send + Sync>;

#[cfg(unix)]
struct PtyProtocolReplyTarget {
    pty: Option<Weak<pty::Pty>>,
    pending: VecDeque<u8>,
}

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
    pub query_colors: QueryColors,
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
            query_colors: QueryColors::default(),
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

struct EngineOutput {
    events: Vec<Event>,
    replies: Vec<u8>,
    synchronized_update_active: bool,
    synchronized_update_deadline: Option<Instant>,
    synchronized_update_refreshed: bool,
    synchronized_update_committed: bool,
    unsynchronized_activity: bool,
}

impl Engine {
    fn new(size: Size, config: &Config) -> Self {
        let mut parser = Parser::default();
        parser.set_size(size);
        parser.set_query_colors(config.query_colors);
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

    fn feed_detailed(&mut self, bytes: &[u8]) -> EngineOutput {
        let parsed = self.parser.advance(&mut self.grid, bytes);
        let events = parsed.events.into_iter().map(Event::from).collect();
        EngineOutput {
            events,
            replies: parsed.replies,
            synchronized_update_active: parsed.synchronized_update_active,
            synchronized_update_deadline: parsed.synchronized_update_deadline,
            synchronized_update_refreshed: parsed.synchronized_update_refreshed,
            synchronized_update_committed: parsed.synchronized_update_committed,
            unsynchronized_activity: parsed.unsynchronized_activity,
        }
    }

    fn feed(&mut self, bytes: &[u8]) -> (Vec<Event>, Vec<u8>, bool) {
        let output = self.feed_detailed(bytes);
        (
            output.events,
            output.replies,
            output.synchronized_update_active,
        )
    }

    fn stop_synchronized_update(&mut self) -> EngineOutput {
        let parsed = self.parser.stop_synchronized_update(&mut self.grid);
        EngineOutput {
            events: parsed.events.into_iter().map(Event::from).collect(),
            replies: parsed.replies,
            synchronized_update_active: parsed.synchronized_update_active,
            synchronized_update_deadline: parsed.synchronized_update_deadline,
            synchronized_update_refreshed: parsed.synchronized_update_refreshed,
            synchronized_update_committed: parsed.synchronized_update_committed,
            unsynchronized_activity: parsed.unsynchronized_activity,
        }
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
    protocol_replies: Arc<Mutex<VecDeque<u8>>>,
    protocol_reply_sink: ProtocolReplySink,
    wakeup_queued: Arc<AtomicBool>,
    wakeup_enabled: Arc<AtomicBool>,
    sync_batch_active: Arc<AtomicBool>,
    sync_generation: Arc<AtomicU64>,
    wakeup_notifier: Option<WakeupNotifier>,
    #[cfg(unix)]
    pty: Option<Arc<pty::Pty>>,
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
        let events: Arc<Mutex<VecDeque<Event>>> = Arc::new(Mutex::new(VecDeque::with_capacity(64)));
        let protocol_replies: Arc<Mutex<VecDeque<u8>>> = Arc::new(Mutex::new(VecDeque::new()));
        let wakeup_queued = Arc::new(AtomicBool::new(false));
        let wakeup_enabled = Arc::new(AtomicBool::new(true));
        let sync_batch_active = Arc::new(AtomicBool::new(false));
        let sync_generation = Arc::new(AtomicU64::new(0));

        #[cfg(unix)]
        let (pty, protocol_reply_sink) = {
            let reply_target = Arc::new(Mutex::new(PtyProtocolReplyTarget {
                pty: None,
                pending: VecDeque::new(),
            }));
            let sink_target = reply_target.clone();
            let protocol_reply_sink: ProtocolReplySink = Arc::new(move |reply| {
                let mut target = sink_target
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                let pty = target.pty.as_ref().and_then(Weak::upgrade);
                if let Some(pty) = pty {
                    drop(target);
                    let _ = pty.write_owned(reply);
                } else if target.pty.is_none() {
                    buffer_protocol_reply_queue(&mut target.pending, reply);
                }
            });
            let output_engine = engine.clone();
            let output_events = events.clone();
            let output_wakeup_queued = wakeup_queued.clone();
            let output_enabled = wakeup_enabled.clone();
            let output_notifier = wakeup_notifier.clone();
            let output_sync_active = sync_batch_active.clone();
            let output_sync_generation = sync_generation.clone();
            let output_protocol_reply_sink = protocol_reply_sink.clone();

            let exit_events = events.clone();
            let exit_wakeup_queued = wakeup_queued.clone();
            let exit_enabled = wakeup_enabled.clone();
            let exit_notifier = wakeup_notifier.clone();
            let spawn = resolve_spawn_config(config)?;
            let pty = Arc::new(pty::Pty::spawn(
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
                        &output_protocol_reply_sink,
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
            )?);
            let pending = {
                let mut target = reply_target
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                target.pty = Some(Arc::downgrade(&pty));
                target.pending.drain(..).collect::<Vec<_>>()
            };
            if !pending.is_empty() {
                let _ = pty.write_owned(pending);
            }
            (Some(pty), protocol_reply_sink)
        };

        #[cfg(not(unix))]
        {
            let _ = config;
            let _ = engine;
            let _ = events;
            let _ = protocol_replies;
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
            protocol_replies,
            protocol_reply_sink,
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
        let protocol_replies = Arc::new(Mutex::new(VecDeque::new()));
        let sink_replies = protocol_replies.clone();
        let protocol_reply_sink: ProtocolReplySink = Arc::new(move |reply| {
            buffer_protocol_reply(&sink_replies, reply);
        });
        Self {
            engine: Arc::new(Mutex::new(Engine::new(size, &config))),
            events: Arc::new(Mutex::new(VecDeque::with_capacity(16))),
            protocol_replies,
            protocol_reply_sink,
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
            self.pty.as_ref().map(|pty| pty.child_pid())
        }
        #[cfg(not(unix))]
        {
            None
        }
    }

    pub fn set_wakeup_enabled(&self, enabled: bool) {
        let was_enabled = self.wakeup_enabled.swap(enabled, Ordering::AcqRel);
        if enabled
            && !was_enabled
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
        #[cfg(not(unix))]
        let _ = input;
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
        engine.parser.reset_transient_state_after_hydration();
        self.sync_batch_active.store(false, Ordering::Release);
        self.sync_generation.fetch_add(1, Ordering::AcqRel);
    }

    pub fn feed_output(&self, bytes: &[u8]) {
        let replies = process_output(
            &self.engine,
            &self.events,
            &self.wakeup_queued,
            &self.wakeup_enabled,
            self.wakeup_notifier.as_ref(),
            &self.sync_batch_active,
            &self.sync_generation,
            &self.protocol_reply_sink,
            bytes,
            true,
        );
        if !replies.is_empty() {
            (self.protocol_reply_sink)(replies);
        }
    }

    /// Drain protocol replies produced by a display-only terminal.
    ///
    /// PTY-backed terminals write these directly to their child. Display-only
    /// hosts can forward the returned bytes to their own transport.
    pub fn drain_protocol_replies(&self) -> Vec<u8> {
        self.protocol_replies
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .drain(..)
            .collect()
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

    /// Resolve the OSC 8 or classified text link under a viewport cell.
    ///
    /// Grid traversal happens under one engine lock. Text classification runs
    /// after releasing that lock, so host classifiers may safely perform file
    /// lookups or call back into this terminal.
    pub fn link_at(
        &self,
        row: usize,
        col: usize,
        classifier: impl FnOnce(&[char], usize) -> Option<LinkMatch>,
    ) -> Option<ViewportLink> {
        let candidate = {
            self.engine
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .grid
                .link_candidate(row, col)?
        };
        match candidate {
            LinkCandidate::Resolved(link) => Some(link),
            LinkCandidate::Text(candidate) => {
                let (characters, hovered_index) = candidate.classifier_input();
                let link_match = classifier(characters, hovered_index)?;
                candidate.resolve(link_match)
            }
        }
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
    protocol_reply_sink: &ProtocolReplySink,
    bytes: &[u8],
    queue_wakeup: bool,
) -> Vec<u8> {
    if bytes.is_empty() {
        return Vec::new();
    }
    let (output, watchdog) = {
        let mut engine = engine
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let output = engine.feed_detailed(bytes);
        let watchdog = if queue_wakeup && output.synchronized_update_active {
            let was_active = sync_batch_active.swap(true, Ordering::AcqRel);
            if !was_active || output.synchronized_update_refreshed {
                let generation = sync_generation.fetch_add(1, Ordering::AcqRel) + 1;
                Some((
                    generation,
                    output
                        .synchronized_update_deadline
                        .unwrap_or_else(|| Instant::now() + SYNC_WATCHDOG_DELAY),
                ))
            } else {
                None
            }
        } else if queue_wakeup {
            sync_batch_active.store(false, Ordering::Release);
            sync_generation.fetch_add(1, Ordering::AcqRel);
            None
        } else {
            None
        };
        (output, watchdog)
    };

    if let Some((generation, deadline)) = watchdog {
        schedule_sync_watchdog(SyncWatchdogTask {
            deadline,
            engine: engine.clone(),
            events: events.clone(),
            wakeup_queued: wakeup_queued.clone(),
            wakeup_enabled: wakeup_enabled.clone(),
            wakeup_notifier: wakeup_notifier.cloned(),
            sync_batch_active: sync_batch_active.clone(),
            sync_generation: sync_generation.clone(),
            protocol_reply_sink: protocol_reply_sink.clone(),
            generation,
        });
    }
    let should_wake = queue_wakeup
        && (!output.synchronized_update_active
            || output.synchronized_update_committed
            || output.unsynchronized_activity);
    queue_events(
        events,
        wakeup_queued,
        wakeup_enabled,
        wakeup_notifier,
        output.events,
        should_wake,
        should_wake,
    );
    output.replies
}

fn schedule_sync_watchdog(task: SyncWatchdogTask) {
    let unscheduled = match sync_watchdog_sender() {
        Some(sender) => sender.send(task).err().map(|error| error.0),
        None => Some(task),
    };
    if let Some(task) = unscheduled {
        execute_sync_watchdog_task(task);
    }
}

struct SyncWatchdogTask {
    deadline: Instant,
    engine: Arc<Mutex<Engine>>,
    events: Arc<Mutex<VecDeque<Event>>>,
    wakeup_queued: Arc<AtomicBool>,
    wakeup_enabled: Arc<AtomicBool>,
    wakeup_notifier: Option<WakeupNotifier>,
    sync_batch_active: Arc<AtomicBool>,
    sync_generation: Arc<AtomicU64>,
    protocol_reply_sink: ProtocolReplySink,
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
            Ok(task) => {
                if let Some(index) = tasks
                    .iter()
                    .position(|queued| Arc::ptr_eq(&queued.engine, &task.engine))
                {
                    tasks[index] = task;
                } else {
                    tasks.push(task);
                }
            }
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
            execute_sync_watchdog_task(task);
        }
    }
}

fn execute_sync_watchdog_task(task: SyncWatchdogTask) {
    let output = {
        // Atomics are checked while holding the same engine mutex used by the
        // PTY parser. `process_output` publishes generation changes before it
        // releases this lock, so an expired task cannot commit a newer batch.
        let mut engine = task
            .engine
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if task.sync_generation.load(Ordering::Acquire) != task.generation
            || !task.sync_batch_active.load(Ordering::Acquire)
        {
            return;
        }
        let output = engine.stop_synchronized_update();
        task.sync_batch_active.store(false, Ordering::Release);
        task.sync_generation.fetch_add(1, Ordering::AcqRel);
        output
    };

    if !output.replies.is_empty() {
        (task.protocol_reply_sink)(output.replies);
    }
    queue_events(
        &task.events,
        &task.wakeup_queued,
        &task.wakeup_enabled,
        task.wakeup_notifier.as_ref(),
        output.events,
        true,
        true,
    );
}

fn buffer_protocol_reply(replies: &Arc<Mutex<VecDeque<u8>>>, reply: Vec<u8>) {
    let mut replies = replies
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    buffer_protocol_reply_queue(&mut replies, reply);
}

fn buffer_protocol_reply_queue(replies: &mut VecDeque<u8>, reply: Vec<u8>) {
    let overflow = replies
        .len()
        .saturating_add(reply.len())
        .saturating_sub(MAX_BUFFERED_PROTOCOL_REPLY_BYTES);
    let drain = overflow.min(replies.len());
    replies.drain(..drain);
    if reply.len() >= MAX_BUFFERED_PROTOCOL_REPLY_BYTES {
        replies.clear();
        replies.extend(
            reply[reply.len() - MAX_BUFFERED_PROTOCOL_REPLY_BYTES..]
                .iter()
                .copied(),
        );
    } else {
        replies.extend(reply);
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

    let (changed, queued_non_wakeup) = {
        let mut queue = events
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut changed = false;
        let mut queued_non_wakeup = false;
        for event in incoming {
            let is_wakeup = matches!(event, Event::Wakeup);
            if push_bounded_event(&mut queue, event) {
                changed = true;
                if is_wakeup {
                    wakeup_queued.store(true, Ordering::Release);
                } else {
                    queued_non_wakeup = true;
                }
            }
        }
        if queue_wakeup && !wakeup_queued.swap(true, Ordering::AcqRel) {
            changed |= push_bounded_event(&mut queue, Event::Wakeup);
        }
        (changed, queued_non_wakeup)
    };
    if changed
        && notify
        // Hiding a terminal suppresses repaint wakeups, not lifecycle or
        // protocol events the host must drain to make forward progress.
        && (queued_non_wakeup || wakeup_enabled.load(Ordering::Acquire))
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

    if event_is_coalescable_state(&event) {
        // State updates can replace older values inside one trailing state-only
        // segment, but never across a lifecycle/protocol event. The desktop
        // consumes command titles before ShellCommandFinished, so crossing that
        // boundary would lose the command identity for plugins.
        let segment_start = queue
            .iter()
            .rposition(|queued| !event_is_coalescable_state(queued))
            .map_or(0, |index| index + 1);
        let mut index = segment_start;
        while index < queue.len() {
            if event_supersedes(&event, &queue[index]) {
                queue.remove(index);
            } else {
                index += 1;
            }
        }
    }

    if queue.len() == MAX_QUEUED_EVENTS {
        let removable = queue.iter().position(|queued| !event_is_critical(queued));
        if let Some(index) = removable {
            queue.remove(index);
        } else if !event_is_critical(&event) {
            return false;
        } else if let Some(index) = queue
            .iter()
            .position(|queued| matches!(queued, Event::ClipboardStore(_)))
            .or_else(|| {
                queue
                    .iter()
                    .position(|queued| matches!(queued, Event::ClipboardLoad(_)))
            })
        {
            queue.remove(index);
        } else {
            // Wakeup and Exit are deduplicated, so a full queue containing only
            // those cannot occur. Keep them if invariants change in the future.
            return false;
        }
    }
    queue.push_back(event);
    true
}

fn event_is_critical(event: &Event) -> bool {
    matches!(
        event,
        Event::Wakeup | Event::Exit | Event::ClipboardLoad(_) | Event::ClipboardStore(_)
    )
}

fn event_is_coalescable_state(event: &Event) -> bool {
    matches!(
        event,
        Event::Title(_) | Event::ResetTitle | Event::Progress(_) | Event::WorkingDirectory(_)
    )
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
    use alacritty_terminal::{
        event::{Event as AlacrittyEvent, EventListener, WindowSize},
        term::{Config as AlacrittyConfig, Term as AlacrittyTerm},
        vte::ansi,
    };

    #[derive(Clone, Default)]
    struct ReplyListener(Arc<Mutex<Vec<AlacrittyEvent>>>);

    impl EventListener for ReplyListener {
        fn send_event(&self, event: AlacrittyEvent) {
            self.0
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push(event);
        }
    }

    fn alacritty_query_replies(bytes: &[u8], size: Size) -> Vec<u8> {
        let dimensions = termy_core::TerminalSize {
            cols: size.cols,
            rows: size.rows,
            cell_width: size.cell_width,
            cell_height: size.cell_height,
        };
        let listener = ReplyListener::default();
        let mut term = AlacrittyTerm::new(
            AlacrittyConfig {
                kitty_keyboard: true,
                ..AlacrittyConfig::default()
            },
            &dimensions,
            listener.clone(),
        );
        let mut parser: ansi::Processor = ansi::Processor::new();
        parser.advance(&mut term, bytes);
        let events = std::mem::take(
            &mut *listener
                .0
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
        );
        let window_size = WindowSize::from(dimensions);
        let mut replies = Vec::new();
        for event in events {
            match event {
                AlacrittyEvent::PtyWrite(text) => replies.extend_from_slice(text.as_bytes()),
                AlacrittyEvent::TextAreaSizeRequest(formatter) => {
                    replies.extend_from_slice(formatter(window_size).as_bytes());
                }
                AlacrittyEvent::ColorRequest(index, formatter) => {
                    if let Some(color) = term.colors()[index] {
                        replies.extend_from_slice(formatter(color).as_bytes());
                    }
                }
                _ => {}
            }
        }
        replies
    }

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
    fn link_at_expands_same_row_osc8_without_calling_the_classifier() {
        let terminal = Terminal::new_display(
            Size {
                cols: 12,
                rows: 2,
                ..Size::default()
            },
            Config::default(),
        );
        terminal.feed_output(b"x\x1b]8;id=docs;https://example.com/docs\x1b\\docs\x1b]8;;\x1b\\ y");

        let link = terminal
            .link_at(0, 2, |_, _| panic!("OSC 8 must bypass classification"))
            .expect("OSC 8 link should resolve");
        assert_eq!(
            link,
            ViewportLink {
                start_row: 0,
                start_col: 1,
                end_row: 0,
                end_col: 4,
                target: "https://example.com/docs".to_string(),
            }
        );
    }

    #[test]
    fn link_at_expands_soft_wrapped_osc8() {
        let terminal = Terminal::new_display(
            Size {
                cols: 5,
                rows: 3,
                ..Size::default()
            },
            Config::default(),
        );
        terminal.feed_output(b"\x1b]8;;https://example.com\x1b\\read-more\x1b]8;;\x1b\\");

        let link = terminal
            .link_at(1, 2, |_, _| panic!("OSC 8 must bypass classification"))
            .expect("wrapped OSC 8 link should resolve");
        assert_eq!((link.start_row, link.start_col), (0, 0));
        assert_eq!((link.end_row, link.end_col), (1, 3));
        assert_eq!(link.target, "https://example.com");
    }

    #[test]
    fn link_at_clips_wrapped_osc8_to_a_scrolled_viewport() {
        let terminal = Terminal::new_display(
            Size {
                cols: 5,
                rows: 2,
                ..Size::default()
            },
            Config::default(),
        );
        terminal.feed_output(b"\x1b]8;;https://example.com\x1b\\abcdefghijklmnop\x1b]8;;\x1b\\");
        assert!(terminal.scroll_display(1));

        let link = terminal
            .link_at(0, 2, |_, _| panic!("OSC 8 must bypass classification"))
            .expect("partially visible OSC 8 link should resolve");
        assert_eq!((link.start_row, link.start_col), (0, 0));
        assert_eq!((link.end_row, link.end_col), (1, 4));
        assert_eq!(link.target, "https://example.com");
    }

    #[test]
    fn link_at_runs_the_heuristic_classifier_after_releasing_the_engine_lock() {
        let terminal = Terminal::new_display(
            Size {
                cols: 20,
                rows: 2,
                ..Size::default()
            },
            Config::default(),
        );
        terminal.feed_output(b"go example.com! rest");

        let link = terminal
            .link_at(0, 5, |characters, hovered_index| {
                // Calling back into the terminal would deadlock if classification
                // still ran while the engine mutex was held.
                assert_eq!(terminal.size().cols, 20);
                assert_eq!(characters.iter().collect::<String>(), "example.com!");
                assert_eq!(hovered_index, 2);
                Some(LinkMatch {
                    start_col: 0,
                    end_col: 10,
                    target: "http://example.com".to_string(),
                })
            })
            .expect("classified text link should resolve");
        assert_eq!((link.start_row, link.start_col), (0, 3));
        assert_eq!((link.end_row, link.end_col), (0, 13));
        assert_eq!(link.target, "http://example.com");

        assert_eq!(
            terminal.link_at(0, 8, |_, _| Some(LinkMatch {
                start_col: 0,
                end_col: 1,
                target: "invalid".to_string(),
            })),
            None,
            "classifier ranges must contain the hovered cell"
        );
    }

    #[test]
    fn configured_query_colors_are_available_before_the_first_theme_refresh() {
        let colors = QueryColors {
            ansi: [Rgb { r: 1, g: 2, b: 3 }; 16],
            foreground: Rgb { r: 4, g: 5, b: 6 },
            background: Rgb { r: 7, g: 8, b: 9 },
            cursor: Some(Rgb {
                r: 10,
                g: 11,
                b: 12,
            }),
        };
        let config = Config {
            query_colors: colors,
            ..Config::default()
        };
        let mut engine = Engine::new(Size::default(), &config);
        let (_, replies, _) = engine.feed(b"\x1b]4;1;?\x07\x1b]10;?\x07\x1b]11;?\x07\x1b]12;?\x07");

        assert_eq!(
            replies,
            b"\x1b]4;1;rgb:0101/0202/0303\x07\
              \x1b]10;rgb:0404/0505/0606\x07\
              \x1b]11;rgb:0707/0808/0909\x07"
        );
    }

    #[test]
    fn query_replies_match_the_alacritty_engine() {
        let size = Size {
            cols: 12,
            rows: 3,
            cell_width: 9.0,
            cell_height: 18.0,
        };
        let input = b"ab\
                      \x1b[5n\
                      \x1b[6n\
                      \x1b[c\
                      \x1b[>c\
                      \x1bZ\
                      \x1b[?7$p\
                      \x1b[20$p\
                      \x1b[14t\
                      \x1b[18t\
                      \x1b[>1u\x1b[=2;2u\x1b[?u";
        let mut engine = Engine::new(size, &Config::default());
        let (_, tmon_replies, _) = engine.feed(input);
        let alacritty_replies = alacritty_query_replies(input, size);
        let expected = b"\x1b[0n\
                         \x1b[1;3R\
                         \x1b[?6c\
                         \x1b[>0;2600;1c\
                         \x1b[?6c\
                         \x1b[?7;1$y\
                         \x1b[20;2$y\
                         \x1b[4;54;108t\
                         \x1b[8;3;12t\
                         \x1b[?1u";

        assert_eq!(tmon_replies, alacritty_replies);
        assert_eq!(tmon_replies, expected);
    }

    #[test]
    fn live_color_query_replies_match_the_alacritty_engine() {
        let size = Size::default();
        let input = b"\x1b]4;1;#123456\x1b\\\
                      \x1b]10;#010203\x07\
                      \x1b]11;#040506\x1b\\\
                      \x1b]12;#070809\x07\
                      \x1b]4;1;?\x1b\\\
                      \x1b]10;?\x07\
                      \x1b]11;?\x1b\\\
                      \x1b]12;?\x07";
        let mut engine = Engine::new(size, &Config::default());
        let (_, tmon_replies, _) = engine.feed(input);
        let alacritty_replies = alacritty_query_replies(input, size);
        let expected = b"\x1b]4;1;rgb:1212/3434/5656\x1b\\\
                         \x1b]10;rgb:0101/0202/0303\x07\
                         \x1b]11;rgb:0404/0505/0606\x1b\\\
                         \x1b]12;rgb:0707/0808/0909\x07";

        assert_eq!(tmon_replies, alacritty_replies);
        assert_eq!(tmon_replies, expected);
    }

    #[test]
    fn synchronized_query_replies_match_the_alacritty_engine() {
        let size = Size {
            cols: 12,
            rows: 3,
            ..Size::default()
        };
        let input = b"\x1b[?2004;2026hX\x1b[6n\x1b[?2026l";
        let mut engine = Engine::new(size, &Config::default());
        let (_, tmon_replies, active) = engine.feed(input);

        assert!(!active);
        assert_eq!(tmon_replies, alacritty_query_replies(input, size));
        assert_eq!(tmon_replies, b"\x1b[1;2R");
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
        assert_eq!(terminal.snapshot().cells[0].character, ' ');

        std::thread::sleep(SYNC_WATCHDOG_DELAY + Duration::from_millis(75));
        assert_eq!(terminal.drain_events().0, vec![Event::Wakeup]);
        assert_eq!(
            terminal.snapshot().cells[..5]
                .iter()
                .map(|cell| cell.character)
                .collect::<String>(),
            "frame"
        );
    }

    #[test]
    fn output_before_bsu_wakes_without_exposing_the_staged_suffix() {
        let terminal = Terminal::new_display(
            Size {
                cols: 16,
                rows: 2,
                ..Size::default()
            },
            Config::default(),
        );
        terminal.feed_output(b"prefix\x1b[?2026hframe");

        assert_eq!(terminal.drain_events().0, vec![Event::Wakeup]);
        assert_eq!(
            terminal.snapshot().cells[..11]
                .iter()
                .map(|cell| cell.character)
                .collect::<String>(),
            "prefix     "
        );
        terminal.feed_output(b"\x1b[?2026l");
        assert_eq!(
            terminal.snapshot().cells[..11]
                .iter()
                .map(|cell| cell.character)
                .collect::<String>(),
            "prefixframe"
        );
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
    fn synchronized_query_replies_are_released_on_esu_and_timeout() {
        let ended = Terminal::new_display(Size::default(), Config::default());
        ended.feed_output(b"\x1b[?2026hX\x1b[6n");
        assert!(ended.drain_protocol_replies().is_empty());
        ended.feed_output(b"\x1b[?2026l");
        assert_eq!(ended.drain_protocol_replies(), b"\x1b[1;2R");

        let timed_out = Terminal::new_display(Size::default(), Config::default());
        timed_out.feed_output(b"\x1b[?2026hY\x1b[6n");
        assert!(timed_out.drain_protocol_replies().is_empty());
        std::thread::sleep(SYNC_WATCHDOG_DELAY + Duration::from_millis(75));
        assert_eq!(timed_out.drain_protocol_replies(), b"\x1b[1;2R");
    }

    #[test]
    fn stale_sync_watchdog_cannot_commit_a_refreshed_batch() {
        let engine = Arc::new(Mutex::new(Engine::new(Size::default(), &Config::default())));
        engine
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .feed_detailed(b"\x1b[?2026hX\x1b[6n");
        let events = Arc::new(Mutex::new(VecDeque::new()));
        let wakeup_queued = Arc::new(AtomicBool::new(false));
        let wakeup_enabled = Arc::new(AtomicBool::new(true));
        let sync_batch_active = Arc::new(AtomicBool::new(true));
        let sync_generation = Arc::new(AtomicU64::new(2));
        let captured = Arc::new(Mutex::new(Vec::new()));
        let sink_capture = captured.clone();
        let protocol_reply_sink: ProtocolReplySink = Arc::new(move |reply| {
            sink_capture
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .extend(reply);
        });
        let task = |generation| SyncWatchdogTask {
            deadline: Instant::now(),
            engine: engine.clone(),
            events: events.clone(),
            wakeup_queued: wakeup_queued.clone(),
            wakeup_enabled: wakeup_enabled.clone(),
            wakeup_notifier: None,
            sync_batch_active: sync_batch_active.clone(),
            sync_generation: sync_generation.clone(),
            protocol_reply_sink: protocol_reply_sink.clone(),
            generation,
        };

        execute_sync_watchdog_task(task(1));
        assert_eq!(
            engine
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .grid
                .line(0)
                .unwrap()[0]
                .character,
            ' '
        );
        assert!(captured.lock().unwrap().is_empty());

        execute_sync_watchdog_task(task(2));
        assert_eq!(
            engine
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .grid
                .line(0)
                .unwrap()[0]
                .character,
            'X'
        );
        assert_eq!(*captured.lock().unwrap(), b"\x1b[1;2R");
        assert_eq!(*events.lock().unwrap(), VecDeque::from([Event::Wakeup]));
    }

    #[test]
    fn hydration_does_not_leak_incomplete_parser_state_into_live_output() {
        let terminal = |hydrated: &[u8], live: &[u8]| {
            let terminal = Terminal::new_display(
                Size {
                    cols: 12,
                    rows: 2,
                    ..Size::default()
                },
                Config::default(),
            );
            terminal.hydrate_output(hydrated);
            terminal.feed_output(live);
            terminal
        };

        let csi = terminal(b"\x1b[31", b"mX");
        assert_eq!(csi.snapshot().cells[0].character, 'm');
        assert_eq!(csi.snapshot().cells[1].character, 'X');
        assert_eq!(csi.snapshot().cells[1].foreground, Color::Default);

        let osc = terminal(b"\x1b]2;stale", b"live");
        assert_eq!(
            osc.snapshot().cells[..4]
                .iter()
                .map(|cell| cell.character)
                .collect::<String>(),
            "live"
        );

        let dcs = terminal(b"\x1bP$q", b"DCS");
        assert_eq!(
            dcs.snapshot().cells[..3]
                .iter()
                .map(|cell| cell.character)
                .collect::<String>(),
            "DCS"
        );

        let apc = terminal(b"\x1b_Ga=T,f=32,s=1,v=1;AAAA", b"APC");
        assert_eq!(
            apc.snapshot().cells[..3]
                .iter()
                .map(|cell| cell.character)
                .collect::<String>(),
            "APC"
        );

        let utf8 = terminal(&[0xe2, 0x9d], b"UTF8");
        assert_eq!(
            utf8.snapshot().cells[..4]
                .iter()
                .map(|cell| cell.character)
                .collect::<String>(),
            "UTF8"
        );

        let sync = terminal(b"\x1b[?2026hdiscarded", b"SYNC");
        assert_eq!(
            sync.snapshot().cells[..4]
                .iter()
                .map(|cell| cell.character)
                .collect::<String>(),
            "SYNC"
        );

        let repeat = terminal(b"H", b"\x1b[3bX");
        assert_eq!(
            repeat.snapshot().cells[..2]
                .iter()
                .map(|cell| cell.character)
                .collect::<String>(),
            "HX"
        );
    }

    #[test]
    fn hydration_keeps_completed_style_charset_title_and_graphics_state() {
        let terminal = Terminal::new_display(
            Size {
                cols: 12,
                rows: 2,
                ..Size::default()
            },
            Config::default(),
        );
        terminal.hydrate_output(
            b"\x1b[31m\x1b)0\x0e\x1b]2;saved\x07\x1b_Ga=T,f=32,s=1,v=1,i=91,C=1;/wAA/w==\x1b\\",
        );
        terminal.feed_output(b"q\x1b[22t\x1b]2;temporary\x07\x1b[23t");

        let snapshot = terminal.snapshot();
        assert_eq!(snapshot.cells[0].character, '\u{2500}');
        assert_eq!(snapshot.cells[0].foreground, Color::Indexed(1));
        assert_eq!(terminal.kitty_graphics_placements().len(), 1);
        assert!(
            terminal
                .drain_events()
                .0
                .contains(&Event::Title("saved".to_string()))
        );
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
                .chain([Event::ClipboardStore("preserved".to_string())])
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
        assert_eq!(
            queue
                .iter()
                .filter(|event| matches!(event, Event::ClipboardStore(_)))
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
    fn event_queue_preserves_state_updates_across_lifecycle_events() {
        let events = Arc::new(Mutex::new(VecDeque::new()));
        let wakeup_queued = AtomicBool::new(false);
        let wakeup_enabled = Arc::new(AtomicBool::new(false));
        queue_events(
            &events,
            &wakeup_queued,
            &wakeup_enabled,
            None,
            [
                Event::Title("termy:tab:command:cargo test".to_string()),
                Event::ShellCommandFinished(Some(0)),
                Event::Title("termy:tab:prompt:/repo".to_string()),
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
                Event::Title("termy:tab:command:cargo test".to_string()),
                Event::ShellCommandFinished(Some(0)),
                Event::Title("termy:tab:prompt:/repo".to_string()),
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
    fn hidden_terminal_notifies_once_for_exit() {
        let events = Arc::new(Mutex::new(VecDeque::new()));
        let wakeup_queued = AtomicBool::new(false);
        let wakeup_enabled = Arc::new(AtomicBool::new(false));
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
                [Event::Exit],
                false,
                true,
            );
        }

        assert_eq!(notifications.load(Ordering::Relaxed), 1);
        assert_eq!(
            *events
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
            VecDeque::from([Event::Exit])
        );
    }

    #[test]
    fn hidden_terminal_notifies_for_clipboard_load() {
        let events = Arc::new(Mutex::new(VecDeque::new()));
        let wakeup_queued = AtomicBool::new(false);
        let wakeup_enabled = Arc::new(AtomicBool::new(false));
        let notifications = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let notifier_notifications = notifications.clone();
        let notifier = WakeupNotifier::new(move || {
            notifier_notifications.fetch_add(1, Ordering::Relaxed);
        });
        let request = ClipboardRequest {
            target: ClipboardTarget::Clipboard,
            selector: b'c',
            bell_terminated: false,
        };

        queue_events(
            &events,
            &wakeup_queued,
            &wakeup_enabled,
            Some(&notifier),
            [Event::ClipboardLoad(request)],
            false,
            true,
        );

        assert_eq!(notifications.load(Ordering::Relaxed), 1);
        assert!(matches!(
            events
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .front(),
            Some(Event::ClipboardLoad(_))
        ));
    }

    #[test]
    fn hidden_content_wakeup_is_signalled_once_when_reenabled() {
        let notifications = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let notifier_notifications = notifications.clone();
        let mut terminal = Terminal::new_display(Size::default(), Config::default());
        terminal.wakeup_notifier = Some(WakeupNotifier::new(move || {
            notifier_notifications.fetch_add(1, Ordering::Relaxed);
        }));

        terminal.set_wakeup_enabled(false);
        terminal.feed_output(b"content");
        assert_eq!(notifications.load(Ordering::Relaxed), 0);

        terminal.set_wakeup_enabled(true);
        terminal.set_wakeup_enabled(true);
        assert_eq!(notifications.load(Ordering::Relaxed), 1);
        assert_eq!(terminal.drain_events().0, vec![Event::Wakeup]);
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
    fn width_reflow_preserves_placements_and_reusable_image_data() {
        let terminal = graphics_test_terminal(8);
        terminal.feed_output(b"\x1b_Ga=T,f=32,s=1,v=1,i=17,c=2,r=1,C=1,q=1;AQID/w==\x1b\\");
        let original = terminal.kitty_graphics_placements();
        assert_eq!(original.len(), 1);
        let original_serial = original[0].placement_serial;

        terminal.resize(Size {
            cols: 5,
            ..terminal.size()
        });
        let resized = terminal.kitty_graphics_placements();
        assert_eq!(resized.len(), 1);
        assert_eq!(resized[0].placement_serial, original_serial);

        terminal.feed_output(b"\x1b_Ga=p,i=17,c=1,r=1,C=1,q=1\x1b\\");
        let placements = terminal.kitty_graphics_placements();
        assert_eq!(placements.len(), 2);
        assert!(placements.iter().all(|placement| placement.image_id == 17));
        assert!(
            placements
                .iter()
                .any(|placement| placement.placement_serial == original_serial)
        );
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
    fn unix_pty_reports_chdir_failure_synchronously() {
        let missing_directory = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join(format!(".missing-tmon-cwd-{}", std::process::id()));
        assert!(!missing_directory.exists());
        let error = Terminal::new(
            Size::default(),
            Config {
                working_directory: Some(missing_directory),
                launch: Some(Launch::ShellCommand("printf should-not-run".to_string())),
                ..Config::default()
            },
            None,
        )
        .err()
        .expect("a missing child working directory should fail the spawn handshake");
        assert!(
            error
                .to_string()
                .contains("failed to change terminal working directory"),
            "unexpected PTY spawn error: {error}"
        );
    }
}
