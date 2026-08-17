use super::*;

pub(super) struct BackendSelection {
    pub(super) backend: Backend,
    pub(super) diagnostics: TerminalEngineDiagnostics,
}

impl BackendSelection {
    fn new(backend: Backend, selection_reason: TerminalEngineSelectionReason) -> Self {
        Self {
            backend,
            diagnostics: TerminalEngineDiagnostics {
                engine: "tmon",
                selection_reason,
                fallback_detail: None,
            },
        }
    }

    fn log_native(&self) {
        log::info!(
            "terminal engine selected: {} ({})",
            self.diagnostics.engine,
            self.diagnostics.selection_reason
        );
    }
}

pub(super) struct Backend(Box<super::tmon_backend::TmonBackend>);

impl Backend {
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
        let backend = super::tmon_backend::TmonBackend::new_with_launch_and_wakeup_notifier(
            size,
            configured_working_dir,
            wakeup_notifier,
            tab_title_shell_integration,
            runtime_config,
            launch,
        )?;
        let selection = BackendSelection::new(
            Self(Box::new(backend)),
            TerminalEngineSelectionReason::TmonDefault,
        );
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
        BackendSelection::new(
            Self(Box::new(
                super::tmon_backend::TmonBackend::new_display_with_wakeup_notifier(
                    size,
                    runtime_config,
                    wakeup_notifier,
                ),
            )),
            TerminalEngineSelectionReason::DisplayDefault,
        )
    }

    pub(super) fn feed_output(&self, bytes: &[u8]) {
        self.0.feed_output(bytes);
    }

    pub(super) fn child_pid(&self) -> Option<u32> {
        self.0.child_pid()
    }

    pub(super) fn set_wakeup_enabled(&self, enabled: bool) {
        self.0.set_wakeup_enabled(enabled);
    }

    pub(super) fn write(&self, input: &[u8]) {
        if let Err(error) = self.try_write(input) {
            log::warn!("terminal PTY write failed: {error}");
        }
    }

    pub(super) fn try_write(&self, input: &[u8]) -> io::Result<()> {
        self.0.try_write(input)
    }

    pub(super) fn write_owned(&self, input: Vec<u8>) {
        if let Err(error) = self.try_write_owned(input) {
            log::warn!("terminal PTY write failed: {error}");
        }
    }

    pub(super) fn try_write_owned(&self, input: Vec<u8>) -> io::Result<()> {
        self.0.try_write_owned(input)
    }

    pub(super) fn hydrate_output(&self, bytes: &[u8]) {
        self.0.hydrate_output(bytes);
    }

    pub(super) fn write_str(&self, input: &str) {
        if let Err(error) = self.try_write_str(input) {
            log::warn!("terminal PTY write failed: {error}");
        }
    }

    pub(super) fn try_write_str(&self, input: &str) -> io::Result<()> {
        self.0.try_write_str(input)
    }

    pub(super) fn resize(&mut self, new_size: TerminalSize) {
        if let Err(error) = self.try_resize(new_size) {
            log::warn!("terminal PTY resize failed: {error}");
        }
    }

    pub(super) fn try_resize(&mut self, new_size: TerminalSize) -> io::Result<()> {
        self.0.try_resize(new_size)
    }

    pub(super) fn nudge_resize(&self) {
        if let Err(error) = self.try_nudge_resize() {
            log::warn!("terminal PTY resize nudge failed: {error}");
        }
    }

    pub(super) fn try_nudge_resize(&self) -> io::Result<()> {
        self.0.try_nudge_resize()
    }

    pub(super) fn size(&self) -> TerminalSize {
        self.0.size()
    }

    pub(super) fn kitty_graphics_placements(&self) -> Vec<KittyGraphicsRenderPlacement> {
        self.0.kitty_graphics_placements()
    }

    pub(super) fn kitty_graphics_revision(&self) -> u64 {
        self.0.kitty_graphics_revision()
    }

    pub(super) fn kitty_graphics_snapshot(&self) -> (u64, Vec<KittyGraphicsRenderPlacement>) {
        self.0.kitty_graphics_snapshot()
    }

    pub(super) fn drain_events(
        &self,
        host: &mut impl TerminalReplyHost,
    ) -> (Vec<TerminalEvent>, bool) {
        self.0.drain_events(host)
    }

    pub(super) fn set_query_colors(&mut self, query_colors: TerminalQueryColors) {
        self.0.set_query_colors(query_colors);
    }

    pub(super) fn palette(&self) -> crate::TerminalPalette {
        self.0.palette()
    }

    pub(super) fn snapshot(&self) -> TermyFrame {
        self.0.snapshot()
    }

    pub(super) fn frame_update(&self, force_full: bool) -> TermyFrameUpdate {
        self.0.frame_update(force_full)
    }

    pub(super) fn take_render_damage_snapshot(&self) -> TerminalRenderDamageSnapshot {
        self.0.take_render_damage_snapshot()
    }

    pub(super) fn render_read(&self, force_full: bool) -> TerminalRenderRead {
        self.0.render_read(force_full)
    }

    pub(super) fn visit_viewport_cells(
        &self,
        visitor: impl FnMut(usize, i32, usize, &crate::TerminalRenderCell),
    ) -> TerminalViewportMetadata {
        self.0.visit_viewport_cells(visitor)
    }

    pub(super) fn visit_viewport_ranges_at_generation(
        &self,
        generation: u64,
        spans: &[TerminalDirtySpan],
        visitor: impl FnMut(usize, usize, i32, usize, &crate::TerminalRenderCell),
    ) -> bool {
        self.0
            .visit_viewport_ranges_at_generation(generation, spans, visitor)
    }

    pub(super) fn line_bounds(&self) -> (i32, i32) {
        self.0.line_bounds()
    }

    pub(super) fn visit_line_cells(
        &self,
        requested_first: i32,
        requested_last: i32,
        visitor: impl FnMut((i32, i32, usize), i32, usize, &crate::TerminalRenderCell),
    ) -> (i32, i32, usize) {
        self.0
            .visit_line_cells(requested_first, requested_last, visitor)
    }

    pub(super) fn search(&self, query: &str) -> Vec<TermySearchMatch> {
        self.0.search(query)
    }

    pub(super) fn search_with_options(
        &self,
        query: &str,
        options: TermySearchOptions,
    ) -> Vec<TermySearchMatch> {
        self.0.search_with_options(query, options)
    }

    pub(super) fn search_shared(&self, query: &str) -> Vec<TermySharedSearchMatch> {
        self.0.search_shared(query)
    }

    pub(super) fn search_shared_with_options(
        &self,
        query: &str,
        options: TermySearchOptions,
    ) -> Vec<TermySharedSearchMatch> {
        self.0.search_shared_with_options(query, options)
    }

    pub(super) fn hyperlink_at(
        &self,
        row: usize,
        col: usize,
    ) -> Option<crate::links::DetectedLink> {
        self.0.hyperlink_at(row, col)
    }

    pub(super) fn link_at(
        &self,
        row: usize,
        col: usize,
    ) -> Option<crate::links::DetectedViewportLink> {
        self.0.link_at(row, col)
    }

    pub(super) fn take_damage_snapshot(&self) -> TerminalDamageSnapshot {
        self.0.take_damage_snapshot()
    }

    pub(super) fn scroll_display(&self, delta_lines: i32) -> bool {
        self.0.scroll_display(delta_lines)
    }

    pub(super) fn scroll_to_bottom(&self) -> bool {
        self.0.scroll_to_bottom()
    }

    pub(super) fn clear_scrollback(&self) -> bool {
        self.0.clear_scrollback()
    }

    pub(super) fn scroll_state(&self) -> (usize, usize) {
        self.0.scroll_state()
    }

    pub(super) fn cursor_state(&self) -> Option<TerminalCursorState> {
        self.0.cursor_state()
    }

    pub(super) fn cursor_position(&self) -> (usize, usize) {
        self.0.cursor_position()
    }

    pub(super) fn has_pending_events(&self) -> bool {
        self.0.has_pending_events()
    }

    pub(super) fn set_term_options(&self, options: TerminalOptions) {
        self.0.set_term_options(options);
    }

    pub(super) fn set_scrollback_history(&self, scrollback_history: usize) {
        self.0.set_scrollback_history(scrollback_history);
    }

    pub(super) fn bracketed_paste_mode(&self) -> bool {
        self.0.bracketed_paste_mode()
    }

    pub(super) fn mouse_mode(&self) -> TerminalMouseMode {
        self.0.mouse_mode()
    }

    pub(super) fn keyboard_mode(&self) -> TerminalKeyboardMode {
        self.0.keyboard_mode()
    }

    pub(super) fn alternate_screen_mode(&self) -> bool {
        self.0.alternate_screen_mode()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(any(
        target_os = "linux",
        target_os = "android",
        target_os = "macos",
        target_os = "windows"
    ))]
    #[test]
    fn native_terminal_reports_tmon_as_the_only_engine() {
        let terminal = Terminal::new(
            TerminalSize::default(),
            None,
            None,
            None,
            None,
            Some("exit"),
        )
        .expect("native diagnostic probe should start");

        assert_eq!(terminal.engine_label(), "tmon");
        assert_eq!(
            terminal.engine_diagnostics(),
            &TerminalEngineDiagnostics {
                engine: "tmon",
                selection_reason: TerminalEngineSelectionReason::TmonDefault,
                fallback_detail: None,
            }
        );
    }

    #[cfg(any(
        target_os = "linux",
        target_os = "android",
        target_os = "macos",
        target_os = "windows"
    ))]
    #[test]
    fn invalid_tmon_launch_is_returned_without_fallback() {
        let missing_program = std::env::temp_dir()
            .join(format!("missing-termy-program-{}", std::process::id()))
            .to_string_lossy()
            .into_owned();
        let error = Backend::new_with_launch_and_wakeup_notifier(
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
    }
}
