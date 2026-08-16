# termy_terminal_ui

GPUI-facing terminal runtime support for the desktop app.

## Owner

This crate owns the terminal grid paint cache, GPUI keystroke adapter, tmux pane display runtime, and tmux client support used by `crates/desktop_app/src/terminal_view/`. `PaneTerminal` delegates parsing and render state to `termy_core::Terminal` display mode; this crate must not own a second terminal engine or expose its grid. It can depend on GPUI, but callers import shared headless terminal types directly from `termy_core` instead of through this crate.

## Validation

```sh
cargo test -p termy_terminal_ui
```

## Forbidden Dependencies

- `termy_ffi`
- `termy` / `crates/desktop_app`
- direct terminal-engine or parser dependencies
- app settings, workspace stores, or command execution workflows
