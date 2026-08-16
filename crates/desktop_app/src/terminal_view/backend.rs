use super::*;
use std::ops::Deref;

/// The terminal-engine facade used by the desktop view. Backend-specific
/// details stay behind this enum so panes do not branch on engine internals.
#[allow(clippy::large_enum_variant)]
pub(super) enum Terminal {
    Tmux(PaneTerminal),
    Native(NativeTerminalInstance),
    Tmon(TmonTerminalInstance),
}

pub(super) struct NativeTerminalInstance {
    pub(super) wakeup_id: NativeTerminalWakeupId,
    pub(super) terminal: Mutex<NativeTerminal>,
}

pub(super) struct TmonTerminalInstance {
    pub(super) wakeup_id: NativeTerminalWakeupId,
    pub(super) terminal: tmon::Terminal,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct TerminalLineRange {
    pub(super) first_line: i32,
    pub(super) last_line: i32,
    pub(super) columns: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum TerminalViewportScrollDirection {
    Up,
    Down,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct TerminalViewportScroll {
    pub(super) top: usize,
    pub(super) bottom: usize,
    pub(super) count: usize,
    pub(super) direction: TerminalViewportScrollDirection,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct TerminalRenderDamageSnapshot {
    pub(super) damage: TerminalDamageSnapshot,
    pub(super) scrolls: Vec<TerminalViewportScroll>,
    pub(super) generation: Option<u64>,
    pub(super) palette_revision: Option<u64>,
}

impl TerminalRenderDamageSnapshot {
    pub(super) fn from_core(update: termy_core::TerminalRenderDamageSnapshot) -> Self {
        Self {
            damage: update.damage,
            scrolls: update
                .scrolls
                .into_iter()
                .map(|scroll| TerminalViewportScroll {
                    top: scroll.top,
                    bottom: scroll.bottom,
                    count: scroll.count,
                    direction: match scroll.direction {
                        termy_core::TerminalViewportScrollDirection::Up => {
                            TerminalViewportScrollDirection::Up
                        }
                        termy_core::TerminalViewportScrollDirection::Down => {
                            TerminalViewportScrollDirection::Down
                        }
                    },
                })
                .collect(),
            generation: Some(update.generation),
            palette_revision: Some(update.palette_revision),
        }
    }

    pub(super) fn from_damage(damage: TerminalDamageSnapshot) -> Self {
        Self {
            damage,
            scrolls: Vec::new(),
            generation: None,
            palette_revision: None,
        }
    }
}

#[derive(Clone, Copy)]
pub(super) enum TerminalCellRef<'a> {
    Alacritty(&'a alacritty_terminal::term::cell::Cell),
    Core(&'a termy_core::TerminalRenderCell),
    Tmon(&'a tmon::Cell, Option<tmon::Combining<'a>>),
}

impl<'a> From<&'a alacritty_terminal::term::cell::Cell> for TerminalCellRef<'a> {
    fn from(cell: &'a alacritty_terminal::term::cell::Cell) -> Self {
        Self::Alacritty(cell)
    }
}

impl<'a> From<&'a termy_core::TerminalRenderCell> for TerminalCellRef<'a> {
    fn from(cell: &'a termy_core::TerminalRenderCell) -> Self {
        Self::Core(cell)
    }
}

impl<'a> From<&'a tmon::Cell> for TerminalCellRef<'a> {
    fn from(cell: &'a tmon::Cell) -> Self {
        Self::Tmon(cell, None)
    }
}

impl TerminalCellRef<'_> {
    pub(super) fn character(self) -> char {
        match self {
            Self::Alacritty(cell) => cell.c,
            Self::Core(cell) => cell.text.chars().next().unwrap_or('\0'),
            Self::Tmon(cell, _) => cell.character,
        }
    }

    pub(super) fn is_wide_spacer(self) -> bool {
        match self {
            Self::Alacritty(cell) => cell
                .flags
                .intersects(Flags::WIDE_CHAR_SPACER | Flags::LEADING_WIDE_CHAR_SPACER),
            Self::Core(cell) => cell.wide_character_spacer || cell.leading_wide_character_spacer,
            Self::Tmon(cell, _) => cell.wide_spacer() || cell.leading_wide_spacer(),
        }
    }

    pub(super) fn is_trailing_wide_spacer(self) -> bool {
        match self {
            Self::Alacritty(cell) => cell.flags.contains(Flags::WIDE_CHAR_SPACER),
            Self::Core(cell) => cell.wide_character_spacer,
            Self::Tmon(cell, _) => cell.wide_spacer(),
        }
    }

    pub(super) fn is_hidden(self) -> bool {
        match self {
            Self::Alacritty(cell) => cell.flags.contains(Flags::HIDDEN),
            Self::Core(cell) => cell.hidden,
            Self::Tmon(cell, _) => cell.attributes.hidden(),
        }
    }

    pub(super) fn combining(self) -> Option<SharedString> {
        match self {
            Self::Alacritty(_) => None,
            Self::Core(cell) => cell
                .text
                .chars()
                .next()
                .map(char::len_utf8)
                .filter(|&start| start < cell.text.len())
                .map(|start| SharedString::from(cell.text[start..].to_string())),
            Self::Tmon(_, combining) => combining
                .map(tmon::Combining::to_owned_string)
                .map(SharedString::from),
        }
    }

    pub(super) fn append_combining_to(self, text: &mut String) {
        match self {
            Self::Alacritty(_) => {}
            Self::Core(cell) => {
                if let Some(start) = cell.text.chars().next().map(char::len_utf8)
                    && start < cell.text.len()
                {
                    text.push_str(&cell.text[start..]);
                }
            }
            Self::Tmon(_, combining) => {
                if let Some(combining) = combining {
                    combining.append_to(text);
                }
            }
        }
    }
}

pub(super) fn tmon_engine_requested(value: Option<&std::ffi::OsStr>) -> bool {
    value.is_some_and(|value| value == "1")
}

pub(super) fn tmon_engine_available() -> bool {
    tmon::native_pty_available()
}

pub(super) fn tmon_engine_enabled_for(value: Option<&std::ffi::OsStr>, available: bool) -> bool {
    tmon_engine_requested(value) && available
}

pub(super) fn tmon_engine_enabled(value: Option<&std::ffi::OsStr>) -> bool {
    tmon_engine_enabled_for(value, tmon_engine_available())
}

pub(super) fn terminal_engine_label(terminal: Option<&Terminal>) -> &'static str {
    match terminal {
        Some(Terminal::Tmon(_)) => "tmon",
        Some(Terminal::Native(_) | Terminal::Tmux(_)) => "alacritty",
        None => "-",
    }
}

