use super::*;

#[test]
fn plugin_input_text_is_single_line_unicode_bounded() {
    assert_eq!(
        PluginUiView::bounded_input_text("a😀b", 2, false).as_ref(),
        "a😀"
    );
    assert_eq!(
        PluginUiView::bounded_input_text("first\nsecond\r", 32, false).as_ref(),
        "firstsecond"
    );
    assert_eq!(
        PluginUiView::bounded_input_text("first\nsecond", 32, true).as_ref(),
        "first\nsecond"
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

#[test]
fn keyboard_focus_skips_inert_and_nested_list_items() {
    let item = |id: &str, action: Option<&str>, disabled: bool| PluginUiNode::ListItem {
        id: id.to_string(),
        title: id.to_string(),
        subtitle: None,
        keywords: Vec::new(),
        status: None,
        payload: None,
        action: action.map(str::to_string),
        disabled,
    };
    let nodes = vec![
        item("inert", None, false),
        item("actionable", Some("open"), false),
        item("disabled", Some("open"), true),
        PluginUiNode::List {
            id: "results".to_string(),
            action: Some("open".to_string()),
            selected_id: None,
            search_placeholder: None,
            filtering: true,
            is_loading: false,
            children: vec![item("nested", Some("open"), false)],
        },
    ];
    let mut ids = Vec::new();

    PluginUiView::interactive_control_ids(&nodes, &mut ids);

    assert_eq!(ids, vec!["actionable", "results"]);
}
