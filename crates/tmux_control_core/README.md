# termy_tmux_control_core

Shared, UI-agnostic core for tmux **control mode** (`tmux -CC`): command-line construction, payload escaping, the control-stream parser/state machine, notification coalescing, session launch, and worker channel plumbing.

## Owner

This crate owns tmux control-mode contracts that must be shared by `termy_terminal_ui` and `termy_ffi`. It has no GPUI, app UI, or `termy_terminal_ui` dependency.

Pane/layout integration and GPUI state live in consumers such as `termy_terminal_ui::tmux`.

## Validation

```sh
cargo test -p termy_tmux_control_core
```

## Forbidden Dependencies

- `gpui`
- `termy_terminal_ui`
- `termy_ffi`
- `termy` / `crates/desktop_app`
