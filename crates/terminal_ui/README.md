# termy_terminal_ui

GPUI-facing terminal runtime support for the desktop app.

## Owner

This crate owns the terminal grid paint cache, GPUI painting and final pixel snapping, GPUI keystroke adapter, tmux pane display runtime, and tmux client support used by `crates/desktop_app/src/terminal_view/`. Renderer-neutral special-glyph semantics come from `termy_core`; this crate converts those shared plans into GPUI quads and paths.

## Validation

```sh
cargo test -p termy_terminal_ui
```

## Forbidden Dependencies

- `termy_ffi`
- `termy` / `crates/desktop_app`
- app settings, workspace stores, or command execution workflows
