use super::*;

#[test]
fn osc8_hyperlink_ranges_match_the_alacritty_engine() {
    let native_size = test_size(12, 4);
    let native = termy_terminal_ui::Terminal::new_display(native_size, None);
    let tmon = tmon::Terminal::new_display(size(native_size), tmon::Config::default());
    let output = b"x\x1b]8;id=same;https://example.com/docs\x1b\\linked\x1b]8;;\x1b\\ y";
    native.feed_output(output);
    tmon.feed_output(output);

    let native_link = native.hyperlink_at(0, 3).unwrap();
    let tmon_link = tmon.hyperlink_at(0, 3).unwrap();
    assert_eq!(
        (tmon_link.start_col, tmon_link.end_col, tmon_link.target),
        (
            native_link.start_col,
            native_link.end_col,
            native_link.target
        )
    );
}

#[test]
fn osc8_identity_ranges_match_the_alacritty_engine() {
    let native_size = test_size(12, 4);
    let native = termy_terminal_ui::Terminal::new_display(native_size, None);
    let tmon = tmon::Terminal::new_display(size(native_size), tmon::Config::default());
    let output = b"\x1b]8;id=one;https://example.com/same\x1b\\A\
                   \x1b]8;id=two;https://example.com/same\x1b\\B\
                   \x1b]8;;https://example.com/same\x1b\\C\
                   \x1b]8;;https://example.com/same\x1b\\D";
    native.feed_output(output);
    tmon.feed_output(output);

    for col in 0..4 {
        let native_link = native
            .hyperlink_at(0, col)
            .expect("native link should exist");
        let tmon_link = tmon.hyperlink_at(0, col).expect("Tmon link should exist");
        assert_eq!(
            (tmon_link.start_col, tmon_link.end_col, tmon_link.target),
            (
                native_link.start_col,
                native_link.end_col,
                native_link.target
            ),
            "OSC 8 identity mismatch at column {col}"
        );
    }
}

#[test]
fn common_terminal_events_match_the_alacritty_engine() {
    let native_size = test_size(12, 4);
    let native = termy_terminal_ui::Terminal::new_display(native_size, None);
    let tmon = tmon::Terminal::new_display(size(native_size), tmon::Config::default());
    let output = b"\x07\x1b]2;hello\x07\x1b]52;c;dGVybXk=\x07";
    native.feed_output(output);
    tmon.feed_output(output);

    let (native_events, native_has_more) = native.drain_events(&mut |_| None);
    let (tmon_events, tmon_has_more) = tmon.drain_events();
    assert!(!native_has_more);
    assert!(!tmon_has_more);
    let native_events = native_events
        .into_iter()
        .filter_map(semantic_event)
        .collect::<Vec<_>>();
    let tmon_events = tmon_events
        .into_iter()
        .filter_map(event)
        .filter_map(semantic_event)
        .collect::<Vec<_>>();

    assert_eq!(tmon_events, native_events);
}

#[test]
fn invalid_utf8_osc_title_events_match_the_alacritty_engine() {
    let native_size = test_size(12, 4);
    let native = termy_terminal_ui::Terminal::new_display(native_size, None);
    let tmon = tmon::Terminal::new_display(size(native_size), tmon::Config::default());
    let output = b"\x1b]2;A\xffB\x07";
    native.feed_output(output);
    tmon.feed_output(output);

    let native_events = native
        .drain_events(&mut |_| None)
        .0
        .into_iter()
        .filter_map(semantic_event)
        .collect::<Vec<_>>();
    let tmon_events = tmon
        .drain_events()
        .0
        .into_iter()
        .filter_map(event)
        .filter_map(semantic_event)
        .collect::<Vec<_>>();

    assert_eq!(tmon_events, native_events);
}

#[test]
fn osc_parameter_limit_matches_the_alacritty_engine() {
    let native_size = test_size(12, 4);
    let native = termy_terminal_ui::Terminal::new_display(native_size, None);
    let tmon = tmon::Terminal::new_display(size(native_size), tmon::Config::default());
    let mut output = b"\x1b]2".to_vec();
    for index in 0..20 {
        output.extend_from_slice(format!(";{index}").as_bytes());
    }
    output.push(0x07);
    native.feed_output(&output);
    tmon.feed_output(&output);

    let native_events = native
        .drain_events(&mut |_| None)
        .0
        .into_iter()
        .filter_map(semantic_event)
        .collect::<Vec<_>>();
    let tmon_events = tmon
        .drain_events()
        .0
        .into_iter()
        .filter_map(event)
        .filter_map(semantic_event)
        .collect::<Vec<_>>();

    assert_eq!(tmon_events, native_events);
}

