# termy_ui

Termy's settings design system as reusable GPUI components.

The crate also owns the pinned `glassy-ui` integration: startup initialization,
light/dark theme synchronization, bundled assets and fonts, and direct
re-exports of the published component API used by desktop surfaces.

## Owner

This crate owns Termy's chrome vocabulary: color tokens derived from the active
terminal theme, layout metrics, and Termy-specific stateless chrome — sidebar,
section headers, grouped cards, setting rows, slider, stepper, segmented
control, shortcut box, banner, toast, and empty state. Generic controls such as
buttons, switches, selects, inputs, badges, tooltips, dialogs, and menus come
directly from `glassy-ui`.

Keep product behavior out of here. Config keys, plugin inventory, SSH hosts, and
command execution stay in `crates/desktop_app/`; this crate only knows how a
value should look once someone hands it over. A component that cannot be
rendered from plain arguments belongs to the surface that owns the state.

Numbers mirror the shipped settings window
(`crates/desktop_app/src/settings_view/`), so a kit-built panel lines up with the
app's own panels. See `src/metrics.rs` for the full table and `src/theme.rs` for
the alpha ladder.

## Validation

```sh
cargo test -p termy_ui
cargo run -p termy_ui --example settings_gallery
```

The desktop app consumes this crate from `crates/desktop_app/src/settings_view/`.
After changing tokens or a component used there, also run:

```sh
cargo test -p termy settings
```

## Forbidden Dependencies

- `termy` / `crates/desktop_app`
- `termy_terminal_ui`
- `termy_config_core`, `termy_command_core`, `termy_plugin_runtime`, `termy_ssh_core`

`glassy-ui` is the allowed external component dependency. Keep both it and GPUI
exactly pinned at the workspace root so every element shares the same GPUI
types.
