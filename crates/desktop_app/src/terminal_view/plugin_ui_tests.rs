use super::*;

#[test]
fn plugin_input_text_is_single_line_unicode_bounded() {
    assert_eq!(PluginUiView::bounded_input_text("a😀b", 2).as_ref(), "a😀");
    assert_eq!(
        PluginUiView::bounded_input_text("first\nsecond\r", 32).as_ref(),
        "firstsecond"
    );
}

#[test]
fn plugin_panel_fits_inside_small_terminal_viewports() {
    let (width, height) = PluginUiView::panel_dimensions(480.0, 268.0);
    assert!(width + MODAL_MARGIN * 2.0 <= 480.0);
    assert!(height + MODAL_MARGIN * 2.0 <= 268.0);
    assert_eq!(width, 448.0);
    assert_eq!(height, 236.0);
}
