mod query_colors;

pub use query_colors::TerminalQueryColors;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TerminalClipboardTarget {
    Clipboard,
    Selection,
}

pub trait TerminalReplyHost {
    fn load_clipboard(&mut self, target: TerminalClipboardTarget) -> Option<String>;
}

impl<F> TerminalReplyHost for F
where
    F: FnMut(TerminalClipboardTarget) -> Option<String>,
{
    fn load_clipboard(&mut self, target: TerminalClipboardTarget) -> Option<String> {
        self(target)
    }
}
