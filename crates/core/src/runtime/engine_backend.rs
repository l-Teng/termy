use super::*;
use anyhow::Context as _;

const TEST_BACKEND_ENV: &str = "TERMY_CORE_TEST_BACKEND";
const FORCE_ALACRITTY_ENV: &str = "TERMY_FORCE_ALACRITTY_ENGINE";

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum BackendChoice {
    #[default]
    Alacritty,
    Tmon,
}

impl BackendChoice {
    fn from_test_value(value: Option<&std::ffi::OsStr>) -> Option<Self> {
        match value.and_then(std::ffi::OsStr::to_str) {
            Some("alacritty") => Some(Self::Alacritty),
            Some("tmon") => Some(Self::Tmon),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum NativeBackendRequest {
    TestOverride(BackendChoice),
    ForcedAlacritty,
    PreferTmon,
}

impl NativeBackendRequest {
    fn current() -> Self {
        Self::from_values(
            env::var_os(TEST_BACKEND_ENV).as_deref(),
            env::var_os(FORCE_ALACRITTY_ENV).as_deref(),
        )
    }

    fn from_values(
        test_value: Option<&std::ffi::OsStr>,
        force_alacritty_value: Option<&std::ffi::OsStr>,
    ) -> Self {
        if let Some(choice) = BackendChoice::from_test_value(test_value) {
            return Self::TestOverride(choice);
        }
        if force_alacritty_value == Some(std::ffi::OsStr::new("1")) {
            Self::ForcedAlacritty
        } else {
            Self::PreferTmon
        }
    }
}

fn tmon_failure_is_fallback_eligible(error: &anyhow::Error) -> bool {
    error
        .downcast_ref::<tmon::Error>()
        .is_some_and(tmon::Error::is_backend_initialization_failure)
}

fn select_native_backend<T>(
    request: NativeBackendRequest,
    tmon_available: bool,
    mut start_alacritty: impl FnMut() -> anyhow::Result<T>,
    mut start_tmon: impl FnMut() -> anyhow::Result<T>,
    tmon_failure_is_eligible: impl Fn(&anyhow::Error) -> bool,
) -> anyhow::Result<(T, TerminalEngineSelectionReason, Option<String>)> {
    match request {
        NativeBackendRequest::TestOverride(BackendChoice::Alacritty) => Ok((
            start_alacritty()?,
            TerminalEngineSelectionReason::TestOverride,
            None,
        )),
        NativeBackendRequest::TestOverride(BackendChoice::Tmon) => Ok((
            start_tmon()?,
            TerminalEngineSelectionReason::TestOverride,
            None,
        )),
        NativeBackendRequest::ForcedAlacritty => Ok((
            start_alacritty()?,
            TerminalEngineSelectionReason::ForcedAlacritty,
            None,
        )),
        NativeBackendRequest::PreferTmon if !tmon_available => Ok((
            start_alacritty()?,
            TerminalEngineSelectionReason::TmonUnavailable,
            Some("native Tmon PTY support is unavailable on this host".to_string()),
        )),
        NativeBackendRequest::PreferTmon => match start_tmon() {
            Ok(backend) => Ok((backend, TerminalEngineSelectionReason::TmonDefault, None)),
            Err(error) if tmon_failure_is_eligible(&error) => {
                let detail = error.to_string();
                let backend = start_alacritty().with_context(|| {
                    format!(
                        "Tmon backend initialization failed ({detail}); Alacritty fallback also failed"
                    )
                })?;
                Ok((
                    backend,
                    TerminalEngineSelectionReason::TmonInitializationFailure,
                    Some(detail),
                ))
            }
            Err(error) => Err(error),
        },
    }
}

fn select_display_backend<T>(
    request: NativeBackendRequest,
    mut start_alacritty: impl FnMut() -> T,
    mut start_tmon: impl FnMut() -> T,
) -> (T, TerminalEngineSelectionReason) {
    match request {
        NativeBackendRequest::TestOverride(BackendChoice::Alacritty) => (
            start_alacritty(),
            TerminalEngineSelectionReason::TestOverride,
        ),
        NativeBackendRequest::TestOverride(BackendChoice::Tmon) => {
            (start_tmon(), TerminalEngineSelectionReason::TestOverride)
        }
        NativeBackendRequest::ForcedAlacritty => (
            start_alacritty(),
            TerminalEngineSelectionReason::ForcedAlacritty,
        ),
        NativeBackendRequest::PreferTmon => {
            (start_tmon(), TerminalEngineSelectionReason::DisplayDefault)
        }
    }
}

pub(super) struct BackendSelection {
    pub(super) backend: Backend,
    pub(super) diagnostics: TerminalEngineDiagnostics,
}

impl BackendSelection {
    fn new(
        backend: Backend,
        selection_reason: TerminalEngineSelectionReason,
        fallback_detail: Option<String>,
    ) -> Self {
        let diagnostics = TerminalEngineDiagnostics {
            engine: backend.engine_label(),
            selection_reason,
            fallback_detail,
        };
        Self {
            backend,
            diagnostics,
        }
    }

    fn log_native(&self) {
        let diagnostics = &self.diagnostics;
        match diagnostics.selection_reason {
            TerminalEngineSelectionReason::TmonUnavailable
            | TerminalEngineSelectionReason::TmonInitializationFailure => log::warn!(
                "terminal engine selected: {} ({}){}",
                diagnostics.engine,
                diagnostics.selection_reason,
                diagnostics
                    .fallback_detail
                    .as_deref()
                    .map_or_else(String::new, |detail| format!("; {detail}"))
            ),
            TerminalEngineSelectionReason::TestOverride => log::debug!(
                "terminal engine selected: {} ({})",
                diagnostics.engine,
                diagnostics.selection_reason
            ),
            _ => log::info!(
                "terminal engine selected: {} ({})",
                diagnostics.engine,
                diagnostics.selection_reason
            ),
        }
    }
}

pub(super) enum Backend {
    Alacritty(Box<super::alacritty_backend::AlacrittyBackend>),
    Tmon(Box<super::tmon_backend::TmonBackend>),
}

impl Backend {
    pub(super) fn engine_label(&self) -> &'static str {
        match self {
            Self::Alacritty(_) => "alacritty",
            Self::Tmon(_) => "tmon",
        }
    }

    pub(super) fn select_native(
        size: TerminalSize,
        configured_working_dir: Option<&str>,
        event_wakeup_tx: Option<Sender<()>>,
        tab_title_shell_integration: Option<&TabTitleShellIntegration>,
        runtime_config: Option<&TerminalRuntimeConfig>,
        startup_command: Option<&str>,
    ) -> anyhow::Result<BackendSelection> {
        let wakeup_notifier = event_wakeup_tx.map(|event_wakeup_tx| {
            TerminalWakeupNotifier::new(move || {
                let _ = event_wakeup_tx.try_send(());
            })
        });
        Self::new_with_wakeup_notifier(
            size,
            configured_working_dir,
            wakeup_notifier,
            tab_title_shell_integration,
            runtime_config,
            startup_command,
        )
    }

    pub(super) fn new_with_wakeup_notifier(
        size: TerminalSize,
        configured_working_dir: Option<&str>,
        wakeup_notifier: Option<TerminalWakeupNotifier>,
        tab_title_shell_integration: Option<&TabTitleShellIntegration>,
        runtime_config: Option<&TerminalRuntimeConfig>,
        startup_command: Option<&str>,
    ) -> anyhow::Result<BackendSelection> {
        let launch =
            startup_command.map(|command| TerminalLaunch::ShellCommand(command.to_string()));
        Self::new_with_launch_and_wakeup_notifier(
            size,
            configured_working_dir,
            wakeup_notifier,
            tab_title_shell_integration,
            runtime_config,
            launch.as_ref(),
        )
    }

    pub(super) fn new_with_launch_and_wakeup_notifier(
        size: TerminalSize,
        configured_working_dir: Option<&str>,
        wakeup_notifier: Option<TerminalWakeupNotifier>,
        tab_title_shell_integration: Option<&TabTitleShellIntegration>,
        runtime_config: Option<&TerminalRuntimeConfig>,
        launch: Option<&TerminalLaunch>,
    ) -> anyhow::Result<BackendSelection> {
        let start_alacritty = || {
            super::alacritty_backend::AlacrittyBackend::new_with_launch_and_wakeup_notifier(
                size,
                configured_working_dir,
                wakeup_notifier.clone(),
                tab_title_shell_integration,
                runtime_config,
                launch,
            )
            .map(Box::new)
            .map(Self::Alacritty)
        };
        let start_tmon = || {
            super::tmon_backend::TmonBackend::new_with_launch_and_wakeup_notifier(
                size,
                configured_working_dir,
                wakeup_notifier.clone(),
                tab_title_shell_integration,
                runtime_config,
                launch,
            )
            .map(Box::new)
            .map(Self::Tmon)
        };

        let (backend, reason, detail) = select_native_backend(
            NativeBackendRequest::current(),
            tmon::native_pty_available(),
            start_alacritty,
            start_tmon,
            tmon_failure_is_fallback_eligible,
        )?;
        let selection = BackendSelection::new(backend, reason, detail);
        selection.log_native();
        Ok(selection)
    }

    pub(super) fn new_display(
        size: TerminalSize,
        runtime_config: Option<&TerminalRuntimeConfig>,
    ) -> BackendSelection {
        Self::new_display_with_wakeup_notifier(size, runtime_config, None)
    }

    pub(super) fn new_display_with_wakeup_notifier(
        size: TerminalSize,
        runtime_config: Option<&TerminalRuntimeConfig>,
        wakeup_notifier: Option<TerminalWakeupNotifier>,
    ) -> BackendSelection {
        let (backend, reason) = select_display_backend(
            NativeBackendRequest::current(),
            || {
                Self::Alacritty(Box::new(
                    super::alacritty_backend::AlacrittyBackend::new_display_with_wakeup_notifier(
                        size,
                        runtime_config,
                        wakeup_notifier.clone(),
                    ),
                ))
            },
            || {
                Self::Tmon(Box::new(
                    super::tmon_backend::TmonBackend::new_display_with_wakeup_notifier(
                        size,
                        runtime_config,
                        wakeup_notifier.clone(),
                    ),
                ))
            },
        );
        BackendSelection::new(backend, reason, None)
    }

    #[cfg(test)]
    pub(super) fn new_alacritty_display_for_test(
        size: TerminalSize,
        runtime_config: Option<&TerminalRuntimeConfig>,
    ) -> BackendSelection {
        BackendSelection::new(
            Self::Alacritty(Box::new(
                super::alacritty_backend::AlacrittyBackend::new_display(size, runtime_config),
            )),
            TerminalEngineSelectionReason::TestOverride,
            None,
        )
    }

    pub(super) fn feed_output(&self, bytes: &[u8]) {
        match self {
            Self::Alacritty(backend) => backend.feed_output(bytes),
            Self::Tmon(backend) => backend.feed_output(bytes),
        }
    }

    pub(super) fn child_pid(&self) -> Option<u32> {
        match self {
            Self::Alacritty(backend) => backend.child_pid(),
            Self::Tmon(backend) => backend.child_pid(),
        }
    }

    pub(super) fn set_wakeup_enabled(&self, enabled: bool) {
        match self {
            Self::Alacritty(backend) => backend.set_wakeup_enabled(enabled),
            Self::Tmon(backend) => backend.set_wakeup_enabled(enabled),
        }
    }

    pub(super) fn write(&self, input: &[u8]) {
        match self {
            Self::Alacritty(backend) => backend.write(input),
            Self::Tmon(backend) => backend.write(input),
        }
    }

    pub(super) fn write_owned(&self, input: Vec<u8>) {
        match self {
            Self::Alacritty(backend) => backend.write_owned(input),
            Self::Tmon(backend) => backend.write_owned(input),
        }
    }

    pub(super) fn hydrate_output(&self, bytes: &[u8]) {
        match self {
            Self::Alacritty(backend) => backend.hydrate_output(bytes),
            Self::Tmon(backend) => backend.hydrate_output(bytes),
        }
    }

    pub(super) fn write_str(&self, input: &str) {
        match self {
            Self::Alacritty(backend) => backend.write_str(input),
            Self::Tmon(backend) => backend.write_str(input),
        }
    }

    pub(super) fn resize(&mut self, new_size: TerminalSize) {
        match self {
            Self::Alacritty(backend) => backend.resize(new_size),
            Self::Tmon(backend) => backend.resize(new_size),
        }
    }

    pub(super) fn nudge_resize(&self) {
        match self {
            Self::Alacritty(backend) => backend.nudge_resize(),
            Self::Tmon(backend) => backend.nudge_resize(),
        }
    }

    pub(super) fn size(&self) -> TerminalSize {
        match self {
            Self::Alacritty(backend) => backend.size(),
            Self::Tmon(backend) => backend.size(),
        }
    }

    pub(super) fn kitty_graphics_placements(&self) -> Vec<KittyGraphicsRenderPlacement> {
        match self {
            Self::Alacritty(backend) => backend.kitty_graphics_placements(),
            Self::Tmon(backend) => backend.kitty_graphics_placements(),
        }
    }

    pub(super) fn kitty_graphics_revision(&self) -> u64 {
        match self {
            Self::Alacritty(backend) => backend.kitty_graphics_revision(),
            Self::Tmon(backend) => backend.kitty_graphics_revision(),
        }
    }

    pub(super) fn kitty_graphics_snapshot(&self) -> (u64, Vec<KittyGraphicsRenderPlacement>) {
        match self {
            Self::Alacritty(backend) => backend.kitty_graphics_snapshot(),
            Self::Tmon(backend) => backend.kitty_graphics_snapshot(),
        }
    }

    pub(super) fn drain_events(
        &self,
        host: &mut impl TerminalReplyHost,
    ) -> (Vec<TerminalEvent>, bool) {
        match self {
            Self::Alacritty(backend) => backend.drain_events(host),
            Self::Tmon(backend) => backend.drain_events(host),
        }
    }

    pub(super) fn set_query_colors(&mut self, query_colors: TerminalQueryColors) {
        match self {
            Self::Alacritty(backend) => backend.set_query_colors(query_colors),
            Self::Tmon(backend) => backend.set_query_colors(query_colors),
        }
    }

    pub(super) fn palette(&self) -> crate::TerminalPalette {
        match self {
            Self::Alacritty(backend) => backend.palette(),
            Self::Tmon(backend) => backend.palette(),
        }
    }

    pub(super) fn snapshot(&self) -> TermyFrame {
        match self {
            Self::Alacritty(backend) => backend.snapshot(),
            Self::Tmon(backend) => backend.snapshot(),
        }
    }

    pub(super) fn frame_update(&self, force_full: bool) -> TermyFrameUpdate {
        match self {
            Self::Alacritty(backend) => backend.frame_update(force_full),
            Self::Tmon(backend) => backend.frame_update(force_full),
        }
    }

    pub(super) fn take_render_damage_snapshot(&self) -> TerminalRenderDamageSnapshot {
        match self {
            Self::Alacritty(backend) => backend.take_render_damage_snapshot(),
            Self::Tmon(backend) => backend.take_render_damage_snapshot(),
        }
    }

    pub(super) fn render_read(&self, force_full: bool) -> TerminalRenderRead {
        match self {
            Self::Alacritty(backend) => backend.render_read(force_full),
            Self::Tmon(backend) => backend.render_read(force_full),
        }
    }

    pub(super) fn visit_viewport_cells(
        &self,
        visitor: impl FnMut(usize, i32, usize, &crate::TerminalRenderCell),
    ) -> TerminalViewportMetadata {
        match self {
            Self::Alacritty(backend) => backend.visit_viewport_cells(visitor),
            Self::Tmon(backend) => backend.visit_viewport_cells(visitor),
        }
    }

    pub(super) fn visit_viewport_ranges_at_generation(
        &self,
        generation: u64,
        spans: &[TerminalDirtySpan],
        visitor: impl FnMut(usize, usize, i32, usize, &crate::TerminalRenderCell),
    ) -> bool {
        match self {
            Self::Alacritty(backend) => {
                backend.visit_viewport_ranges_at_generation(generation, spans, visitor)
            }
            Self::Tmon(backend) => {
                backend.visit_viewport_ranges_at_generation(generation, spans, visitor)
            }
        }
    }

    pub(super) fn line_bounds(&self) -> (i32, i32) {
        match self {
            Self::Alacritty(backend) => backend.line_bounds(),
            Self::Tmon(backend) => backend.line_bounds(),
        }
    }

    pub(super) fn visit_line_cells(
        &self,
        requested_first: i32,
        requested_last: i32,
        visitor: impl FnMut((i32, i32, usize), i32, usize, &crate::TerminalRenderCell),
    ) -> (i32, i32, usize) {
        match self {
            Self::Alacritty(backend) => {
                backend.visit_line_cells(requested_first, requested_last, visitor)
            }
            Self::Tmon(backend) => {
                backend.visit_line_cells(requested_first, requested_last, visitor)
            }
        }
    }

    pub(super) fn search(&self, query: &str) -> Vec<TermySearchMatch> {
        match self {
            Self::Alacritty(backend) => backend.search(query),
            Self::Tmon(backend) => backend.search(query),
        }
    }

    pub(super) fn search_with_options(
        &self,
        query: &str,
        options: TermySearchOptions,
    ) -> Vec<TermySearchMatch> {
        match self {
            Self::Alacritty(backend) => backend.search_with_options(query, options),
            Self::Tmon(backend) => backend.search_with_options(query, options),
        }
    }

    pub(super) fn search_shared(&self, query: &str) -> Vec<TermySharedSearchMatch> {
        match self {
            Self::Alacritty(backend) => backend.search_shared(query),
            Self::Tmon(backend) => backend.search_shared(query),
        }
    }

    pub(super) fn search_shared_with_options(
        &self,
        query: &str,
        options: TermySearchOptions,
    ) -> Vec<TermySharedSearchMatch> {
        match self {
            Self::Alacritty(backend) => backend.search_shared_with_options(query, options),
            Self::Tmon(backend) => backend.search_shared_with_options(query, options),
        }
    }

    pub(super) fn hyperlink_at(
        &self,
        row: usize,
        col: usize,
    ) -> Option<crate::links::DetectedLink> {
        match self {
            Self::Alacritty(backend) => backend.hyperlink_at(row, col),
            Self::Tmon(backend) => backend.hyperlink_at(row, col),
        }
    }

    pub(super) fn link_at(
        &self,
        row: usize,
        col: usize,
    ) -> Option<crate::links::DetectedViewportLink> {
        match self {
            Self::Alacritty(backend) => backend.link_at(row, col),
            Self::Tmon(backend) => backend.link_at(row, col),
        }
    }

    pub(super) fn take_damage_snapshot(&self) -> TerminalDamageSnapshot {
        match self {
            Self::Alacritty(backend) => backend.take_damage_snapshot(),
            Self::Tmon(backend) => backend.take_damage_snapshot(),
        }
    }

    pub(super) fn scroll_display(&self, delta_lines: i32) -> bool {
        match self {
            Self::Alacritty(backend) => backend.scroll_display(delta_lines),
            Self::Tmon(backend) => backend.scroll_display(delta_lines),
        }
    }

    pub(super) fn scroll_to_bottom(&self) -> bool {
        match self {
            Self::Alacritty(backend) => backend.scroll_to_bottom(),
            Self::Tmon(backend) => backend.scroll_to_bottom(),
        }
    }

    pub(super) fn clear_scrollback(&self) -> bool {
        match self {
            Self::Alacritty(backend) => backend.clear_scrollback(),
            Self::Tmon(backend) => backend.clear_scrollback(),
        }
    }

    pub(super) fn scroll_state(&self) -> (usize, usize) {
        match self {
            Self::Alacritty(backend) => backend.scroll_state(),
            Self::Tmon(backend) => backend.scroll_state(),
        }
    }

    pub(super) fn cursor_state(&self) -> Option<TerminalCursorState> {
        match self {
            Self::Alacritty(backend) => backend.cursor_state(),
            Self::Tmon(backend) => backend.cursor_state(),
        }
    }

    pub(super) fn cursor_position(&self) -> (usize, usize) {
        match self {
            Self::Alacritty(backend) => backend.cursor_position(),
            Self::Tmon(backend) => backend.cursor_position(),
        }
    }

    pub(super) fn has_pending_events(&self) -> bool {
        match self {
            Self::Alacritty(backend) => backend.has_pending_events(),
            Self::Tmon(backend) => backend.has_pending_events(),
        }
    }

    pub(super) fn set_term_options(&self, options: TerminalOptions) {
        match self {
            Self::Alacritty(backend) => backend.set_term_options(options),
            Self::Tmon(backend) => backend.set_term_options(options),
        }
    }

    pub(super) fn set_scrollback_history(&self, scrollback_history: usize) {
        match self {
            Self::Alacritty(backend) => backend.set_scrollback_history(scrollback_history),
            Self::Tmon(backend) => backend.set_scrollback_history(scrollback_history),
        }
    }

    pub(super) fn bracketed_paste_mode(&self) -> bool {
        match self {
            Self::Alacritty(backend) => backend.bracketed_paste_mode(),
            Self::Tmon(backend) => backend.bracketed_paste_mode(),
        }
    }

    pub(super) fn mouse_mode(&self) -> TerminalMouseMode {
        match self {
            Self::Alacritty(backend) => backend.mouse_mode(),
            Self::Tmon(backend) => backend.mouse_mode(),
        }
    }

    pub(super) fn keyboard_mode(&self) -> TerminalKeyboardMode {
        match self {
            Self::Alacritty(backend) => backend.keyboard_mode(),
            Self::Tmon(backend) => backend.keyboard_mode(),
        }
    }

    pub(super) fn alternate_screen_mode(&self) -> bool {
        match self {
            Self::Alacritty(backend) => backend.alternate_screen_mode(),
            Self::Tmon(backend) => backend.alternate_screen_mode(),
        }
    }

    #[cfg(test)]
    pub(super) fn send_wakeup_for_test(&self) {
        match self {
            Self::Alacritty(backend) => backend.send_wakeup_for_test(),
            Self::Tmon(_) => panic!("Alacritty wakeup test used a Tmon backend"),
        }
    }

    #[cfg(test)]
    pub(super) fn try_recv_event_for_test(&self) -> Option<RuntimeEvent> {
        match self {
            Self::Alacritty(backend) => backend.try_recv_event_for_test(),
            Self::Tmon(_) => panic!("Alacritty wakeup test used a Tmon backend"),
        }
    }

    #[cfg(test)]
    pub(super) fn event_queue_is_empty_for_test(&self) -> bool {
        match self {
            Self::Alacritty(backend) => backend.event_queue_is_empty_for_test(),
            Self::Tmon(_) => panic!("Alacritty wakeup test used a Tmon backend"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backend_selector_only_accepts_exact_private_values() {
        assert_eq!(BackendChoice::from_test_value(None), None);
        assert_eq!(
            BackendChoice::from_test_value(Some(std::ffi::OsStr::new("alacritty"))),
            Some(BackendChoice::Alacritty)
        );
        assert_eq!(
            BackendChoice::from_test_value(Some(std::ffi::OsStr::new("tmon"))),
            Some(BackendChoice::Tmon)
        );
        assert_eq!(
            BackendChoice::from_test_value(Some(std::ffi::OsStr::new("1"))),
            None
        );
        assert_eq!(
            BackendChoice::from_test_value(Some(std::ffi::OsStr::new("TMON"))),
            None
        );
    }

    #[test]
    fn native_selector_defaults_to_tmon_and_requires_exact_force_value() {
        assert_eq!(
            NativeBackendRequest::from_values(None, None),
            NativeBackendRequest::PreferTmon
        );
        for value in [
            Some(std::ffi::OsStr::new("0")),
            Some(std::ffi::OsStr::new("true")),
        ] {
            assert_eq!(
                NativeBackendRequest::from_values(None, value),
                NativeBackendRequest::PreferTmon
            );
        }
        assert_eq!(
            NativeBackendRequest::from_values(None, Some(std::ffi::OsStr::new("1"))),
            NativeBackendRequest::ForcedAlacritty
        );
    }

    #[test]
    fn private_test_override_precedes_emergency_force_value() {
        assert_eq!(
            NativeBackendRequest::from_values(
                Some(std::ffi::OsStr::new("tmon")),
                Some(std::ffi::OsStr::new("1")),
            ),
            NativeBackendRequest::TestOverride(BackendChoice::Tmon)
        );
        assert_eq!(
            NativeBackendRequest::from_values(Some(std::ffi::OsStr::new("alacritty")), None,),
            NativeBackendRequest::TestOverride(BackendChoice::Alacritty)
        );
    }

    #[test]
    fn display_selector_honors_exact_force_and_private_override_precedence() {
        let (backend, reason) = select_display_backend(
            NativeBackendRequest::from_values(None, Some(std::ffi::OsStr::new("1"))),
            || "alacritty",
            || "tmon",
        );
        assert_eq!(backend, "alacritty");
        assert_eq!(reason, TerminalEngineSelectionReason::ForcedAlacritty);

        let (backend, reason) = select_display_backend(
            NativeBackendRequest::from_values(
                Some(std::ffi::OsStr::new("tmon")),
                Some(std::ffi::OsStr::new("1")),
            ),
            || "alacritty",
            || "tmon",
        );
        assert_eq!(backend, "tmon");
        assert_eq!(reason, TerminalEngineSelectionReason::TestOverride);
    }

    #[test]
    fn eligible_tmon_initialization_failure_uses_alacritty_with_reason() {
        let alacritty_starts = std::cell::Cell::new(0);
        let (backend, reason, detail) = select_native_backend(
            NativeBackendRequest::PreferTmon,
            true,
            || {
                alacritty_starts.set(alacritty_starts.get() + 1);
                Ok("alacritty")
            },
            || Err(anyhow::anyhow!("injected Tmon initialization failure")),
            |_| true,
        )
        .expect("eligible initialization failure should fall back");

        assert_eq!(backend, "alacritty");
        assert_eq!(
            reason,
            TerminalEngineSelectionReason::TmonInitializationFailure
        );
        assert_eq!(
            detail.as_deref(),
            Some("injected Tmon initialization failure")
        );
        assert_eq!(alacritty_starts.get(), 1);
    }

    #[test]
    fn ineligible_tmon_failure_returns_without_starting_alacritty() {
        let alacritty_starts = std::cell::Cell::new(0);
        let error = select_native_backend(
            NativeBackendRequest::PreferTmon,
            true,
            || {
                alacritty_starts.set(alacritty_starts.get() + 1);
                Ok("alacritty")
            },
            || Err(anyhow::anyhow!("invalid launch")),
            |_| false,
        )
        .expect_err("invalid launch must stay visible");

        assert_eq!(error.to_string(), "invalid launch");
        assert_eq!(alacritty_starts.get(), 0);
    }

    #[test]
    fn unavailable_tmon_uses_structured_fallback_without_starting_tmon() {
        let tmon_starts = std::cell::Cell::new(0);
        let (backend, reason, detail) = select_native_backend(
            NativeBackendRequest::PreferTmon,
            false,
            || Ok("alacritty"),
            || {
                tmon_starts.set(tmon_starts.get() + 1);
                Ok("tmon")
            },
            |_| true,
        )
        .expect("unavailable Tmon should use the retained backend");

        assert_eq!(backend, "alacritty");
        assert_eq!(reason, TerminalEngineSelectionReason::TmonUnavailable);
        assert!(detail.is_some());
        assert_eq!(tmon_starts.get(), 0);
    }

    #[cfg(any(
        target_os = "linux",
        target_os = "android",
        target_os = "macos",
        target_os = "windows"
    ))]
    #[test]
    fn native_terminal_reports_the_actual_selected_engine_and_reason() {
        let terminal = Terminal::new(
            TerminalSize::default(),
            None,
            None,
            None,
            None,
            Some("exit"),
        )
        .expect("native diagnostic probe should start");
        let diagnostics = terminal.engine_diagnostics();
        let test_override =
            BackendChoice::from_test_value(env::var_os(TEST_BACKEND_ENV).as_deref());

        match test_override {
            Some(BackendChoice::Alacritty) => {
                assert_eq!(diagnostics.engine, "alacritty");
                assert_eq!(
                    diagnostics.selection_reason,
                    TerminalEngineSelectionReason::TestOverride
                );
            }
            Some(BackendChoice::Tmon) => {
                assert_eq!(diagnostics.engine, "tmon");
                assert_eq!(
                    diagnostics.selection_reason,
                    TerminalEngineSelectionReason::TestOverride
                );
            }
            None if env::var_os(FORCE_ALACRITTY_ENV).as_deref()
                == Some(std::ffi::OsStr::new("1")) =>
            {
                assert_eq!(diagnostics.engine, "alacritty");
                assert_eq!(
                    diagnostics.selection_reason,
                    TerminalEngineSelectionReason::ForcedAlacritty
                );
            }
            None if tmon::native_pty_available() => {
                assert_eq!(diagnostics.engine, "tmon");
                assert_eq!(
                    diagnostics.selection_reason,
                    TerminalEngineSelectionReason::TmonDefault
                );
            }
            None => {
                assert_eq!(diagnostics.engine, "alacritty");
                assert_eq!(
                    diagnostics.selection_reason,
                    TerminalEngineSelectionReason::TmonUnavailable
                );
            }
        }
    }

    #[cfg(any(
        target_os = "linux",
        target_os = "android",
        target_os = "macos",
        target_os = "windows"
    ))]
    #[test]
    fn invalid_tmon_launch_is_not_eligible_for_alacritty_fallback() {
        let missing_program = std::env::temp_dir()
            .join(format!("missing-termy-program-{}", std::process::id()))
            .to_string_lossy()
            .into_owned();
        let error = super::super::tmon_backend::TmonBackend::new_with_launch_and_wakeup_notifier(
            TerminalSize::default(),
            None,
            None,
            None,
            None,
            Some(&TerminalLaunch::Program {
                program: missing_program,
                args: Vec::new(),
            }),
        )
        .err()
        .expect("a missing program should fail before a terminal starts");

        assert!(error.downcast_ref::<tmon::Error>().is_some());
        assert!(!tmon_failure_is_fallback_eligible(&error));
    }
}
