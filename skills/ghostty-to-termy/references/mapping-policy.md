# Mapping policy

Termy documentation snapshot: `https://termy.sh/llms-full.txt`, fetched
2026-07-25. Refresh the snapshot before changing mappings when current behavior
matters.

## Exact settings

| Ghostty | Termy | Notes |
| --- | --- | --- |
| `font-family` | `font_family` | Termy supports one family; report Ghostty fallbacks. |
| `background-opacity` | `background_opacity` | Require 0.0 through 1.0. |
| `background-opacity-cells` | `background_opacity_cells` | Boolean. |
| `cursor-style = block` | `cursor_style = block` | Other styles need special handling. |
| `cursor-style-blink` | `cursor_blink` | Boolean. |
| `mouse-scroll-multiplier = N` | `mouse_scroll_multiplier = N` | Prefixed per-device values are unsupported. |
| `working-directory` | `working_dir` | Preserve the path text. |
| `background`, `foreground`, `cursor-color` | `[colors]` values | Require a 6-digit hex value. |
| `palette = 0..15=#RRGGBB` | named `[colors]` entries | Map ANSI indexes to Termy color names. |

## Approximations

| Ghostty | Termy | Loss |
| --- | --- | --- |
| `font-size` | `font_size` | Ghostty documents points; Termy documents pixels. |
| `window-padding-x/y` | `padding_x/y` | UI coordinate behavior can vary by platform. |
| positive numeric `background-blur` | `background_blur = true` | Termy exposes a boolean, not blur intensity. |
| `cursor-style = bar` | `cursor_style = line` | Closest shape, not the same spelling or guaranteed geometry. |
| enabled/detected `shell-integration` | `shell_integration_enabled = true` | Termy owns its integration behavior. |

## Common unsupported settings

- Ghostty named or light/dark themes: do not assume Termy has the same slug.
- Ghostty fallback fonts, font features, synthetic styles, thickness, and cell
  adjustment settings.
- Background images, selection colors, cursor opacity, and underline colors.
- Window dimensions: Ghostty commonly expresses terminal cells while Termy
  documents startup pixels.
- Ghostty scrollback limits: Ghostty and Termy use different units/semantics.
- Quick terminal, global shortcuts, macOS/GTK-specific chrome, shaders, and
  surface/window placement.
- Startup `command`: it is not necessarily equivalent to Termy's `shell`.

## Keybindings

Normalize triggers from `modifier+key` to `modifier-key`. Normalize modifier
aliases to `ctrl`, `alt`, `shift`, or `cmd`.

Use only documented Termy actions. Safe mappings include:

| Ghostty action | Termy action |
| --- | --- |
| `copy_to_clipboard` | `copy` |
| `paste_from_clipboard` | `paste` |
| `new_tab` | `new_tab` |
| `close_surface` | `close_pane_or_tab` |
| `close_tab` | `close_tab` |
| `previous_tab` / `next_tab` | `switch_tab_left` / `switch_tab_right` |
| `goto_tab:1..9` | `switch_to_tab_1..9` |
| `new_split:left/right` | `split_pane_vertical` |
| `new_split:up/down` | `split_pane_horizontal` |
| `goto_split:DIR` | `focus_pane_DIR` |
| `toggle_split_zoom` | `toggle_pane_zoom` |
| `increase_font_size` / `decrease_font_size` | `zoom_in` / `zoom_out` |
| `reset_font_size` | `zoom_reset` |
| `start_search` / `end_search` | `open_search` / `close_search` |
| `navigate_search:next/previous` | `search_next` / `search_previous` |
| `toggle_command_palette`, `open_config`, `check_for_updates`, `quit` | same name |

Ghostty `keybind = clear` maps to Termy's `keybind = clear`. `unbind` maps to
`unbind`, but `ignore` does not: Termy's `unbind` forwards input while Ghostty's
`ignore` consumes it.

Report key sequences (`>`), trigger prefixes (`global:`, `all:`, `unconsumed:`,
`performable:`), and actions that send arbitrary text or escape sequences.
