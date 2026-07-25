#!/usr/bin/env python3
"""Convert the supported subset of Ghostty config to Termy config."""

from __future__ import annotations

import argparse
import json
import math
import os
import re
import sys
from dataclasses import dataclass
from pathlib import Path


ANSI_NAMES = [
    "black", "red", "green", "yellow", "blue", "magenta", "cyan", "white",
    "bright_black", "bright_red", "bright_green", "bright_yellow",
    "bright_blue", "bright_magenta", "bright_cyan", "bright_white",
]

SIMPLE_KEYS = {
    "font-family": "font_family",
    "background-opacity": "background_opacity",
    "background-opacity-cells": "background_opacity_cells",
    "cursor-style-blink": "cursor_blink",
    "working-directory": "working_dir",
}

KEY_ACTIONS = {
    "copy_to_clipboard": "copy",
    "paste_from_clipboard": "paste",
    "new_tab": "new_tab",
    "close_surface": "close_pane_or_tab",
    "close_tab": "close_tab",
    "previous_tab": "switch_tab_left",
    "next_tab": "switch_tab_right",
    "toggle_split_zoom": "toggle_pane_zoom",
    "increase_font_size": "zoom_in",
    "decrease_font_size": "zoom_out",
    "reset_font_size": "zoom_reset",
    "start_search": "open_search",
    "end_search": "close_search",
    "toggle_command_palette": "toggle_command_palette",
    "open_config": "open_config",
    "check_for_updates": "check_for_updates",
    "quit": "quit",
    "unbind": "unbind",
}


@dataclass(frozen=True)
class Entry:
    key: str
    value: str
    path: Path
    line: int

    @property
    def location(self) -> str:
        return f"{self.path}:{self.line}"


