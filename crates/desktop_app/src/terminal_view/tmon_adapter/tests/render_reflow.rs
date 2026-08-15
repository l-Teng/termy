use super::*;

#[test]
fn damage_covers_all_observable_changes_for_both_engines() {
    let initial = test_size(12, 4);
    let mut native = termy_core::Terminal::new_display(initial, None);
    let tmon = tmon::Terminal::new_display(size(initial), tmon::Config::default());

    assert_eq!(native.take_damage_snapshot(), TerminalDamageSnapshot::Full);
    assert_eq!(
        damage(tmon.take_damage_snapshot()),
        TerminalDamageSnapshot::Full
    );
    assert!(matches!(
        native.take_damage_snapshot(),
        TerminalDamageSnapshot::Partial(_)
    ));
    assert_eq!(
        damage(tmon.take_damage_snapshot()),
        TerminalDamageSnapshot::Partial(Vec::new())
    );

    let mut native_before = native_cells(&native);
    let mut tmon_before = tmon_cells(&tmon);
    for (name, output) in [
        ("plain text", b"abc".as_slice()),
        ("styled overwrite", b"\x1b[1;2H\x1b[31mZ\x1b[0m"),
        ("wide character", "\x1b[2;5H界".as_bytes()),
        ("overwrite wide spacer", b"\x1b[2;6HX"),
        (
            "insert-mode write",
            b"\x1b[1;1Habcdef\x1b[1;2H\x1b[4hZ\x1b[4l",
        ),
        ("insert and delete", b"\x1b[1;1H\x1b[2@++\x1b[1P"),
        ("erase", b"\x1b[2;1H\x1b[4X"),
    ] {
        native.feed_output(output);
        tmon.feed_output(output);
        let native_after = native_cells(&native);
        let tmon_after = tmon_cells(&tmon);
        assert_eq!(tmon_after, native_after, "{name}: visible state");
        let native_damage = native.take_damage_snapshot();
        let tmon_damage = damage(tmon.take_damage_snapshot());
        assert_damage_covers_changes(
            &native_before,
            &native_after,
            &native_damage,
            12,
            4,
            &format!("native {name}"),
        );
        assert_damage_covers_changes(
            &tmon_before,
            &tmon_after,
            &tmon_damage,
            12,
            4,
            &format!("Tmon {name}"),
        );
        native_before = native_after;
        tmon_before = tmon_after;
    }

    let scroll = b"\x1b[Hone\r\ntwo\r\nthree\r\nfour\r\nfive";
    native.feed_output(scroll);
    tmon.feed_output(scroll);
    assert_terminal_states_match(&native, &tmon, "damage scroll");
    assert_eq!(native.take_damage_snapshot(), TerminalDamageSnapshot::Full);
    assert_eq!(
        damage(tmon.take_damage_snapshot()),
        TerminalDamageSnapshot::Full
    );

    let resized = test_size(9, 5);
    native.resize(resized);
    tmon.resize(size(resized));
    assert_terminal_states_match(&native, &tmon, "damage resize");
    assert_eq!(native.take_damage_snapshot(), TerminalDamageSnapshot::Full);
    assert_eq!(
        damage(tmon.take_damage_snapshot()),
        TerminalDamageSnapshot::Full
    );
}

#[test]
fn kitty_partial_region_cursor_advance_matches_the_alacritty_engine() {
    let native_size = test_size(8, 4);
    let native = termy_core::Terminal::new_display(native_size, None);
    let tmon = tmon::Terminal::new_display(size(native_size), tmon::Config::default());
    let output = b"\x1b[2;3r\x1b[3;1H\x1b_Ga=T,f=32,s=1,v=1,i=82,c=2,r=3;AQID/w==\x1b\\\x1b[r";

    native.feed_output(output);
    tmon.feed_output(output);

    assert_terminal_states_match(&native, &tmon, "Kitty partial DECSTBM cursor advance");
    let native_placements = native.kitty_graphics_placements();
    let tmon_placements = tmon
        .kitty_graphics_placements()
        .into_iter()
        .map(kitty_graphics_placement)
        .collect::<Vec<_>>();
    assert_eq!(native_placements.len(), 1);
    assert_eq!(tmon_placements.len(), 1);
    assert_eq!(
        semantic_placement(&tmon_placements[0]),
        semantic_placement(&native_placements[0])
    );
}

