# termy_core

Reusable headless libtermy runtime and API.

## Owner

This crate owns terminal lifecycle, frame snapshots, keyboard/mouse protocol helpers, search over frames, config-to-runtime conversion, shell integration state, and embedder-facing render metrics. It must remain independent of GPUI and desktop app chrome.

Use this crate when behavior should be available to FFI, WASM, JS, or non-GPUI host examples.

## Validation

```sh
cargo test -p termy_core
```

## Forbidden Dependencies

- `gpui`
- `termy_terminal_ui`
- `termy_ffi`
- `termy` / `crates/desktop_app`

## Rust render contracts

`Terminal::snapshot` and `Terminal::frame_update` are the stable flat-cell
contract used by the C ABI. Rust renderers that need complete combining text,
underline variants and colors, live palette revisions, ordered viewport scroll
damage, or generation-checked range reads should use `Terminal::render_read`,
`Terminal::take_render_damage_snapshot`, and the `visit_*` methods instead.

The terminal engine is deliberately private. `termy_core` 0.2 removed
`Terminal::with_term`, `TerminalOptions::term_config`, and the exported raw
Alacritty conversion helpers. Embedders should use core-owned types such as
`TerminalColor`, `TerminalRenderCell`, and `TerminalQueryColors`, plus the
neutral `Terminal` methods, rather than depending on an engine grid or parser.

## Why libtermy instead of a VT engine directly?

A VT engine provides parser, grid, and PTY primitives. libtermy wraps those
primitives with the harness every embedder ends up writing anyway:

- **Stable frame API** — `TermyFrame` / `TermyCell` / `TermyColor` snapshots so renderers do not couple to engine-internal grid types.
- **Damage tracking** — `TerminalDamageSnapshot` and dirty spans for partial redraws and cell-cache renderers.
- **Input encoding** — `keystroke_to_input`, mouse report encoder, and keyboard/mouse mode state (xterm, SGR, kitty, etc.).
- **Search** — `search_frame` over snapshots without re-implementing grid traversal.
- **Shell integration** — OSC 133 / 633 / 1337 command lifecycle, progress state, and tab title resolution.
- **OSC interception** — clipboard, color queries, and a reply-host abstraction so embedders do not parse OSC themselves.
- **Link detection** — URL and path classification for click-to-open.
- **Config bridge** — `AppConfig` → `TerminalRuntimeConfig`, theme color resolution, terminal query colors.
- **Font + cell metrics** — `measure_cell` backed by fontdb / ttf-parser for embedder layout.
- **Render metrics** — span timings (grid paint, text shaping, cache hit/miss) for performance debugging.
- **Launch resolution** — working directory normalization, fallbacks, locale, and PATH setup.
- **PTY plumbing** — `rustix-openpty` on Unix, shell discovery via `winreg` on Windows.
- **Portable surface** — no GPUI dependency. The same crate powers the desktop app, `termy_ffi` (C ABI), WASM hosts, JS bindings, and headless tests. Wrap the engine once, host many times.

In short: the VT engine is an implementation detail; libtermy is the portable
terminal contract plus the harness embedders actually need.
