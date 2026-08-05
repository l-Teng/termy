use super::*;
use std::ops::Range;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SearchKeyAction {
    Close,
    Next,
    Previous,
}

fn search_key_action(key: &str, shift_pressed: bool) -> Option<SearchKeyAction> {
    match key {
        "escape" => Some(SearchKeyAction::Close),
        "enter" if shift_pressed => Some(SearchKeyAction::Previous),
        "enter" => Some(SearchKeyAction::Next),
        _ => None,
    }
}

fn search_counter_label(current: usize, total: usize, query_is_empty: bool) -> Option<String> {
    if total > 0 {
        Some(format!("{current}/{total}"))
    } else if query_is_empty {
        None
    } else {
        Some("0/0".to_string())
    }
}

impl TerminalView {
    fn normalize_search_prefill(text: &str) -> String {
        let mut normalized = String::with_capacity(text.len());
        let mut last_was_line_break = false;
        for character in text.chars() {
            if matches!(character, '\n' | '\r') {
                if !last_was_line_break {
                    normalized.push(' ');
                }
                last_was_line_break = true;
            } else {
                normalized.push(character);
                last_was_line_break = false;
            }
        }
        normalized.trim().to_string()
    }

    fn search_prefill_from_selection(selection: Option<String>) -> Option<String> {
        selection
            .map(|text| Self::normalize_search_prefill(&text))
            .filter(|text| !text.is_empty())
    }

    pub(super) fn execute_search_command_action(
        &mut self,
        action: CommandAction,
        cx: &mut Context<Self>,
    ) -> bool {
        match action {
            CommandAction::OpenSearch => {
                self.open_search(cx);
                true
            }
            CommandAction::CloseSearch => {
                self.close_search(cx);
                true
            }
            CommandAction::SearchNext => {
                self.search_next(cx);
                true
            }
            CommandAction::SearchPrevious => {
                self.search_previous(cx);
                true
            }
            CommandAction::ToggleSearchCaseSensitive => {
                self.search_state.toggle_case_sensitive();
                self.perform_search();
                cx.notify();
                true
            }
            CommandAction::ToggleSearchRegex => {
                self.search_state.toggle_regex_mode();
                self.perform_search();
                cx.notify();
                true
            }
            _ => false,
        }
    }

    pub(super) fn open_search(&mut self, cx: &mut Context<Self>) {
        let prefill = Self::search_prefill_from_selection(self.selected_text());

        if self.search_open {
            if let Some(prefill) = prefill {
                self.search_input.set_text(prefill);
                self.perform_search();
                self.reset_cursor_blink_phase();
                cx.notify();
            }
            return;
        }

        let _ = self.close_terminal_context_menu(cx);

        // Close other overlays
        if self.is_command_palette_open() {
            self.close_command_palette(cx);
        }
        if self.renaming_tab.is_some() {
            self.cancel_rename_tab(cx);
        }
        if self.renaming_workspace.is_some() {
            self.cancel_rename_workspace(cx);
        }

        self.search_open = true;
        self.search_state.open();
        if let Some(prefill) = prefill {
            self.search_input.set_text(prefill);
            self.perform_search();
        } else {
            self.search_input.clear();
            self.clear_terminal_scrollbar_marker_cache();
        }
        self.reset_cursor_blink_phase();
        cx.notify();
    }

    pub(super) fn close_search(&mut self, cx: &mut Context<Self>) {
        if !self.search_open {
            return;
        }

        self.search_open = false;
        self.search_state.close();
        self.search_input.clear();
        self.clear_terminal_scrollbar_marker_cache();
        cx.notify();
    }

    /// Navigate to the next match in the result list. Since results are ordered
    /// newest-first (bottom of terminal = index 0), this moves toward older content.
    pub(super) fn search_next(&mut self, cx: &mut Context<Self>) {
        if !self.search_open || self.search_state.results().is_empty() {
            return;
        }

        self.search_state.next_match();
        self.scroll_to_current_match(cx);
        cx.notify();
    }