#[test]
fn kitty_chunk_completion_on_another_screen_matches_the_alacritty_engine() {
    let native_size = test_size(8, 4);
    let native = termy_core::Terminal::new_display(native_size, None);
    let tmon = tmon::Terminal::new_display(size(native_size), tmon::Config::default());
    let output = b"\x1b[2;3H\x1b_Ga=T,f=32,s=1,v=1,i=7,c=2,r=1,m=1,q=1;AQI=\x1b\\\x1b[?1049h\x1b_Gm=0;A/8=\x1b\\\x1b[?1049l";

    native.feed_output(output);
    tmon.feed_output(output);

    assert_terminal_states_match(&native, &tmon, "Kitty cross-screen chunk completion");
    let native_placements = native.kitty_graphics_placements();
    let tmon_placements = tmon
        .kitty_graphics_placements()
        .into_iter()
        .map(kitty_graphics_placement)
        .collect::<Vec<_>>();
    assert_eq!(native_placements.len(), 1);
    assert_eq!(tmon_placements.len(), 1);
    assert_eq!(
        semantic_placement(&tmon_placements[0]),
        semantic_placement(&native_placements[0])
    );
}

#[test]
fn kitty_image_data_survives_terminal_reset_like_the_alacritty_engine() {
    let native_size = test_size(8, 4);
    let native = termy_core::Terminal::new_display(native_size, None);
    let tmon = tmon::Terminal::new_display(size(native_size), tmon::Config::default());
    let output = b"\x1b_Ga=t,f=32,s=1,v=1,i=7,q=2;AQID/w==\x1b\\\x1bc\x1b_Ga=p,i=7,C=1,q=2\x1b\\";

    native.feed_output(output);
    tmon.feed_output(output);

    assert_terminal_states_match(&native, &tmon, "Kitty image reuse after RIS");
    let native_placements = native.kitty_graphics_placements();
    let tmon_placements = tmon
        .kitty_graphics_placements()
        .into_iter()
        .map(kitty_graphics_placement)
        .collect::<Vec<_>>();
    assert_eq!(native_placements.len(), 1);
    assert_eq!(tmon_placements.len(), 1);
    assert_eq!(
        semantic_placement(&tmon_placements[0]),
        semantic_placement(&native_placements[0])
    );
}

#[test]
fn kitty_footer_survives_partial_top_scroll_like_the_alacritty_engine() {
    let native_size = test_size(8, 4);
    let native = termy_core::Terminal::new_display(native_size, None);
    let tmon = tmon::Terminal::new_display(size(native_size), tmon::Config::default());
    let output = b"\x1b[4;1H\x1b_Ga=T,f=32,s=1,v=1,i=7,C=1,q=2;AQID/w==\x1b\\\x1b[1;3r\x1b[3;1H\n";

    native.feed_output(output);
    tmon.feed_output(output);

    assert_terminal_states_match(&native, &tmon, "Kitty footer after partial top scroll");
    let native_placements = native.kitty_graphics_placements();
    let tmon_placements = tmon
        .kitty_graphics_placements()
        .into_iter()
        .map(kitty_graphics_placement)
        .collect::<Vec<_>>();
    assert_eq!(native_placements.len(), 1);
    assert_eq!(tmon_placements.len(), 1);
    assert_eq!(native_placements[0].viewport_row, 3);
    assert_eq!(
        semantic_placement(&tmon_placements[0]),
        semantic_placement(&native_placements[0])
    );
}

#[test]
fn kitty_placements_survive_column_resize_like_the_alacritty_engine() {
    let initial = test_size(8, 4);
    let mut native = termy_core::Terminal::new_display(initial, None);
    let tmon = tmon::Terminal::new_display(size(initial), tmon::Config::default());
    let output = b"\x1b[2;7H\x1b_Ga=T,f=32,s=1,v=1,i=83,c=3,r=2,C=1;AQID/w==\x1b\\";
    native.feed_output(output);
    tmon.feed_output(output);

    native.resize(test_size(5, 4));
    tmon.resize(size(test_size(5, 4)));
    assert!(native.kitty_graphics_placements().is_empty());
    assert!(tmon.kitty_graphics_placements().is_empty());

    native.resize(initial);
    tmon.resize(size(initial));
    let native_placements = native.kitty_graphics_placements();
    let tmon_placements = tmon
        .kitty_graphics_placements()
        .into_iter()
        .map(kitty_graphics_placement)
        .collect::<Vec<_>>();
    assert_eq!(native_placements.len(), 1);
    assert_eq!(tmon_placements.len(), 1);
    assert_eq!(
        semantic_placement(&tmon_placements[0]),
        semantic_placement(&native_placements[0])
    );
}