#[test]
fn invalid_utf8_custom_osc_events_match_the_native_interceptor() {
    let native_size = test_size(12, 4);
    let native = termy_terminal_ui::Terminal::new_display(native_size, None);
    let tmon = tmon::Terminal::new_display(size(native_size), tmon::Config::default());
    let output = b"\x1b]7;file:///tmp/A\xffB\x07\
        \x1b]9;9;/tmp/A\xffB\x07\
        \x1b]9;4;1;50;\xff\x07\
        \x1b]133;A;\xff\x07";
    native.feed_output(output);
    tmon.feed_output(output);

    let native_events = native
        .drain_events(&mut |_| None)
        .0
        .into_iter()
        .filter_map(semantic_event)
        .collect::<Vec<_>>();
    let tmon_events = tmon
        .drain_events()
        .0
        .into_iter()
        .filter_map(event)
        .filter_map(semantic_event)
        .collect::<Vec<_>>();

    assert_eq!(tmon_events, native_events);
}

#[test]
fn title_stack_parameters_match_the_alacritty_engine() {
    let native_size = test_size(12, 4);
    let native = termy_terminal_ui::Terminal::new_display(native_size, None);
    let tmon = tmon::Terminal::new_display(size(native_size), tmon::Config::default());

    for output in [
        b"\x1b]2;A\x07".as_slice(),
        b"\x1b[22;1t".as_slice(),
        b"\x1b]2;B\x07".as_slice(),
        b"\x1b[23;1t".as_slice(),
    ] {
        native.feed_output(output);
        tmon.feed_output(output);

        let native_events = native
            .drain_events(&mut |_| None)
            .0
            .into_iter()
            .filter_map(semantic_event)
            .collect::<Vec<_>>();
        let tmon_events = tmon
            .drain_events()
            .0
            .into_iter()
            .filter_map(event)
            .filter_map(semantic_event)
            .collect::<Vec<_>>();
        assert_eq!(tmon_events, native_events);
    }
}

#[test]
fn osc_palette_overrides_and_resets_match_the_alacritty_engine() {
    let native_size = test_size(12, 4);
    let native = termy_terminal_ui::Terminal::new_display(native_size, None);
    let tmon = tmon::Terminal::new_display(size(native_size), tmon::Config::default());
    assert_eq!(tmon_palette(&tmon), native_palette(&native));

    let set = b"\x1b]4;1;#123456;200;rgb:f/8/0\x1b\\\
                \x1b]10;#010203;#040506;#070809\x07";
    native.feed_output(set);
    tmon.feed_output(set);
    let palette = tmon_palette(&tmon);
    assert_eq!(palette, native_palette(&native));
    assert_eq!(palette.indexed[1], Some((0x12, 0x34, 0x56)));
    assert_eq!(palette.indexed[200], Some((0xff, 0x88, 0x00)));
    assert_eq!(palette.foreground, Some((1, 2, 3)));
    assert_eq!(palette.background, Some((4, 5, 6)));
    assert_eq!(palette.cursor, Some((7, 8, 9)));

    let partial_reset = b"\x1b]104;1\x07\x1b]110\x07\x1b]112\x07";
    native.feed_output(partial_reset);
    tmon.feed_output(partial_reset);
    let palette = tmon_palette(&tmon);
    assert_eq!(palette, native_palette(&native));
    assert_eq!(palette.indexed[1], None);
    assert_eq!(palette.indexed[200], Some((0xff, 0x88, 0x00)));
    assert_eq!(palette.foreground, None);
    assert_eq!(palette.background, Some((4, 5, 6)));
    assert_eq!(palette.cursor, None);

    let full_reset = b"\x1b]104\x07\x1b]111\x07";
    native.feed_output(full_reset);
    tmon.feed_output(full_reset);
    let palette = tmon_palette(&tmon);
    assert_eq!(palette, native_palette(&native));
    assert!(palette.indexed.iter().all(Option::is_none));
    assert_eq!(palette.background, None);
}

#[test]
fn ris_preserves_osc_palette_overrides_like_the_alacritty_engine() {
    let native_size = test_size(12, 4);
    let native = termy_terminal_ui::Terminal::new_display(native_size, None);
    let tmon = tmon::Terminal::new_display(size(native_size), tmon::Config::default());
    let set_then_reset = b"\x1b]4;1;#123456\x07\x1b]10;#010203;#040506;#070809\x07\x1bc";

    native.feed_output(set_then_reset);
    tmon.feed_output(set_then_reset);

    let palette = tmon_palette(&tmon);
    assert_eq!(palette, native_palette(&native));
    assert_eq!(palette.indexed[1], Some((0x12, 0x34, 0x56)));
    assert_eq!(palette.foreground, Some((1, 2, 3)));
    assert_eq!(palette.background, Some((4, 5, 6)));
    assert_eq!(palette.cursor, Some((7, 8, 9)));
}
