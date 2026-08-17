// Centralize PTY reply handling so protocol-specific branches stay out of runtime.rs.
mod query_colors;
mod replies;

pub use query_colors::TerminalQueryColors;
pub(crate) use replies::reply_bytes_for_event;
pub use replies::{TerminalClipboardTarget, TerminalReplyHost};