#[test]
fn reflow_resize_matches_the_alacritty_engine() {
    let initial = test_size(8, 3);
    let mut native = termy_core::Terminal::new_display(initial, None);
    let tmon = tmon::Terminal::new_display(size(initial), tmon::Config::default());
    let output = b"one two three four five six seven eight";
    native.feed_output(output);
    tmon.feed_output(output);
    assert_terminal_states_match(&native, &tmon, "before reflow");

    for resized in [test_size(5, 5), test_size(13, 2), test_size(9, 4)] {
        native.resize(resized);
        tmon.resize(size(resized));
        assert_terminal_states_match(
            &native,
            &tmon,
            &format!("reflow to {}x{}", resized.cols, resized.rows),
        );
    }
}

#[test]
fn scrolled_viewport_stays_anchored_through_width_reflow() {
    let initial = test_size(4, 2);
    let mut native = termy_core::Terminal::new_display(initial, None);
    let tmon = tmon::Terminal::new_display(size(initial), tmon::Config::default());
    native.feed_output(b"abcdefghijkl");
    tmon.feed_output(b"abcdefghijkl");
    assert!(native.scroll_display(1));
    assert!(tmon.scroll_display(1));
    assert_terminal_states_match(&native, &tmon, "scrolled before width reflow");

    let resized = test_size(2, 2);
    native.resize(resized);
    tmon.resize(size(resized));
    assert_terminal_states_match(&native, &tmon, "scrolled after width reflow");
}

#[test]
fn scrolled_viewport_matches_the_alacritty_engine_when_widening() {
    for (fixture, initial_cols, resized_cols) in [
        (b"abcdefghijklmnop".as_slice(), 4, 5),
        (b"abcdefghijklmnopqrstuvwxyzABCDEF".as_slice(), 4, 6),
    ] {
        let initial = test_size(initial_cols, 2);
        let mut native = termy_core::Terminal::new_display(initial, None);
        let tmon = tmon::Terminal::new_display(size(initial), tmon::Config::default());
        native.feed_output(fixture);
        tmon.feed_output(fixture);
        assert!(native.scroll_display(1));
        assert!(tmon.scroll_display(1));
        assert_terminal_states_match(&native, &tmon, "scrolled before widening");

        let resized = test_size(resized_cols, 2);
        native.resize(resized);
        tmon.resize(size(resized));
        assert_eq!(
            tmon.scroll_state(),
            native.scroll_state(),
            "scroll state after widening {initial_cols} to {resized_cols} columns"
        );
        assert_terminal_states_match(
            &native,
            &tmon,
            &format!("scrolled after widening {initial_cols} to {resized_cols} columns"),
        );
    }
}

#[test]
fn scrolled_soft_wrap_width_reflow_matrix_matches_the_alacritty_engine() {
    for initial_cols in 2..=7 {
        for rows in 2..=3 {
            for extra_cells in 0..=initial_cols + 1 {
                let cell_count = initial_cols * (rows + 3) + extra_cells;
                let fixture = (0..cell_count)
                    .map(|index| b'a' + (index % 26) as u8)
                    .collect::<Vec<_>>();
                let initial = test_size(initial_cols, rows);

                let history_probe = termy_core::Terminal::new_display(initial, None);
                history_probe.feed_output(&fixture);
                let history = history_probe.scroll_state().1;

                for offset in 1..=history {
                    for resized_rows in 1..=4 {
                        for resized_cols in 1..=9 {
                            if resized_cols == initial_cols && resized_rows == rows {
                                continue;
                            }
                            let mut native = termy_core::Terminal::new_display(initial, None);
                            let tmon =
                                tmon::Terminal::new_display(size(initial), tmon::Config::default());
                            native.feed_output(&fixture);
                            tmon.feed_output(&fixture);
                            assert!(native.scroll_display(offset as i32));
                            assert!(tmon.scroll_display(offset as i32));

                            let resized = test_size(resized_cols, resized_rows);
                            native.resize(resized);
                            tmon.resize(size(resized));
                            let context = format!(
                                "soft wrap {initial_cols}x{rows} to {resized_cols}x{resized_rows}, \
                                 {cell_count} cells, offset {offset}"
                            );
                            assert_eq!(tmon.scroll_state(), native.scroll_state(), "{context}");
                            assert_terminal_states_match(&native, &tmon, &context);
                        }
                    }
                }
            }
        }
    }
}