    /// Navigate to the previous match in the result list. Since results are ordered
    /// newest-first (bottom of terminal = index 0), this moves toward newer content.
    pub(super) fn search_previous(&mut self, cx: &mut Context<Self>) {
        if !self.search_open || self.search_state.results().is_empty() {
            return;
        }

        self.search_state.previous_match();
        self.scroll_to_current_match(cx);
        cx.notify();
    }

    fn scroll_to_current_match(&mut self, cx: &mut Context<Self>) {
        let Some(current) = self.search_state.results().current() else {
            return;
        };

        let Some(terminal) = self.active_terminal() else {
            return;
        };
        let size = terminal.size();
        let rows = size.rows as i32;

        let (display_offset, history_size) = terminal.scroll_state();

        // `current.line` uses Alacritty coordinates: negative values are scrollback.
        let viewport_row = current.line + display_offset as i32;

        if viewport_row >= 0 && viewport_row < rows {
            return;
        }

        let target_offset = if current.line < 0 {
            (-current.line) as usize
        } else {
            0
        };

        let target_offset = target_offset.min(history_size);
        let delta = target_offset as i32 - display_offset as i32;

        if delta != 0 {
            terminal.scroll_display(delta);
            self.sync_content_scroll_baseline();
            self.mark_terminal_scrollbar_activity(cx);
        }
    }

    pub(super) fn perform_search(&mut self) {
        self.search_state.set_query(self.search_input.text());

        if !self.search_state.has_valid_pattern() {
            self.search_state.clear_results_preserving_query();
            self.clear_terminal_scrollbar_marker_cache();
            return;
        }

        let Some(terminal) = self.active_terminal() else {
            self.search_state.clear_results_preserving_query();
            self.clear_terminal_scrollbar_marker_cache();
            return;
        };
        let line_texts = collect_search_line_texts(terminal, i32::MIN, i32::MAX);
        let start_line = line_texts.first_line;
        let end_line = line_texts.last_line();

        let search_state = &mut self.search_state;
        search_state.search(start_line, end_line, |line_idx| line_texts.line(line_idx));

        // Start from the bottommost (newest) match, which is now index 0.
        self.search_state.jump_to_first();
        if self.search_state.results().is_empty() {
            self.clear_terminal_scrollbar_marker_cache();
        }
    }

    pub(super) fn handle_search_key_down(
        &mut self,
        key: &str,
        shift_pressed: bool,
        cx: &mut Context<Self>,
    ) -> bool {
        match search_key_action(key, shift_pressed) {
            Some(SearchKeyAction::Close) => {
                self.close_search(cx);
                true
            }
            Some(SearchKeyAction::Next) => {
                self.search_next(cx);
                true
            }
            Some(SearchKeyAction::Previous) => {
                self.search_previous(cx);
                true
            }
            None => {
                // Text input is handled elsewhere via InlineInput actions
                false
            }
        }
    }