class Conversion:
    def __init__(self, source: Path) -> None:
        self.source = source
        self.settings: dict[str, str] = {}
        self.colors: dict[str, str] = {}
        self.keybinds: list[str] = []
        self.converted: list[dict[str, str]] = []
        self.approximated: list[dict[str, str]] = []
        self.unsupported: list[dict[str, str]] = []
        self.warnings: list[dict[str, str]] = []
        self._font_seen = False

    def note(
        self,
        bucket: list[dict[str, str]],
        entry: Entry,
        reason: str,
        target: str = "",
    ) -> None:
        item = {
            "location": entry.location,
            "ghostty_key": entry.key,
            "value": entry.value,
            "reason": reason,
        }
        if target:
            item["termy_target"] = target
        bucket.append(item)

    def set_value(
        self,
        entry: Entry,
        target: str,
        value: str,
        approximate: str | None = None,
    ) -> None:
        self.settings[target] = value
        if approximate:
            self.note(self.approximated, entry, approximate, target)
        else:
            self.note(self.converted, entry, "exact supported mapping", target)

    def convert_entry(self, entry: Entry) -> None:
        key, value = entry.key, entry.value
        if key == "config-file":
            self.note(self.converted, entry, "included file was inlined")
            return
        if value == "":
            self.note(
                self.unsupported,
                entry,
                "Ghostty empty-value reset has no safe automatic Termy equivalent",
            )
            return
        if key == "keybind":
            self.convert_keybind(entry)
            return
        if key in ("background", "foreground", "cursor-color"):
            target = {"cursor-color": "cursor"}.get(key, key)
            color = normalize_color(value)
            if color is None:
                self.note(
                    self.unsupported,
                    entry,
                    "Termy colors require 6-digit hexadecimal values",
                    f"colors.{target}",
                )
            else:
                self.colors[target] = color
                self.note(
                    self.converted,
                    entry,
                    "exact supported color mapping",
                    f"colors.{target}",
                )
            return
        if key == "palette":
            self.convert_palette(entry)
            return
        if key == "font-family":
            if self._font_seen:
                self.note(
                    self.unsupported,
                    entry,
                    "Termy exposes one font family; Ghostty fallback family was omitted",
                    "font_family",
                )
                return
            self._font_seen = True
            self.set_value(entry, "font_family", unquote(value))
            return
        if key == "font-size":
            if numeric(value, minimum=1):
                self.set_value(
                    entry,
                    "font_size",
                    value,
                    "Ghostty uses points while Termy documents pixels",
                )
            else:
                self.invalid(entry, "font size must be a positive number")
            return
        if key in ("window-padding-x", "window-padding-y"):
            if numeric(value, minimum=0):
                target = key.replace("window-", "").replace("-", "_")
                self.set_value(
                    entry,
                    target,
                    value,
                    "padding coordinate behavior can vary by platform",
                )
            else:
                self.invalid(entry, "padding must be a nonnegative number")
            return
        if key == "background-opacity":
            if numeric(value, minimum=0, maximum=1):
                self.set_value(entry, "background_opacity", value)
            else:
                self.invalid(entry, "opacity must be between 0 and 1")
            return
        if key in ("background-opacity-cells", "cursor-style-blink"):
            parsed = boolean(value)
            if parsed is None:
                self.invalid(entry, "expected a boolean")
            else:
                self.set_value(entry, SIMPLE_KEYS[key], parsed)
            return
        if key == "background-blur":
            parsed = boolean(value)
            if parsed is not None:
                self.set_value(entry, "background_blur", parsed)
            elif numeric(value, minimum=0):
                enabled = "false" if float(value) == 0 else "true"
                self.set_value(
                    entry,
                    "background_blur",
                    enabled,
                    "Termy supports blur on/off but not Ghostty blur intensity",
                )
            else:
                self.invalid(entry, "expected a boolean or nonnegative intensity")
            return
        if key == "cursor-style":
            style = value.lower()
            if style == "block":
                self.set_value(entry, "cursor_style", "block")
            elif style in ("bar", "line"):
                self.set_value(
                    entry,
                    "cursor_style",
                    "line",
                    "Termy's line cursor is the closest supported shape",
                )
            else:
                self.note(
                    self.unsupported,
                    entry,
                    "Termy documents only block and line cursor styles",
                    "cursor_style",
                )
            return
        if key == "mouse-scroll-multiplier":
            if numeric(value, minimum=0) and ":" not in value and "," not in value:
                self.set_value(entry, "mouse_scroll_multiplier", value)
            else:
                self.note(
                    self.unsupported,
                    entry,
                    "Termy has one multiplier, not Ghostty per-device multipliers",
                    "mouse_scroll_multiplier",
                )
            return
        if key == "working-directory":
            self.set_value(entry, "working_dir", unquote(value))
            return
        if key == "shell-integration":
            lowered = value.lower()
            if lowered in ("none", "false", "off"):
                self.set_value(entry, "shell_integration_enabled", "false")
            elif lowered in ("detect", "true", "bash", "zsh", "fish", "elvish"):
                self.set_value(
                    entry,
                    "shell_integration_enabled",
                    "true",
                    "Termy owns integration detection and shell hooks",
                )
            else:
                self.invalid(entry, "unrecognized shell integration mode")
            return
        self.note(
            self.unsupported,
            entry,
            "no verified Termy equivalent in the bundled documentation",
        )

    def invalid(self, entry: Entry, reason: str) -> None:
        self.note(self.warnings, entry, reason)
        self.note(self.unsupported, entry, "invalid or unsafe value was omitted")

    def convert_palette(self, entry: Entry) -> None:
        match = re.fullmatch(r"\s*(\d{1,2})\s*=\s*(.+?)\s*", entry.value)
        if not match:
            self.invalid(entry, "expected palette value INDEX=RRGGBB")
            return
        index = int(match.group(1))
        color = normalize_color(match.group(2))
        if index not in range(16) or color is None:
            self.invalid(entry, "palette index must be 0..15 with a 6-digit hex color")
            return
        target = ANSI_NAMES[index]
        self.colors[target] = color
        self.note(
            self.converted,
            entry,
            "ANSI palette index mapped to Termy color name",
            f"colors.{target}",
        )

    def convert_keybind(self, entry: Entry) -> None:
        value = entry.value.strip()
        if value == "clear":
            self.keybinds.append("clear")
            self.note(self.converted, entry, "clear defaults", "keybind")
            return
        if "=" not in value:
            self.invalid(entry, "expected TRIGGER=ACTION")
            return
        trigger, action = value.split("=", 1)
        converted_trigger, trigger_error = convert_trigger(trigger)
        converted_action, action_error = convert_action(action)
        if trigger_error or action_error:
            self.note(
                self.unsupported,
                entry,
                "; ".join(part for part in (trigger_error, action_error) if part),
                "keybind",
            )
            return
        self.keybinds.append(f"{converted_trigger}={converted_action}")
        self.note(self.converted, entry, "documented Termy keybinding mapping", "keybind")

    def render(self) -> str:
        lines = [
            "# Generated from Ghostty by the ghostty-to-termy skill.",
            "# Review the compatibility report for omitted or lossy settings.",
        ]
        for key, value in self.settings.items():
            lines.append(f"{key} = {value}")
        for binding in self.keybinds:
            lines.append(f"keybind = {binding}")
        if self.colors:
            lines.extend(["", "[colors]"])
            for key in ("foreground", "background", "cursor", *ANSI_NAMES):
                if key in self.colors:
                    lines.append(f"{key} = {self.colors[key]}")
        return "\n".join(lines) + "\n"

    def report(self) -> dict[str, object]:
        return {
            "source": str(self.source),
            "summary": {
                "converted": len(self.converted),
                "approximated": len(self.approximated),
                "unsupported": len(self.unsupported),
                "warnings": len(self.warnings),
            },
            "converted": self.converted,
            "approximated": self.approximated,
            "unsupported": self.unsupported,
            "warnings": self.warnings,
        }