#[test]
fn scrolled_complex_width_reflow_matrix_matches_the_alacritty_engine() {
    for (fixture_name, fixture, initial) in [
        (
            "wide and combining",
            "界a🙂b界c\u{301}d🙂ef界gh🙂ij".as_bytes(),
            test_size(4, 2),
        ),
        (
            "hard lines",
            b"one\r\ntwo\r\nthree\r\nfour\r\nfive\r\nsix".as_slice(),
            test_size(5, 3),
        ),
        (
            "cursor above retained content",
            b"aaaa\r\nbbbb\r\ncccc\r\ndddd\x1b[2A".as_slice(),
            test_size(4, 3),
        ),
        (
            "styled sparse rows",
            b"ab\x1b[31m  \x1b[0mZ\r\nnext\x1b[3C!\r\nthird\r\nfourth".as_slice(),
            test_size(6, 3),
        ),
    ] {
        let history_probe = termy_core::Terminal::new_display(initial, None);
        history_probe.feed_output(fixture);
        let history = history_probe.scroll_state().1;

        for offset in 1..=history {
            // Alacritty's wide-character reflow can stall at a one-column
            // target, so keep this differential matrix at representable
            // two-cell-wide character widths. Tmon covers one-column
            // behavior with its own focused grid tests.
            for resized_cols in 2..=9 {
                if resized_cols == initial.cols {
                    continue;
                }
                let mut native = termy_core::Terminal::new_display(initial, None);
                let tmon = tmon::Terminal::new_display(size(initial), tmon::Config::default());
                native.feed_output(fixture);
                tmon.feed_output(fixture);
                assert!(native.scroll_display(offset as i32));
                assert!(tmon.scroll_display(offset as i32));

                let resized = test_size(resized_cols, initial.rows);
                native.resize(resized);
                tmon.resize(size(resized));
                let context = format!(
                    "{fixture_name}, {}x{} to {}x{}, offset {offset}",
                    initial.cols, initial.rows, resized.cols, resized.rows
                );
                assert_eq!(tmon.scroll_state(), native.scroll_state(), "{context}");
                assert_terminal_states_match(&native, &tmon, &context);
            }
        }
    }
}

#[test]
fn deterministic_scrolled_reflow_stress_matches_the_alacritty_engine() {
    let initial = test_size(6, 3);
    let resized = [
        test_size(3, 3),
        test_size(9, 3),
        test_size(4, 2),
        test_size(7, 4),
    ];
    for (fixture_name, fixture) in [
        (
            "soft ASCII",
            b"abcdefghijklmnopqrstuvwxyz0123456789".as_slice(),
        ),
        (
            "hard lines",
            b"one\r\ntwo\r\nthree\r\nfour\r\nfive\r\nsix".as_slice(),
        ),
        (
            "wide and combining",
            "ab界c\u{301}d🙂efgh界ijklmnop".as_bytes(),
        ),
        (
            "sparse rows",
            b"abc\x1b[6GZ\r\nnext\x1b[5C!\r\nthird\r\nfourth".as_slice(),
        ),
    ] {
        for requested_offset in 1..=3 {
            let mut native = termy_core::Terminal::new_display(initial, None);
            let tmon = tmon::Terminal::new_display(size(initial), tmon::Config::default());
            native.feed_output(fixture);
            tmon.feed_output(fixture);
            let history = native.scroll_state().1;
            let offset = requested_offset.min(history);
            if offset == 0 {
                continue;
            }
            assert!(native.scroll_display(offset as i32));
            assert!(tmon.scroll_display(offset as i32));
            assert_terminal_states_match(
                &native,
                &tmon,
                &format!("{fixture_name}, offset {offset}, before resize"),
            );

            for target in resized {
                native.resize(target);
                tmon.resize(size(target));
                assert_eq!(
                    tmon.scroll_state(),
                    native.scroll_state(),
                    "{fixture_name}, offset {offset}, scroll state after {}x{}",
                    target.cols,
                    target.rows
                );
                assert_terminal_states_match(
                    &native,
                    &tmon,
                    &format!(
                        "{fixture_name}, offset {offset}, after {}x{}",
                        target.cols, target.rows
                    ),
                );
            }
        }
    }
}

#[test]
fn cursor_after_sparse_wide_reflow_matches_the_alacritty_engine() {
    let initial = test_size(8, 2);
    let mut native = termy_core::Terminal::new_display(initial, None);
    let tmon = tmon::Terminal::new_display(size(initial), tmon::Config::default());
    let output = "  ws界a";
    native.feed_output(output.as_bytes());
    tmon.feed_output(output.as_bytes());
    assert_terminal_states_match(&native, &tmon, "before cursor reflow");

    let resized = test_size(5, 2);
    native.resize(resized);
    tmon.resize(size(resized));
    assert_terminal_states_match(&native, &tmon, "after cursor reflow");
}

