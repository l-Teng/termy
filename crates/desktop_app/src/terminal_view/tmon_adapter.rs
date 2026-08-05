use super::{
    TerminalRenderDamageSnapshot, TerminalViewportScroll, TerminalViewportScrollDirection,
};
use termy_terminal_ui::{
    KittyGraphicsRenderPlacement, MAX_TERMINAL_SCROLLBACK_HISTORY, ProgressState,
    TabTitleShellIntegration, TerminalClipboardTarget, TerminalCursorState, TerminalCursorStyle,
    TerminalDamageSnapshot, TerminalDirtySpan, TerminalEvent, TerminalKeyboardMode, TerminalLaunch,
    TerminalMouseMode, TerminalOptions, TerminalQueryColors, TerminalRuntimeConfig, TerminalSize,
    resolve_launch_working_directory, resolve_terminal_launch, terminal_environment_overrides,
};

pub(super) fn size(size: TerminalSize) -> tmon::Size {
    tmon::Size {
        cols: size.cols,
        rows: size.rows,
        cell_width: size.cell_width,
        cell_height: size.cell_height,
    }
}

pub(super) fn terminal_size(size: tmon::Size) -> TerminalSize {
    TerminalSize {
        cols: size.cols,
        rows: size.rows,
        cell_width: size.cell_width,
        cell_height: size.cell_height,
    }
}

fn cursor_style(style: TerminalCursorStyle) -> tmon::CursorStyle {
    match style {
        TerminalCursorStyle::Line => tmon::CursorStyle::Line,
        TerminalCursorStyle::Block => tmon::CursorStyle::Block,
    }
}

fn terminal_cursor_style(style: tmon::CursorStyle) -> TerminalCursorStyle {
    match style {
        tmon::CursorStyle::Line => TerminalCursorStyle::Line,
        tmon::CursorStyle::Block => TerminalCursorStyle::Block,
    }
}

pub(super) fn cursor_state(state: tmon::CursorState) -> TerminalCursorState {
    TerminalCursorState {
        col: state.col,
        row: state.row,
        style: terminal_cursor_style(state.style),
    }
}

pub(super) fn options(options: TerminalOptions) -> tmon::TerminalOptions {
    tmon::TerminalOptions {
        scrollback_history: options
            .scrollback_history
            .min(MAX_TERMINAL_SCROLLBACK_HISTORY),
        default_cursor_style: cursor_style(options.default_cursor_style),
    }
}

pub(super) fn query_colors(colors: TerminalQueryColors) -> tmon::QueryColors {
    let rgb = |color: alacritty_terminal::vte::ansi::Rgb| tmon::Rgb {
        r: color.r,
        g: color.g,
        b: color.b,
    };
    tmon::QueryColors {
        ansi: colors.ansi.map(rgb),
        foreground: rgb(colors.foreground),
        background: rgb(colors.background),
        cursor: colors.cursor.map(rgb),
    }
}

pub(super) fn config(
    configured_working_dir: Option<&str>,
    shell_integration: Option<&TabTitleShellIntegration>,
    runtime_config: Option<&TerminalRuntimeConfig>,
    launch: Option<&TerminalLaunch>,
) -> anyhow::Result<tmon::Config> {
    let runtime_config = runtime_config.cloned().unwrap_or_default();
    let resolved_launch = resolve_terminal_launch(&runtime_config, launch)?;

    Ok(tmon::Config {
        // The core resolver is the single source of truth for shell selection
        // and arguments. Passing a typed program prevents Tmon from applying a
        // second, potentially divergent round of launch resolution.
        shell: None,
        working_directory: resolve_launch_working_directory(
            configured_working_dir,
            runtime_config.working_dir_fallback,
        ),
        environment: environment(shell_integration, &runtime_config),
        launch: Some(tmon::Launch::Program {
            program: resolved_launch.program,
            args: resolved_launch.args,
        }),
        scrollback_history: runtime_config
            .scrollback_history
            .min(MAX_TERMINAL_SCROLLBACK_HISTORY),
        default_cursor_style: cursor_style(runtime_config.default_cursor_style),
        query_colors: query_colors(runtime_config.query_colors),
        // Match the native Alacritty path: Termy handles both OSC 52 stores and
        // clipboard-load queries at the application boundary.
        osc52: tmon::Osc52::CopyPaste,
    })
}

fn environment(
    shell_integration: Option<&TabTitleShellIntegration>,
    runtime_config: &TerminalRuntimeConfig,
) -> Vec<(String, String)> {
    terminal_environment_overrides(shell_integration, runtime_config)
        .into_iter()
        .collect()
}

