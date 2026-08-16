use super::*;
use std::ops::Deref;

/// The terminal-engine facade used by the desktop view. Backend-specific
/// details stay behind this enum so panes do not branch on engine internals.
#[allow(clippy::large_enum_variant)]
pub(super) enum Terminal {
    Tmux(PaneTerminal),
    Native(NativeTerminalInstance),
}

pub(super) struct NativeTerminalInstance {
    pub(super) wakeup_id: NativeTerminalWakeupId,
    pub(super) terminal: Mutex<NativeTerminal>,
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
    Tmux(&'a alacritty_terminal::term::cell::Cell),
    Native(&'a termy_core::TerminalRenderCell),
}

impl<'a> From<&'a alacritty_terminal::term::cell::Cell> for TerminalCellRef<'a> {
    fn from(cell: &'a alacritty_terminal::term::cell::Cell) -> Self {
        Self::Tmux(cell)
    }
}

impl<'a> From<&'a termy_core::TerminalRenderCell> for TerminalCellRef<'a> {
    fn from(cell: &'a termy_core::TerminalRenderCell) -> Self {
        Self::Native(cell)
    }
}

impl TerminalCellRef<'_> {
    pub(super) fn character(self) -> char {
        match self {
            Self::Tmux(cell) => cell.c,
            Self::Native(cell) => cell.text.chars().next().unwrap_or('\0'),
        }
    }

    pub(super) fn is_wide_spacer(self) -> bool {
        match self {
            Self::Tmux(cell) => cell
                .flags
                .intersects(Flags::WIDE_CHAR_SPACER | Flags::LEADING_WIDE_CHAR_SPACER),
            Self::Native(cell) => cell.wide_character_spacer || cell.leading_wide_character_spacer,
        }
    }

    pub(super) fn is_trailing_wide_spacer(self) -> bool {
        match self {
            Self::Tmux(cell) => cell.flags.contains(Flags::WIDE_CHAR_SPACER),
            Self::Native(cell) => cell.wide_character_spacer,
        }
    }

    pub(super) fn is_hidden(self) -> bool {
        match self {
            Self::Tmux(cell) => cell.flags.contains(Flags::HIDDEN),
            Self::Native(cell) => cell.hidden,
        }
    }

    pub(super) fn combining(self) -> Option<SharedString> {
        match self {
            Self::Tmux(_) => None,
            Self::Native(cell) => cell_text_suffix(cell)
                .map(str::to_owned)
                .map(SharedString::from),
        }
    }

    pub(super) fn append_combining_to(self, text: &mut String) {
        match self {
            Self::Tmux(_) => {}
            Self::Native(cell) => {
                if let Some(suffix) = cell_text_suffix(cell) {
                    text.push_str(suffix);
                }
            }
        }
    }
}

fn cell_text_suffix(cell: &termy_core::TerminalRenderCell) -> Option<&str> {
    let text = cell.text.as_str();
    let suffix_start = text.chars().next().map_or(0, char::len_utf8);
    text.get(suffix_start..).filter(|suffix| !suffix.is_empty())
}

pub(super) fn terminal_engine_label(terminal: Option<&Terminal>) -> &'static str {
    match terminal {
        Some(Terminal::Native(terminal)) => terminal
            .lock()
            .map_or("unknown", |terminal| terminal.engine_label()),
        Some(Terminal::Tmux(_)) => "alacritty",
        None => "-",
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

#[cfg(test)]
mod tests {
    use super::TerminalCellRef;

    #[test]
    fn core_cells_preserve_native_combining_text_after_convergence() {
        let terminal = termy_core::Terminal::new_display(termy_core::TerminalSize::default(), None);
        terminal.feed_output("e\u{301}".as_bytes());

        let mut observed = false;
        terminal.visit_viewport_cells(|_, _, _, cell| {
            let cell = TerminalCellRef::Native(cell);
            if cell.character() == 'e' {
                observed = true;
                assert_eq!(
                    cell.combining().map(|text| text.to_string()).as_deref(),
                    Some("\u{301}")
                );
                let mut suffix = String::new();
                cell.append_combining_to(&mut suffix);
                assert_eq!(suffix, "\u{301}");
            }
        });
        assert!(observed);
    }
}
