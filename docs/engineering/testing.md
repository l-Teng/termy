# Testing strategy

Where tests live and which command to run for a given change.

## Pyramid

| Layer | What | Where | Command |
|-------|------|-------|---------|
| **Unit** | Pure logic, parsers, catalogs | `config_core`, `command_core`, `search`, `core`, inline `#[test]` | `cargo test -p <crate>` |
| **Compatibility replay** | Sanitized output from real terminal applications | `terminal_test_support`, `tmon`, `core`, `desktop_app` | `cargo test -p termy_terminal_test_support -p tmon -p termy_core -p termy` |
| **Integration** | Tmux client, grid, FFI | `terminal_ui/tests/`, `ffi` | `cargo test -p termy_terminal_ui` |
| **App** | GPUI terminal view, settings, commands | `desktop_app` | `just test` |
| **Manual** | Visual chrome, GPU paint | — | Run app; see [development.md](../development.md) render metrics |

## Ignored tests

- `crates/terminal_ui/tests/tmux_split_integration.rs` — requires **tmux ≥ 3.3** locally.
- Run: `just test-tmux-integration`
- CI: macOS `architecture-checks` job (when tmux available).

Every `#[ignore]` must reference a tracking issue in a comment.
`just check-boundaries` enforces that the repo stays at or below 10 ignored tests.

## Real-application replay traces

`crates/terminal_test_support` stores sanitized output captured from real TUIs.
The same bytes are replayed through the engine, Core's renderer-neutral facade,
and the desktop renderer when a regression crosses those boundaries. Keep each
fixture's original control-byte ordering, but remove credentials, user input,
machine names, and local paths. Record the source application, version, and
terminal dimensions in the trace catalog.

Add the narrowest assertion at each affected layer. Engine tests own parser and
grid semantics, Core tests own public render-cell mapping, and desktop tests own
resolved visual behavior. Replay at least one fragmented feed so an application
trace also checks incremental parser boundaries.

## Before opening a PR

Use the **smallest** pass that proves your change:

```sh
cargo check -p termy              # UI-only tweak
cargo test -p termy_config_core   # config schema/parser
just check-boundaries             # deps, generated docs, commands
just test-workspace               # broad Rust change (target: matches CI E0.1)
just validate                     # full local gate (once E0 lands)
```

## Roadmap

CI parity and tmux reliability: [roadmap.md](roadmap.md) phase E0–E2.
