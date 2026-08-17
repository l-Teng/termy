# Terminal test support

Shared, sanitized terminal-application output traces for compatibility tests.

## Owner

This crate owns test-only trace metadata and decoding. Tmon, `termy_core`, and
the desktop renderer consume the same bytes as development dependencies so a
real application regression can be checked at each boundary without exposing
fixtures through production APIs.

Trace files are hexadecimal byte streams because terminal output is binary and
must preserve incomplete UTF-8 and escape-sequence boundaries. Comments begin
with `#`. New traces must be minimized when practical, identify the source
application and version, keep the control-byte ordering that triggered the
behavior, and remove credentials, user input, machine names, and local paths.

## Corpus

| Application | Version | Coverage |
| --- | --- | --- |
| Claude Code | 2.1.233 | Hidden hardware cursor with a reverse-video prompt cursor |
| Neovim | 0.12.4 | Alternate screen, synchronized updates, mouse and paste modes, truecolor, Unicode width, and colored undercurl |

The Neovim fixture was captured at 80x24 from the
[official macOS ARM64 release](https://github.com/neovim/neovim/releases/tag/v0.12.4)
with `-u` pointing at a deterministic minimal configuration and all XDG state
isolated outside the repository. The capture harness answered the same terminal
capability queries Tmon supports, then stopped after Neovim committed its final
synchronized frame. The fixture contains output bytes only, never terminal
replies sent back to Neovim.

## Validation

```sh
cargo test -p termy_terminal_test_support
cargo clippy -p termy_terminal_test_support --all-targets -- -D warnings
```

## Forbidden Dependencies

- Product crates, terminal engines, GPUI, and platform APIs.
- Normal production dependencies from any Termy crate; this crate is for
  development dependencies only.