    pub(super) fn handle_search_input_changed(&mut self, cx: &mut Context<Self>) {
        self.search_state.set_query(self.search_input.text());
        if !self.search_state.has_valid_pattern() {
            // Cancel pending debounced searches and drop stale highlights immediately.
            self.search_debounce_token = self.search_debounce_token.wrapping_add(1);
            self.search_state.clear_results_preserving_query();
            self.clear_terminal_scrollbar_marker_cache();
            self.notify_search_inline_input(cx);
            return;
        }

        // Debounce search
        self.search_debounce_token = self.search_debounce_token.wrapping_add(1);
        let token = self.search_debounce_token;

        cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            smol::Timer::after(Duration::from_millis(SEARCH_DEBOUNCE_MS)).await;
            let _ = cx.update(|cx| {
                this.update(cx, |view, cx| {
                    if view.search_debounce_token == token {
                        view.perform_search();
                        view.scroll_to_current_match(cx);
                        view.notify_search_inline_input(cx);
                    }
                })
            });
        })
        .detach();
    }

    pub(super) fn render_search_bar(&self, cx: &mut Context<Self>) -> AnyElement {
        let overlay_style = self.overlay_style();
        let bar_bg = overlay_style.chrome_panel_background(SEARCH_BAR_BG_ALPHA);
        let input_bg = overlay_style.chrome_panel_background(SEARCH_INPUT_BG_ALPHA);
        let input_border = overlay_style.panel_foreground(0.10);
        let counter_text = overlay_style.panel_foreground(SEARCH_COUNTER_TEXT_ALPHA);
        let button_text = overlay_style.panel_foreground(SEARCH_BUTTON_TEXT_ALPHA);
        let button_hover_bg = overlay_style.chrome_panel_cursor(SEARCH_BUTTON_HOVER_BG_ALPHA);
        let button_active_bg = overlay_style.chrome_panel_cursor(0.28);
        let button_pressed_bg = overlay_style.chrome_panel_cursor(0.36);
        let strong_text = overlay_style.panel_foreground(0.92);
        let divider_color = overlay_style.panel_foreground(0.08);
        let transparent = gpui::Rgba {
            r: 0.0,
            g: 0.0,
            b: 0.0,
            a: 0.0,
        };

        let (current, total) = self.search_state.results().position().unwrap_or((0, 0));
        let counter_label =
            search_counter_label(current, total, self.search_input.text().is_empty());

        let has_error = self.search_state.error().is_some();
        let case_sensitive = self.search_state.is_case_sensitive();
        let regex_mode = matches!(self.search_state.mode(), termy_search::SearchMode::Regex);
        let error_color = gpui::Rgba {
            r: 0.98,
            g: 0.48,
            b: 0.48,
            a: 1.0,
        };

        div()
            .id("search-bar")
            .absolute()
            .top(px(8.0))
            .right(px(12.0))
            .w(px(SEARCH_BAR_WIDTH))
            .h(px(SEARCH_BAR_HEIGHT))
            .bg(bar_bg)
            .rounded(px(SEARCH_OVERLAY_GEOMETRY.panel_radius))
            .border_1()
            .border_color(overlay_style.panel_foreground(0.08))
            .shadow_lg()
            .flex()
            .items_center()
            .px(px(6.0))
            .gap(px(5.0))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|_this, _event, _window, cx| {
                    cx.stop_propagation();
                }),
            )
            .child(
                div()
                    .flex_1()
                    .h(px(28.0))
                    .min_w(px(0.0))
                    .relative()
                    .overflow_hidden()
                    .rounded(px(SEARCH_OVERLAY_GEOMETRY.input_radius))
                    .bg(input_bg)
                    .border_1()
                    .border_color(if has_error { error_color } else { input_border })
                    .flex()
                    .items_center()
                    .child(
                        div()
                            .absolute()
                            .left(px(10.0))
                            .top_0()
                            .bottom_0()
                            .w(px(14.0))
                            .flex()
                            .items_center()
                            .justify_center()
                            .child(
                                gpui::svg()
                                    .path(gpui::SharedString::from("icons/settings/search.svg"))
                                    .size(px(13.0))
                                    .text_color(button_text),
                            ),
                    )
                    .child(
                        div()
                            .absolute()
                            .left(px(32.0))
                            .right(px(if counter_label.is_some() { 58.0 } else { 10.0 }))
                            .top_0()
                            .bottom_0()
                            .overflow_hidden()
                            .child(div().relative().size_full().child(
                                self.render_inline_input_layer(
                                    Font {
                                        family: self.ui_font_family.clone(),
                                        ..Font::default()
                                    },
                                    px(13.0),
                                    strong_text.into(),
                                    {
                                        overlay_style
                                            .chrome_panel_cursor(SEARCH_INPUT_SELECTION_ALPHA)
                                            .into()
                                    },
                                    InlineInputAlignment::Left,
                                    cx,
                                ),
                            )),
                    )
                    .children(counter_label.map(|label| {
                        div()
                            .absolute()
                            .right(px(6.0))
                            .top(px(4.0))
                            .bottom(px(4.0))
                            .min_w(px(42.0))
                            .px(px(6.0))
                            .rounded(px(5.0))
                            .bg(if has_error {
                                let mut bg = error_color;
                                bg.a = 0.14;
                                bg
                            } else {
                                overlay_style.panel_foreground(0.06)
                            })
                            .flex()
                            .items_center()
                            .justify_center()
                            .text_size(px(10.0))
                            .text_color(if has_error { error_color } else { counter_text })
                            .child(label)
                    })),
            )
            .child(div().w(px(1.0)).h(px(20.0)).bg(divider_color).flex_none())
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(1.0))
                    .child(
                        div()
                            .id("search-prev")
                            .w(px(26.0))
                            .h(px(26.0))
                            .rounded(px(SEARCH_OVERLAY_GEOMETRY.control_radius))
                            .flex()
                            .items_center()
                            .justify_center()
                            .text_color(button_text)
                            .hover(|style| style.bg(button_hover_bg))
                            .active(move |style| style.bg(button_pressed_bg))
                            .cursor_pointer()
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|this, _event, _window, cx| {
                                    this.search_next(cx);
                                    cx.stop_propagation();
                                }),
                            )
                            .child(
                                gpui::svg()
                                    .path(gpui::SharedString::from("icons/settings/chevron-up.svg"))
                                    .size(px(13.0))
                                    .text_color(button_text),
                            ),
                    )
                    .child(
                        div()
                            .id("search-next")
                            .w(px(26.0))
                            .h(px(26.0))
                            .rounded(px(SEARCH_OVERLAY_GEOMETRY.control_radius))
                            .flex()
                            .items_center()
                            .justify_center()
                            .text_color(button_text)
                            .hover(|style| style.bg(button_hover_bg))
                            .active(move |style| style.bg(button_pressed_bg))
                            .cursor_pointer()
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|this, _event, _window, cx| {
                                    this.search_previous(cx);
                                    cx.stop_propagation();
                                }),
                            )
                            .child(
                                gpui::svg()
                                    .path(gpui::SharedString::from(
                                        "icons/settings/chevron-down.svg",
                                    ))
                                    .size(px(13.0))
                                    .text_color(button_text),
                            ),
                    ),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(1.0))
                    .child(
                        div()
                            .id("search-case-sensitive")
                            .w(px(30.0))
                            .h(px(26.0))
                            .rounded(px(SEARCH_OVERLAY_GEOMETRY.control_radius))
                            .flex()
                            .items_center()
                            .justify_center()
                            .text_size(px(10.0))
                            .text_color(if case_sensitive {
                                strong_text
                            } else {
                                button_text
                            })
                            .bg(if case_sensitive {
                                button_active_bg
                            } else {
                                transparent
                            })
                            .hover(|style| style.bg(button_hover_bg))
                            .active(move |style| style.bg(button_pressed_bg))
                            .cursor_pointer()
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|this, _event, _window, cx| {
                                    this.execute_search_command_action(
                                        CommandAction::ToggleSearchCaseSensitive,
                                        cx,
                                    );
                                    cx.stop_propagation();
                                }),
                            )
                            .child("Aa"),
                    )
                    .child(
                        div()
                            .id("search-regex")
                            .w(px(30.0))
                            .h(px(26.0))
                            .rounded(px(SEARCH_OVERLAY_GEOMETRY.control_radius))
                            .flex()
                            .items_center()
                            .justify_center()
                            .text_size(px(11.0))
                            .text_color(if regex_mode { strong_text } else { button_text })
                            .bg(if regex_mode {
                                button_active_bg
                            } else {
                                transparent
                            })
                            .hover(|style| style.bg(button_hover_bg))
                            .active(move |style| style.bg(button_pressed_bg))
                            .cursor_pointer()
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|this, _event, _window, cx| {
                                    this.execute_search_command_action(
                                        CommandAction::ToggleSearchRegex,
                                        cx,
                                    );
                                    cx.stop_propagation();
                                }),
                            )
                            .child(".*"),
                    ),
            )
            .child(
                div()
                    .id("search-close")
                    .w(px(26.0))
                    .h(px(26.0))
                    .rounded(px(SEARCH_OVERLAY_GEOMETRY.control_radius))
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_color(button_text)
                    .hover(|style| style.bg(button_hover_bg))
                    .active(move |style| style.bg(button_pressed_bg))
                    .cursor_pointer()
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _event, _window, cx| {
                            this.close_search(cx);
                            cx.stop_propagation();
                        }),
                    )
                    .child(
                        gpui::svg()
                            .path(gpui::SharedString::from("icons/tab_strip/x.svg"))
                            .size(px(11.0))
                            .text_color(button_text),
                    ),
            )
            .into_any()
    }
}

