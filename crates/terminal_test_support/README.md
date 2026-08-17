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

## Validation

```sh
cargo test -p termy_terminal_test_support
cargo clippy -p termy_terminal_test_support --all-targets -- -D warnings
```

## Forbidden Dependencies

- Product crates, terminal engines, GPUI, and platform APIs.
- Normal production dependencies from any Termy crate; this crate is for
  development dependencies only.
