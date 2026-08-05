mod grid;
mod keyboard;
mod links;
mod locale;
mod mouse_protocol;
mod osc_intercept;
mod pane_terminal;
#[cfg(unix)]
mod path_env;
mod protocol;
mod render_metrics;
mod runtime;
mod shell_integration;
mod tmux;

// Intentionally re-exported for the app renderer adapter boundary. These types are the
// cross-crate contract for row-level paint-cache invalidation between `termy` and this crate.
pub use grid::{
    CellRenderInfo, TerminalGrid, TerminalGridPaintCacheHandle, TerminalGridPaintDamage,
    TerminalGridRow, TerminalGridRows, TerminalUnderline, TerminalUnderlineStyle,
};
pub use keyboard::{TerminalKeyEventKind, TerminalKeyboardMode, keystroke_to_input};
pub use links::{
    DetectedLink, DetectedViewportLink, classify_link_token, find_link_in_line,
    hyperlink_at_viewport_cell, link_at_viewport_cell,
};
pub use mouse_protocol::{
    TerminalMouseButton, TerminalMouseEventKind, TerminalMouseMode, TerminalMouseModifiers,
    TerminalMousePosition, encode_mouse_report,
};
pub use osc_intercept::{OscEvent, OscInterceptor};
pub use pane_terminal::PaneTerminal;
pub use protocol::{TerminalClipboardTarget, TerminalQueryColors, TerminalReplyHost};
pub use render_metrics::{
    TerminalUiRenderMetricsSnapshot, add_span_damage_compute_us,
    terminal_ui_render_metrics_enabled, terminal_ui_render_metrics_reset,
    terminal_ui_render_metrics_snapshot,
};
pub use runtime::{
    MAX_TERMINAL_SCROLLBACK_HISTORY, ResolvedTerminalLaunch, TabTitleShellIntegration, Terminal,
    TerminalCursorState, TerminalCursorStyle, TerminalDamageSnapshot, TerminalDirtySpan,
    TerminalEvent, TerminalLaunch, TerminalOptions, TerminalRuntimeConfig, TerminalSize,
    TerminalWakeupNotifier, WindowsShell, WorkingDirFallback,
    normalize_working_directory_candidate, resolve_launch_working_directory,
    resolve_terminal_launch, resolve_working_directory_path, terminal_environment_overrides,
};
pub use shell_integration::{CommandLifecycle, CommandPhase, ProgressState};
pub use termy_core::KittyGraphicsRenderPlacement;
pub use termy_core::monotonic_now_ns;
pub use tmux::{
    TmuxClient, TmuxLaunchTarget, TmuxNotification, TmuxPaneMouseMode, TmuxPaneState,
    TmuxRuntimeConfig, TmuxSessionSummary, TmuxShutdownMode, TmuxSnapshot, TmuxSocketTarget,
    TmuxWindowState,
};