struct SearchLineSnapshot {
    first_line: i32,
    text: String,
    ranges: Vec<Range<usize>>,
}

impl SearchLineSnapshot {
    const MISSING_LINE: usize = usize::MAX;

    fn new(first_line: i32, line_count: usize) -> Self {
        Self {
            first_line,
            text: String::new(),
            ranges: Vec::with_capacity(line_count),
        }
    }

    fn line(&self, line_idx: i32) -> Option<&str> {
        let offset = usize::try_from(line_idx.checked_sub(self.first_line)?).ok()?;
        let range = self.ranges.get(offset)?;
        if range.start == Self::MISSING_LINE {
            return None;
        }
        self.text.get(range.clone())
    }

    fn last_line(&self) -> i32 {
        i32::try_from(self.ranges.len().saturating_sub(1))
            .ok()
            .and_then(|offset| self.first_line.checked_add(offset))
            .unwrap_or(self.first_line)
    }

    #[cfg(test)]
    fn len(&self) -> usize {
        self.ranges.len()
    }

    #[cfg(test)]
    fn lines(&self) -> impl Iterator<Item = &str> {
        self.ranges
            .iter()
            .filter(|range| range.start != Self::MISSING_LINE)
            .filter_map(|range| self.text.get(range.clone()))
    }
}