#[test]
fn cursor_gap_reflow_matches_the_termy_native_engine() {
    let initial = test_size(8, 4);
    let mut native = termy_core::Terminal::new_display(initial, None);
    let tmon = tmon::Terminal::new_display(size(initial), tmon::Config::default());

    native.feed_output(b"\x1b[8G");
    tmon.feed_output(b"\x1b[8G");
    assert_terminal_states_match(&native, &tmon, "before empty cursor-gap reflow");

    let resized = test_size(3, 4);
    native.resize(resized);
    tmon.resize(size(resized));
    // Termy's native wrapper intentionally bottom-anchors the resize instead of
    // exposing the pinned Alacritty Term's two trailing blank rows as history.
    assert_eq!(native.scroll_state(), (0, 0));
    assert_eq!(
        native.cursor_state().map(|state| (state.col, state.row)),
        Some((1, 2))
    );
    assert_terminal_states_match(&native, &tmon, "after empty cursor-gap reflow");
}

#[test]
fn cursor_at_wrapped_wide_boundary_matches_the_alacritty_engine() {
    let initial = test_size(9, 3);
    let mut native = termy_core::Terminal::new_display(initial, None);
    let tmon = tmon::Terminal::new_display(size(initial), tmon::Config::default());
    let output = "\r\n  \u{301}m界α界 \u{8}\u{301}";
    native.feed_output(output.as_bytes());
    tmon.feed_output(output.as_bytes());
    assert_terminal_states_match(&native, &tmon, "before wide boundary reflow");

    let resized = test_size(4, 6);
    native.resize(resized);
    tmon.resize(size(resized));
    assert_terminal_states_match(&native, &tmon, "after wide boundary reflow");
}

#[test]
fn growing_preserves_an_empty_soft_wrap_continuation() {
    let initial = test_size(4, 3);
    let mut native = termy_core::Terminal::new_display(initial, None);
    let tmon = tmon::Terminal::new_display(size(initial), tmon::Config::default());
    let output = b"abcdefghX\x1b[2K";
    native.feed_output(output);
    tmon.feed_output(output);
    assert_terminal_states_match(&native, &tmon, "before empty continuation growth");

    let resized = test_size(8, 3);
    native.resize(resized);
    tmon.resize(size(resized));
    assert_terminal_states_match(&native, &tmon, "after empty continuation growth");
}

#[test]
fn growing_retains_the_clear_live_cursor_continuation() {
    let initial = test_size(2, 2);
    let mut native = termy_core::Terminal::new_display(initial, None);
    let tmon = tmon::Terminal::new_display(size(initial), tmon::Config::default());
    let output = b"abcdefghijk\x1b[1;1H";
    native.feed_output(output);
    tmon.feed_output(output);
    assert!(native.scroll_display(1));
    assert!(tmon.scroll_display(1));
    assert_terminal_states_match(&native, &tmon, "before clear cursor continuation growth");

    let resized = test_size(5, 2);
    native.resize(resized);
    tmon.resize(size(resized));

    assert_eq!(native.scroll_state(), (1, 2));
    assert_terminal_states_match(&native, &tmon, "after clear cursor continuation growth");
}

#[test]
fn wide_margin_reflow_matches_the_alacritty_engine() {
    let initial = test_size(4, 3);
    let mut native = termy_core::Terminal::new_display(initial, None);
    let tmon = tmon::Terminal::new_display(size(initial), tmon::Config::default());
    let output = "abc界x".as_bytes();
    native.feed_output(output);
    tmon.feed_output(output);
    assert_terminal_states_match(&native, &tmon, "before wide margin reflow");

    for resized in [test_size(5, 3), test_size(3, 4), test_size(7, 2)] {
        native.resize(resized);
        tmon.resize(size(resized));
        assert_terminal_states_match(
            &native,
            &tmon,
            &format!("wide margin reflow to {}x{}", resized.cols, resized.rows),
        );
    }
}