def unquote(value: str) -> str:
    value = value.strip()
    if len(value) >= 2 and value[0] == value[-1] and value[0] in ("'", '"'):
        return value[1:-1]
    return value


def normalize_color(value: str) -> str | None:
    value = unquote(value)
    if value.startswith("#"):
        value = value[1:]
    if re.fullmatch(r"[0-9A-Fa-f]{6}", value):
        return f"#{value.lower()}"
    return None


def boolean(value: str) -> str | None:
    lowered = unquote(value).lower()
    if lowered in ("true", "yes", "on", "1"):
        return "true"
    if lowered in ("false", "no", "off", "0"):
        return "false"
    return None


def numeric(
    value: str,
    minimum: float | None = None,
    maximum: float | None = None,
) -> bool:
    try:
        parsed = float(unquote(value))
    except ValueError:
        return False
    return (
        math.isfinite(parsed)
        and (minimum is None or parsed >= minimum)
        and (maximum is None or parsed <= maximum)
    )


def convert_trigger(trigger: str) -> tuple[str, str | None]:
    trigger = trigger.strip().lower()
    prefixes = ("all:", "global:", "unconsumed:", "performable:")
    if trigger.startswith(prefixes):
        return "", "Ghostty keybinding prefixes are unsupported"
    if ">" in trigger:
        return "", "Ghostty key sequences are unsupported"
    aliases = {
        "control": "ctrl",
        "option": "alt",
        "opt": "alt",
        "command": "cmd",
    }
    parts = [part.strip() for part in trigger.split("+")]
    if not parts or any(not part for part in parts):
        return "", "invalid Ghostty trigger"
    parts = [aliases.get(part, part) for part in parts]
    allowed_modifiers = {"ctrl", "alt", "shift", "cmd", "secondary"}
    if any(modifier not in allowed_modifiers for modifier in parts[:-1]):
        return "", "trigger uses a modifier not documented by Termy"
    key = parts[-1]
    named_keys = {
        "enter", "escape", "tab", "space", "backspace", "delete", "insert",
        "home", "end", "pageup", "pagedown", "up", "down", "left", "right",
        "equal", "minus", "comma", "period", "slash", "backslash",
        "semicolon", "apostrophe", "grave",
    }
    simple_key = bool(re.fullmatch(r"[a-z0-9]", key))
    function_key = bool(re.fullmatch(r"f(?:[1-9]|1[0-9]|2[0-4])", key))
    if not (simple_key or function_key or key in named_keys):
        return "", "trigger key is not in the conservative Termy key allowlist"
    return "-".join(parts), None