fn collect_search_line_texts(
    terminal: &Terminal,
    start_line: i32,
    end_line: i32,
) -> SearchLineSnapshot {
    let mut line_texts = None;
    let captured_range =
        terminal.for_each_line_cell_range(start_line, end_line, |range, line_idx, _, cell| {
            let snapshot = line_texts.get_or_insert_with(|| {
                let first = start_line.max(range.first_line);
                let last = end_line.min(range.last_line);
                let line_count = inclusive_line_count(first, last);
                let mut snapshot = SearchLineSnapshot::new(first, line_count);
                snapshot.ranges.resize(
                    line_count,
                    SearchLineSnapshot::MISSING_LINE..SearchLineSnapshot::MISSING_LINE,
                );
                snapshot
                    .text
                    .reserve(line_count.saturating_mul(range.columns));
                snapshot
            });
            let Some(index) = line_idx
                .checked_sub(snapshot.first_line)
                .and_then(|offset| usize::try_from(offset).ok())
            else {
                return;
            };
            let Some(cell_range) = snapshot.ranges.get_mut(index) else {
                return;
            };
            if cell_range.start == SearchLineSnapshot::MISSING_LINE {
                *cell_range = snapshot.text.len()..snapshot.text.len();
            }
            let character = cell.character();
            if character == '\0' || cell.is_trailing_wide_spacer() || character.is_control() {
                snapshot.text.push(' ');
            } else {
                snapshot.text.push(character);
                cell.append_combining_to(&mut snapshot.text);
            }
            cell_range.end = snapshot.text.len();
        });

    line_texts.unwrap_or_else(|| {
        let Some(range) = captured_range else {
            return SearchLineSnapshot::new(start_line, 0);
        };
        let first = start_line.max(range.first_line);
        let last = end_line.min(range.last_line);
        let line_count = inclusive_line_count(first, last);
        let mut snapshot = SearchLineSnapshot::new(first, line_count);
        snapshot.ranges.resize(
            line_count,
            SearchLineSnapshot::MISSING_LINE..SearchLineSnapshot::MISSING_LINE,
        );
        snapshot
    })
}