impl Deref for TmonTerminalInstance {
    type Target = tmon::Terminal;

    fn deref(&self) -> &Self::Target {
        &self.terminal
    }
}

impl Deref for NativeTerminalInstance {
    type Target = Mutex<NativeTerminal>;

    fn deref(&self) -> &Self::Target {
        &self.terminal
    }
}

#[derive(Default)]
pub(super) struct ClipboardTextCache {
    // Outer `Option` records whether GPUI has been queried; the inner value is
    // the clipboard result, which can legitimately be empty.
    value: Option<Option<String>>,
}

impl ClipboardTextCache {
    pub(super) fn get_or_read(&mut self, read: impl FnOnce() -> Option<String>) -> Option<String> {
        if let Some(value) = &self.value {
            return value.clone();
        }

        let value = read();
        self.value = Some(value.clone());
        value
    }
}

pub(super) struct GpuiClipboardReplyHost<'host, 'cx> {
    cx: &'host mut Context<'cx, TerminalView>,
    clipboard_text: &'host mut ClipboardTextCache,
}

impl<'host, 'cx> GpuiClipboardReplyHost<'host, 'cx> {
    pub(super) fn new(
        cx: &'host mut Context<'cx, TerminalView>,
        clipboard_text: &'host mut ClipboardTextCache,
    ) -> Self {
        Self { cx, clipboard_text }
    }
}

impl TerminalReplyHost for GpuiClipboardReplyHost<'_, '_> {
    fn load_clipboard(&mut self, _target: TerminalClipboardTarget) -> Option<String> {
        // GPUI exposes a single host clipboard source here, so both OSC 52
        // targets resolve through the same adapter.
        self.clipboard_text
            .get_or_read(|| self.cx.read_from_clipboard().and_then(|item| item.text()))
    }
}