#[test]
fn row_only_resize_matches_the_alacritty_engine() {
    let initial = test_size(8, 3);
    let mut native = termy_core::Terminal::new_display(initial, None);
    let tmon = tmon::Terminal::new_display(size(initial), tmon::Config::default());
    native.feed_output(b"line-1\r\nline-2\r\nline-3\r\nline-4");
    tmon.feed_output(b"line-1\r\nline-2\r\nline-3\r\nline-4");
    assert_terminal_states_match(&native, &tmon, "before row resize");

    for resized in [test_size(8, 6), test_size(8, 2), test_size(8, 4)] {
        native.resize(resized);
        tmon.resize(size(resized));
        assert_terminal_states_match(
            &native,
            &tmon,
            &format!("row resize to {}x{}", resized.cols, resized.rows),
        );
    }
}

#[test]
fn repeated_output_and_reflow_resizes_match_the_alacritty_engine() {
    let initial = test_size(10, 4);
    let mut native = termy_core::Terminal::new_display(initial, None);
    let tmon = tmon::Terminal::new_display(size(initial), tmon::Config::default());
    let output = "one two three four\r\nfive 界 six seven\r\neight nine ten";
    native.feed_output(output.as_bytes());
    tmon.feed_output(output.as_bytes());
    assert_terminal_states_match(&native, &tmon, "before repeated resize trace");

    for (index, resized) in [
        test_size(6, 3),
        test_size(14, 6),
        test_size(8, 2),
        test_size(12, 5),
        test_size(5, 4),
        test_size(10, 4),
    ]
    .into_iter()
    .enumerate()
    {
        native.resize(resized);
        tmon.resize(size(resized));
        assert_terminal_states_match(
            &native,
            &tmon,
            &format!("repeated resize trace {index} after resize to {resized:?}"),
        );

        let output = format!("\r\nstep-{index} α界 tail");
        native.feed_output(output.as_bytes());
        tmon.feed_output(output.as_bytes());
        assert_terminal_states_match(
            &native,
            &tmon,
            &format!("repeated resize trace {index} after output"),
        );
    }
}

#[test]
fn wide_character_after_a_sparse_wrapped_row_reflows_like_alacritty() {
    let initial = test_size(12, 2);
    let resized = test_size(5, 2);
    let mut native = termy_core::Terminal::new_display(initial, None);
    let tmon = tmon::Terminal::new_display(size(initial), tmon::Config::default());
    let output = "word 界\x1b[12G α界 ";
    native.feed_output(output.as_bytes());
    tmon.feed_output(output.as_bytes());
    assert_terminal_states_match(&native, &tmon, "before sparse wide reflow");

    native.resize(resized);
    tmon.resize(size(resized));
    assert_terminal_states_match(&native, &tmon, "after sparse wide reflow");
}

#[test]
fn deterministic_reflow_stress_matches_the_alacritty_engine() {
    let mut random = 0xa076_1d64_78bd_642f_u64;
    for trace in 0..16 {
        let initial = test_size(10, 4);
        let mut native = termy_core::Terminal::new_display(initial, None);
        let tmon = tmon::Terminal::new_display(size(initial), tmon::Config::default());
        let mut operations = Vec::new();

        for step in 0..64 {
            random ^= random << 13;
            random ^= random >> 7;
            random ^= random << 17;
            let value = random;

            if value.is_multiple_of(7) {
                let resized = test_size(4 + (value >> 8) as u16 % 13, 2 + (value >> 24) as u16 % 6);
                native.resize(resized);
                tmon.resize(size(resized));
                operations.push(format!("resize {resized:?}"));
                assert_terminal_states_match(
                    &native,
                    &tmon,
                    &format!(
                        "reflow stress trace {trace}, step {step}, resize {resized:?}, operations {operations:?}"
                    ),
                );
                continue;
            }

            let output = match value % 12 {
                0 => vec![b'a' + value as u8 % 26],
                1 => b"word ".to_vec(),
                2 => "界".as_bytes().to_vec(),
                3 => "α界 ".as_bytes().to_vec(),
                4 => b"\r".to_vec(),
                5 => b"\n".to_vec(),
                6 => b"\r\n".to_vec(),
                7 => b"\x08".to_vec(),
                8 => b"  ".to_vec(),
                9 => b"\x1b[31;1mX\x1b[0m".to_vec(),
                10 => b"tail".to_vec(),
                _ => "\u{301}".as_bytes().to_vec(),
            };
            native.feed_output(&output);
            tmon.feed_output(&output);
            operations.push(format!("output {output:?}"));
            assert_terminal_states_match(
                &native,
                &tmon,
                &format!(
                    "reflow stress trace {trace}, step {step}, output {output:?}, operations {operations:?}"
                ),
            );
        }
    }
}