fn inclusive_line_count(first: i32, last: i32) -> usize {
    if first > last {
        return 0;
    }
    usize::try_from(i64::from(last) - i64::from(first) + 1).unwrap_or(usize::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;
    use termy_terminal_ui::TerminalSize;

    #[test]
    fn search_prefill_ignores_empty_selection() {
        assert_eq!(TerminalView::search_prefill_from_selection(None), None);
        assert_eq!(
            TerminalView::search_prefill_from_selection(Some("   \n\t".to_string())),
            None
        );
    }

    #[test]
    fn search_prefill_keeps_non_empty_selection() {
        assert_eq!(
            TerminalView::search_prefill_from_selection(Some("panic: missing semicolon".into())),
            Some("panic: missing semicolon".into())
        );
    }

    #[test]
    fn search_prefill_normalizes_multiline_selection_for_single_line_input() {
        assert_eq!(
            TerminalView::search_prefill_from_selection(Some("  line-1\r\nline-2\n  ".into())),
            Some("line-1 line-2".into())
        );
    }

    #[test]
    fn search_counter_label_hides_empty_query_without_results() {
        assert_eq!(search_counter_label(0, 0, true), None);
    }

    #[test]
    fn search_counter_label_reports_zero_results_for_non_empty_query() {
        assert_eq!(search_counter_label(0, 0, false), Some("0/0".to_string()));
    }

    #[test]
    fn search_counter_label_reports_current_result_position() {
        assert_eq!(search_counter_label(3, 12, false), Some("3/12".to_string()));
    }

    fn filled_line_count(lines: &SearchLineSnapshot) -> usize {
        lines.lines().filter(|line| !line.trim().is_empty()).count()
    }

    #[test]
    fn terminal_read_adapter_extracts_lines_for_both_runtime_variants() {
        let size = TerminalSize {
            cols: 24,
            rows: 4,
            ..TerminalSize::default()
        };

        let tmux = Terminal::new_tmux(
            size,
            TerminalOptions {
                scrollback_history: 256,
                ..TerminalOptions::default()
            },
        );
        tmux.feed_output(b"tmux-line\r\n");
        let tmux_lines = collect_search_line_texts(&tmux, 0, i32::from(size.rows) - 1);
        assert_eq!(tmux_lines.len(), usize::from(size.rows));
        assert!(
            filled_line_count(&tmux_lines) >= 1,
            "tmux terminal should expose at least one non-empty line"
        );

        let native = Terminal::new_native(size, None, None, None, None, None)
            .expect("native terminal should initialize for read adapter test");
        let native_lines = collect_search_line_texts(&native, 0, i32::from(size.rows) - 1);
        assert_eq!(native_lines.len(), usize::from(size.rows));
        let native_has_non_empty_buffer = native_lines.lines().any(|line| !line.is_empty());
        assert!(
            native_has_non_empty_buffer,
            "native terminal read adapter should expose at least one non-empty line buffer"
        );
    }

    #[test]
    fn terminal_read_adapter_preserves_tmon_combining_characters_in_history() {
        let size = TerminalSize {
            cols: 4,
            rows: 2,
            ..TerminalSize::default()
        };
        let terminal = Terminal::Tmon(TmonTerminalInstance {
            wakeup_id: 0,
            terminal: tmon::Terminal::new_display(
                tmon_adapter::size(size),
                tmon::Config::default(),
            ),
        });
        terminal.hydrate_output("e\u{301}\r\nmid\r\nnew".as_bytes());

        let lines = collect_search_line_texts(&terminal, -1, -1);
        assert_eq!(lines.line(-1), Some("e\u{301}   "));
    }

    #[test]
    fn search_key_action_uses_shift_for_enter_navigation() {
        assert_eq!(
            search_key_action("enter", false),
            Some(SearchKeyAction::Next)
        );
        assert_eq!(
            search_key_action("enter", true),
            Some(SearchKeyAction::Previous)
        );
        assert_eq!(
            search_key_action("escape", false),
            Some(SearchKeyAction::Close)
        );
        assert_eq!(search_key_action("a", true), None);
    }
}
