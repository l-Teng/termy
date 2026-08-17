//! Shared, sanitized byte traces captured from real terminal applications.

#![forbid(unsafe_code)]

/// One replayable terminal application output trace.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TerminalTrace {
    pub id: &'static str,
    pub application: &'static str,
    pub application_version: &'static str,
    pub cols: u16,
    pub rows: u16,
    encoded: &'static str,
}

impl TerminalTrace {
    /// Decode the fixture into the exact sanitized byte stream used for replay.
    pub fn bytes(&self) -> Vec<u8> {
        decode_hex(self.encoded)
    }
}

/// Initial Claude Code frame whose input prompt uses a reverse-video blank as
/// its visible cursor while the real terminal cursor is hidden.
pub const CLAUDE_CODE_2_1_233_INITIAL_FRAME: TerminalTrace = TerminalTrace {
    id: "claude-code-2.1.233-initial-frame",
    application: "Claude Code",
    application_version: "2.1.233",
    cols: 80,
    rows: 24,
    encoded: include_str!("traces/claude_code_2_1_233_initial_frame.hex"),
};

/// Expected viewport position of Claude Code's reverse-video cursor cell.
pub const CLAUDE_CODE_2_1_233_CURSOR_CELL: (usize, usize) = (21, 2);

/// A clean Neovim frame with line numbers, a highlighted cursor line,
/// truecolor syntax, Unicode text, and a colored undercurl.
pub const NEOVIM_0_12_4_RUST_FRAME: TerminalTrace = TerminalTrace {
    id: "neovim-0.12.4-rust-frame",
    application: "Neovim",
    application_version: "0.12.4",
    cols: 80,
    rows: 24,
    encoded: include_str!("traces/neovim_0_12_4_rust_frame.hex"),
};

/// Expected viewport position of Neovim's steady block cursor.
pub const NEOVIM_0_12_4_CURSOR_CELL: (usize, usize) = (1, 20);

/// Every trace in the compatibility corpus.
pub const TERMINAL_TRACES: &[TerminalTrace] =
    &[CLAUDE_CODE_2_1_233_INITIAL_FRAME, NEOVIM_0_12_4_RUST_FRAME];

fn decode_hex(encoded: &str) -> Vec<u8> {
    let mut decoded = Vec::new();
    for (line_index, line) in encoded.lines().enumerate() {
        let payload = line.split_once('#').map_or(line, |(payload, _)| payload);
        for token in payload.split_ascii_whitespace() {
            assert!(
                token.len().is_multiple_of(2),
                "invalid odd-length hex token on line {}",
                line_index + 1
            );
            for pair in token.as_bytes().chunks_exact(2) {
                let pair = std::str::from_utf8(pair).expect("hex fixture must be ASCII");
                decoded.push(u8::from_str_radix(pair, 16).unwrap_or_else(|error| {
                    panic!("invalid hex byte on line {}: {error}", line_index + 1)
                }));
            }
        }
    }
    decoded
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_trace_decodes_and_matches_its_declared_terminal_size() {
        let mut ids = std::collections::BTreeSet::new();
        for trace in TERMINAL_TRACES {
            assert!(ids.insert(trace.id), "duplicate trace ID: {}", trace.id);
            assert!(!trace.bytes().is_empty(), "{} is empty", trace.id);
            assert!(
                !trace.application.is_empty(),
                "{} has no application",
                trace.id
            );
            assert!(
                !trace.application_version.is_empty(),
                "{} has no application version",
                trace.id
            );
            assert!(trace.cols > 0, "{} has no columns", trace.id);
            assert!(trace.rows > 0, "{} has no rows", trace.id);
        }
    }

    #[test]
    fn claude_code_fixture_keeps_the_regression_controls() {
        let bytes = CLAUDE_CODE_2_1_233_INITIAL_FRAME.bytes();

        assert!(bytes.windows(6).any(|window| window == b"\x1b[?25l"));
        assert!(bytes.windows(5).any(|window| window == b"\x1b[7m "));
        assert!(bytes.windows(5).any(|window| window == b"\x1b[27m"));
        assert!(!bytes.windows(11).any(|window| window == b"~/Dev/termy"));
        assert!(bytes.windows(11).any(|window| window == b"~/workspace"));
    }

    #[test]
    fn neovim_fixture_keeps_real_tui_modes_and_rich_styles() {
        let bytes = NEOVIM_0_12_4_RUST_FRAME.bytes();
        let contains = |needle: &[u8]| bytes.windows(needle.len()).any(|window| window == needle);

        assert_eq!(bytes.len(), 6_258);
        assert!(contains(b"\x1b[?1049h"));
        assert!(!contains(b"\x1b[?1049l"));
        assert!(contains(b"\x1b[?2026h"));
        assert!(contains(b"\x1b[?2026l"));
        assert!(contains(b"\x1b[4:3m"));
        assert!(contains(b"\x1b[58:2::246:193:119m"));
        assert!(contains(b"Termy"));
        assert!(contains("✓".as_bytes()));
        assert!(contains("界".as_bytes()));
        assert!(!contains(b"/Users/"));
        assert!(!contains(b"/private/tmp"));
        assert!(!contains(b"E303:"));
    }
}
