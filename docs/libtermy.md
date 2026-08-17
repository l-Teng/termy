# libtermy

`libtermy` is the embeddable Termy terminal engine. The first cut is split into:

- `termy_core`: Rust API for a single headless terminal surface.
- `termy_ffi`: C ABI wrapper over `termy_core`.

The core API owns PTY startup, terminal parsing, input writes, resize, event
draining, damage snapshots, and renderer-neutral frame snapshots. It does not
depend on GPUI and does not expose Termy's app chrome, tabs, panes, or tmux
session model as part of the public v1 surface.

`termy_core` can also load Termy's normal config format through the existing
headless `termy_config_core` parser. Config loading returns the full parsed
`AppConfig`, parse diagnostics, and a `TerminalRuntimeConfig` ready to pass into
terminal creation. Missing default config files fall back to defaults without
creating files; explicit missing paths are reported as load errors.

## Rust Embedding

Use `termy_core::Terminal` directly:

```rust
let loaded_config = termy_core::load_config_from_default_path()?;
let cell_metrics = termy_core::measure_cell_from_config(&loaded_config.app_config);
let terminal = termy_core::Terminal::new(
    termy_core::TerminalSize {
        cols: 80,
        rows: 24,
        cell_width: cell_metrics.cell_width,
        cell_height: cell_metrics.cell_height,
    },
    None,
    None,
    None,
    Some(&loaded_config.runtime_config),
    None,
)?;
terminal.try_write(b"echo hello\r")?;
let frame = terminal.snapshot();
```

`TermyFrame` contains a flat row-major `Vec<TermyCell>`, cursor state, scroll
state, and cell colors as simple RGBA bytes.

PTY-aware Rust hosts should use `Terminal::try_write`, `try_write_owned`,
`try_resize`, and `try_nudge_resize` when they need to react to a disconnected
PTY. Tmon also reports a full bounded input queue. The original void methods
remain source-compatible wrappers and log failures. With Tmon, a successful
live resize updates the in-memory grid only after the platform PTY acknowledges
the new dimensions; a failed resize leaves the previous grid size intact.
Display-only terminals accept writes as no-ops.

Rust renderers that need more than the stable flat-cell contract can use
`Terminal::render_read`. Its `TerminalRenderRead` contains coherent viewport
metadata, a revisioned live palette, complete cell text, exact underline style
and color, ordered viewport scroll damage, and a render generation. For a
damage-first renderer, call `take_render_damage_snapshot` and then
`visit_viewport_ranges_at_generation`; a `false` result means the generation
changed and the renderer must take a fresh read. The `visit_viewport_cells` and
`visit_line_cells` methods provide engine-neutral full-viewport and buffer-line
access without copying an implementation-specific grid into the host. The
`visit_line_cells` callback runs under the terminal lock to keep a full-buffer
read coherent and allocation-bounded; it must not call back into that terminal.

The underlying terminal engine is not part of the Rust embedding contract.
`termy_core` 0.2 removes `Terminal::with_term`,
`TerminalOptions::term_config`, and the raw engine conversion helpers. Use `TerminalColor`,
`TerminalRenderCell`, `TerminalQueryColors`, and the public `Terminal` methods
instead. This is an intentional Rust source break; the flat frame types and C
ABI remain unchanged.

Native and display-only terminals use Tmon exclusively. Launch, configuration,
PTY, and backend-initialization errors are returned without an engine retry.

Use `termy_core::measure_cell(font_family, font_size, line_height)` or
`termy_core::measure_cell_from_config(&app_config)` to derive the
`TerminalSize` cell width and height from Termy's font metrics instead of
guessing a monospace ratio in the host app.

## C ABI

Use `termy_ffi` as an opaque-handle API:

- `crates/ffi/include/termy.h`
- `termy_config_load_default`
- `termy_config_load_path`
- `termy_config_from_contents`
- `termy_config_free`
- `termy_terminal_new_with_config`
- `termy_terminal_new_with_options`
- `termy_terminal_new`
- `termy_terminal_free`
- `termy_terminal_write`
- `termy_terminal_resize`
- `termy_terminal_snapshot`
- `termy_frame_free`
- `termy_terminal_take_damage`
- `termy_damage_free`
- `termy_terminal_drain_events`
- `termy_event_batch_free`
- `termy_terminal_search`
- `termy_search_batch_free`

Any returned frame, damage, event batch, search batch, or standalone byte payload must be
released by the matching `termy_*_free` function. Event payloads owned by an
event batch are freed by `termy_event_batch_free`; do not free them separately.
Search match line payloads owned by a search batch are freed by
`termy_search_batch_free`.
Embedders should synchronize access to a terminal handle if they call into it
from multiple threads.

`termy_terminal_write` and `termy_terminal_resize` return
`TERMY_FFI_WRITE_FAILED` when Tmon rejects the write or the platform PTY cannot
apply the resize. This reuses the existing status value and does not change the
C header or ABI.

`termy_terminal_new_with_options` is the host-control constructor. It accepts an
optional loaded config plus a launch working directory, startup command, and a
list of environment variable overrides. The environment array is copied during
the call and may be released by the caller after the constructor returns.

Config diagnostics are available through `termy_config_diagnostics` and must be
released with `termy_config_diagnostics_free`. Diagnostic kind values:

- `1`: unknown section
- `2`: unknown root key
- `3`: unknown color key
- `4`: invalid syntax
- `5`: invalid value
- `6`: duplicate root key

Renderer-facing config values are available through `termy_config_render_config`
and must be released with `termy_render_config_free`. This returns the parsed
font family, active theme id, foreground/background/cursor colors, font size,
line height, padding, background opacity, cursor blink, and cursor style so
non-Rust embedders can render the terminal with the same user config that was
passed into `termy_terminal_new_with_config`. The render config also includes
`cell_width` and `cell_height`, measured by `termy_core`, for constructing
`TermyFfiSize` without duplicating font metric logic in the host app.

Event kind values:

- `1`: wakeup
- `2`: title
- `3`: reset title
- `4`: bell
- `5`: exit
- `6`: clipboard store
- `7`: shell prompt start
- `8`: shell command start
- `9`: shell command executing
- `10`: shell command finished
- `11`: progress
- `12`: working directory

Progress state values:

- `0`: clear
- `1`: in progress
- `2`: error
- `3`: indeterminate
- `4`: warning

Cursor style values:

- `1`: line
- `2`: block

Search returns visible-frame matches only. Each `TermyFfiSearchMatch` reports the
row, inclusive start and end columns, and the visible line text that matched.

The C ABI header is at `crates/ffi/include/termy.h`.