def convert_action(action: str) -> tuple[str, str | None]:
    action = action.strip()
    if action in KEY_ACTIONS:
        return KEY_ACTIONS[action], None
    if action.startswith("goto_tab:"):
        number = action.removeprefix("goto_tab:")
        if number.isdigit() and 1 <= int(number) <= 9:
            return f"switch_to_tab_{number}", None
    if action.startswith("new_split:"):
        direction = action.removeprefix("new_split:")
        if direction in ("left", "right"):
            return "split_pane_vertical", None
        if direction in ("up", "down"):
            return "split_pane_horizontal", None
    if action.startswith("goto_split:"):
        direction = action.removeprefix("goto_split:")
        if direction in ("left", "right", "up", "down"):
            return f"focus_pane_{direction}", None
    if action in ("navigate_search:next", "navigate_search:forward"):
        return "search_next", None
    if action in ("navigate_search:previous", "navigate_search:backward"):
        return "search_previous", None
    return "", "Ghostty action has no verified Termy equivalent"


def parse_file(
    path: Path,
    seen: set[Path] | None = None,
) -> tuple[list[Entry], list[dict[str, str]]]:
    seen = seen or set()
    path = path.expanduser().resolve()
    if path in seen:
        return [], [{"location": str(path), "reason": "config-file include cycle"}]
    seen.add(path)
    entries: list[Entry] = []
    includes: list[Entry] = []
    warnings: list[dict[str, str]] = []
    try:
        lines = path.read_text(encoding="utf-8").splitlines()
    except (OSError, UnicodeError) as error:
        return [], [{"location": str(path), "reason": str(error)}]
    for number, raw in enumerate(lines, 1):
        stripped = raw.strip()
        if not stripped or stripped.startswith("#"):
            continue
        if "=" not in raw:
            warnings.append({
                "location": f"{path}:{number}",
                "reason": "line has no key/value separator",
            })
            continue
        key, value = raw.split("=", 1)
        entry = Entry(key.strip(), unquote(value.strip()), path, number)
        entries.append(entry)
        if entry.key == "config-file":
            includes.append(entry)
    for include in includes:
        optional = include.value.startswith("?")
        include_value = include.value[1:] if optional else include.value
        include_path = Path(os.path.expanduser(include_value))
        if not include_path.is_absolute():
            include_path = include.path.parent / include_path
        if optional and not include_path.exists():
            continue
        child_entries, child_warnings = parse_file(include_path, seen)
        entries.extend(child_entries)
        warnings.extend(child_warnings)
    return entries, warnings


def write_text(path: str | None, content: str) -> None:
    if path:
        target = Path(path).expanduser()
        target.parent.mkdir(parents=True, exist_ok=True)
        target.write_text(content, encoding="utf-8")
    else:
        sys.stdout.write(content)


def main() -> int:
    parser = argparse.ArgumentParser(
        description="Convert supported Ghostty settings into valid Termy config syntax."
    )
    parser.add_argument("source", help="Ghostty config file")
    parser.add_argument("-o", "--output", help="Termy config output; defaults to stdout")
    parser.add_argument("--report", help="JSON compatibility report path")
    parser.add_argument(
        "--strict",
        action="store_true",
        help="exit 2 when any setting is unsupported or a warning occurs",
    )
    args = parser.parse_args()

    source = Path(args.source).expanduser()
    entries, parse_warnings = parse_file(source)
    if not source.exists():
        print(f"error: source does not exist: {source}", file=sys.stderr)
        return 1

    conversion = Conversion(source.resolve())
    conversion.warnings.extend(parse_warnings)
    for entry in entries:
        conversion.convert_entry(entry)

    write_text(args.output, conversion.render())
    report = conversion.report()
    report_text = json.dumps(report, indent=2) + "\n"
    if args.report:
        write_text(args.report, report_text)
    else:
        summary = report["summary"]
        print(
            "compatibility: "
            f"{summary['converted']} exact, "
            f"{summary['approximated']} approximated, "
            f"{summary['unsupported']} unsupported, "
            f"{summary['warnings']} warnings",
            file=sys.stderr,
        )
    if args.strict and (conversion.unsupported or conversion.warnings):
        return 2
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