pub(super) fn event(event: tmon::Event) -> Option<TerminalEvent> {
    Some(match event {
        tmon::Event::Wakeup => TerminalEvent::Wakeup,
        tmon::Event::Title(title) => TerminalEvent::Title(title),
        tmon::Event::ResetTitle => TerminalEvent::ResetTitle,
        tmon::Event::Bell => TerminalEvent::Bell,
        tmon::Event::Exit => TerminalEvent::Exit,
        tmon::Event::ClipboardStore(text) => TerminalEvent::ClipboardStore(text),
        tmon::Event::ClipboardLoad(_) => return None,
        tmon::Event::ShellPromptStart => TerminalEvent::ShellPromptStart,
        tmon::Event::ShellCommandStart => TerminalEvent::ShellCommandStart,
        tmon::Event::ShellCommandExecuting => TerminalEvent::ShellCommandExecuting,
        tmon::Event::ShellCommandFinished(exit_code) => {
            TerminalEvent::ShellCommandFinished(exit_code)
        }
        tmon::Event::Progress(progress) => TerminalEvent::Progress(match progress {
            tmon::Progress::Clear => ProgressState::Clear,
            tmon::Progress::InProgress(value) => ProgressState::InProgress(value),
            tmon::Progress::Error(value) => ProgressState::Error(value),
            tmon::Progress::Indeterminate => ProgressState::Indeterminate,
            tmon::Progress::Warning(value) => ProgressState::Warning(value),
        }),
        tmon::Event::WorkingDirectory(path) => TerminalEvent::WorkingDirectory(path),
    })
}

pub(super) fn clipboard_target(target: tmon::ClipboardTarget) -> TerminalClipboardTarget {
    match target {
        tmon::ClipboardTarget::Clipboard => TerminalClipboardTarget::Clipboard,
        tmon::ClipboardTarget::Selection => TerminalClipboardTarget::Selection,
    }
}

pub(super) fn damage(damage: tmon::DamageSnapshot) -> TerminalDamageSnapshot {
    match damage {
        tmon::DamageSnapshot::Full => TerminalDamageSnapshot::Full,
        tmon::DamageSnapshot::Partial(spans) => TerminalDamageSnapshot::Partial(
            spans
                .into_iter()
                .map(|span| TerminalDirtySpan {
                    row: span.row,
                    left_col: span.left_col,
                    right_col: span.right_col,
                })
                .collect(),
        ),
    }
}

pub(super) fn render_damage(update: tmon::RenderDamageSnapshot) -> TerminalRenderDamageSnapshot {
    TerminalRenderDamageSnapshot {
        damage: damage(update.damage),
        scrolls: update
            .scrolls
            .into_iter()
            .map(|scroll| TerminalViewportScroll {
                top: scroll.top,
                bottom: scroll.bottom,
                count: scroll.count,
                direction: match scroll.direction {
                    tmon::ScrollDirection::Up => TerminalViewportScrollDirection::Up,
                    tmon::ScrollDirection::Down => TerminalViewportScrollDirection::Down,
                },
            })
            .collect(),
        generation: Some(update.generation),
    }
}

pub(super) fn kitty_graphics_placement(
    placement: tmon::GraphicsRenderPlacement,
) -> KittyGraphicsRenderPlacement {
    KittyGraphicsRenderPlacement {
        placement_serial: placement.placement_serial,
        image_id: placement.image_id,
        placement_id: placement.placement_id,
        png: placement.png,
        image_width: placement.image_width,
        image_height: placement.image_height,
        image_generation: placement.image_generation,
        viewport_row: placement.viewport_row,
        col: placement.col,
        source_x: placement.source_x,
        source_y: placement.source_y,
        source_width: placement.source_width,
        source_height: placement.source_height,
        display_cols: placement.display_cols,
        display_rows: placement.display_rows,
        occupied_cols: placement.occupied_cols,
        occupied_rows: placement.occupied_rows,
        x_offset: placement.x_offset,
        y_offset: placement.y_offset,
        z_index: placement.z_index,
    }
}

pub(super) fn mouse_mode(mode: tmon::MouseMode) -> TerminalMouseMode {
    TerminalMouseMode {
        enabled: mode.enabled,
        report_click: mode.report_click,
        report_drag: mode.report_drag,
        report_motion: mode.report_motion,
        sgr_encoding: mode.sgr_encoding,
        utf8_encoding: mode.utf8_encoding,
    }
}

pub(super) fn keyboard_mode(mode: tmon::KeyboardMode) -> TerminalKeyboardMode {
    TerminalKeyboardMode::from_flags(
        mode.application_cursor_keys,
        mode.disambiguate_escape_codes,
        mode.report_event_types,
        mode.report_alternate_keys,
        mode.report_all_keys_as_esc,
        mode.report_associated_text,
    )
}

#[cfg(test)]
#[path = "tmon_adapter/tests/mod.rs"]
mod tests;
