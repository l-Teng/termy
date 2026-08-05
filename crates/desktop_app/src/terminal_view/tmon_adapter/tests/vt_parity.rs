use super::*;

#[test]
fn common_vt_fixtures_match_the_alacritty_engine() {
    for (name, fixture) in [
        (
            "basic cursor and rendition",
            b"plain\r\ntext\x1b[2;3H@\x1b[31;44;1;3;4;7;8;9mX\x1b[0m".as_slice(),
        ),
        (
            "wide text and scrollback",
            "ab界cd\r\nline2\r\nline3\r\nline4\r\nline5".as_bytes(),
        ),
        (
            "combining characters on narrow and wide cells",
            "e\u{301}\u{308} 界\u{301}\r\n12345678901e\u{301}".as_bytes(),
        ),
        (
            "editing characters",
            b"abcdefghijklM\x1b[2D\x1b[2@++\x1b[3P\x1b[4X\x1b[2K".as_slice(),
        ),
        (
            "oversized delete preserves the prefix",
            b"abcdefghijkl\x1b[1;6H\x1b[999P".as_slice(),
        ),
        (
            "scroll regions and lines",
            b"one\r\ntwo\r\nthree\x1b[2;4r\x1b[4;1H\n\x1bM\x1b[2L\x1b[1M".as_slice(),
        ),
        (
            "alternate screen and modes",
            b"main\x1b7\x1b[32mP\x1b[?1049hALT\x1b[?2004h\x1b[?1000h\x1b[?1006h\x1b[=5u\x1b[?1049lR\x1b8S".as_slice(),
        ),
        (
            "DEC graphics and alignment",
            b"\x1b(0lqk\x1b(B\x1b#8\x1b[Hok".as_slice(),
        ),
        (
            "DEC charset blank mapping",
            b"\x1b(0_`x\x1b(B".as_slice(),
        ),
        (
            "alternate screen isolates charset designation",
            b"\x1b[?1049h\x1b(0x\x1b[?1049lx".as_slice(),
        ),
    ] {
        assert_engines_match(name, fixture);
    }
}

#[test]
fn alternate_screen_round_trip_preserves_scrolled_primary_viewport() {
    let terminal_size = test_size(4, 2);
    let native = termy_terminal_ui::Terminal::new_display(terminal_size, None);
    let tmon = tmon::Terminal::new_display(size(terminal_size), tmon::Config::default());

    native.feed_output(b"a\r\nb\r\nc");
    tmon.feed_output(b"a\r\nb\r\nc");
    assert!(native.scroll_display(1));
    assert!(tmon.scroll_display(1));
    assert_terminal_states_match(&native, &tmon, "before alternate screen round trip");

    native.feed_output(b"\x1b[?1049hALT\x1b[?1049l");
    tmon.feed_output(b"\x1b[?1049hALT\x1b[?1049l");

    assert_terminal_states_match(&native, &tmon, "after alternate screen round trip");
}

#[test]
fn bounded_scrollback_eviction_preserves_scrolled_viewport_anchor() {
    let terminal_size = test_size(4, 2);
    let runtime_config = TerminalRuntimeConfig {
        scrollback_history: 2,
        ..TerminalRuntimeConfig::default()
    };
    let native = termy_terminal_ui::Terminal::new_display(terminal_size, Some(&runtime_config));
    let tmon = tmon::Terminal::new_display(
        size(terminal_size),
        tmon::Config {
            scrollback_history: 2,
            ..tmon::Config::default()
        },
    );

    native.feed_output(b"a\r\nb\r\nc\r\nd");
    tmon.feed_output(b"a\r\nb\r\nc\r\nd");
    assert!(native.scroll_display(1));
    assert!(tmon.scroll_display(1));
    assert_terminal_states_match(&native, &tmon, "before bounded history eviction");

    native.feed_output(b"\r\ne");
    tmon.feed_output(b"\r\ne");

    assert_terminal_states_match(&native, &tmon, "after bounded history eviction");
}

#[test]
fn alternate_row_resize_preserves_pending_wrap() {
    let initial = test_size(4, 2);
    let mut native = termy_terminal_ui::Terminal::new_display(initial, None);
    let tmon = tmon::Terminal::new_display(size(initial), tmon::Config::default());

    native.feed_output(b"\x1b[?1049habcd");
    tmon.feed_output(b"\x1b[?1049habcd");
    assert_terminal_states_match(&native, &tmon, "before alternate row resize");

    let resized = test_size(4, 3);
    native.resize(resized);
    tmon.resize(size(resized));
    native.feed_output(b"X");
    tmon.feed_output(b"X");

    assert_terminal_states_match(&native, &tmon, "after alternate row resize");
}

#[test]
fn extended_vt_fixtures_match_the_alacritty_engine() {
    for (name, fixture) in [
        (
            "origin addressing",
            b"111\r\n222\r\n333\r\n444\x1b[2;3r\x1b[?6h\x1b[1;2H@\x1b[2B#\x1b[?6l\x1b[4;4H$"
                .as_slice(),
        ),
        (
            "clamped scroll region homes the cursor",
            b"abc\x1b[2;4r".as_slice(),
        ),
        (
            "insert and autowrap modes",
            b"abcdefghijkl\x1b[1;3H\x1b[4hXY\x1b[4l\x1b[?7l\x1b[1;12HZQ\x1b[?7hR".as_slice(),
        ),
        ("newline mode", b"abc\x1b[20h\nx\x1b[20l\ny".as_slice()),
        (
            "colored erasures",
            b"abcdefghijkl\r\nmnopqrstuvwx\x1b[45m\x1b[1;4H\x1b[K\x1b[2;3H\x1b[3X\x1b[0m"
                .as_slice(),
        ),
        (
            "underline metadata editing and erasure",
            b"\x1b]8;;https://example.com/edit\x1b\\\x1b[4:2;58;2;1;2;3mABCDE\x1b[0m\x1b]8;;\x1b\\\r\x1b[2C\x1b[@\x1b[P\x1b[X"
                .as_slice(),
        ),
        (
            "wide spacer overwrite keeps link and color but drops combining text",
            "\x1b]8;;https://example.com/wide\x1b\\\x1b[4:3;58:2::4:5:6m界\u{301}\x1b[0m\x1b]8;;\x1b\\\r\x1b[2GX"
                .as_bytes(),
        ),
        (
            "extended colors",
            b"\x1b[38;5;123;48;2;4;5;6mA\x1b[39;49mB\x1b[38:2::7:8:9;48:5:201mC".as_slice(),
        ),
        (
            "extended underline colors and resets",
            b"\x1b[4;58;2;1;2;3mA\x1b[58;5;201mB\x1b[58:2::4:5:6mC\x1b[59mD\x1b[58:5:7mE\x1b[0mF"
                .as_slice(),
        ),
        (
            "SGR reset preserves OSC 8 hyperlink identity",
            b"\x1b]8;;https://example.com/reset\x1b\\\x1b[4;58;2;1;2;3mA\x1b[0mB\x1b]8;;\x1b\\C"
                .as_slice(),
        ),
        (
            "invalid UTF-8 OSC 8 targets are rejected",
            b"\x1b]8;;https://exa\xffmple\x1b\\X".as_slice(),
        ),
        (
            "SGR cancel bold and malformed colors",
            b"\x1b[1;4mA\x1b[21mB\x1b[38;2;300;1;2mC\x1b[0m\x1b[48;5;999;3mD".as_slice(),
        ),
        (
            "unsupported SGR subparameter groups are ignored",
            b"\x1b[1:2mX".as_slice(),
        ),
        (
            "custom tabs",
            b"a\tb\x1b[3g\r\x1b[5G\x1bH\r\tX\x1b[2IY\x1b[1Zz".as_slice(),
        ),
        (
            "cursor visibility and style",
            b"abc\x1b[?25l\x1b[6 q\x1b[2;3H\x1b[?25h".as_slice(),
        ),
        (
            "OSC cursor shape",
            b"abc\x1b]50;CursorShape=10\x07".as_slice(),
        ),
        (
            "8-bit APC Kitty graphics",
            b"\x9fGa=T,f=32,s=1,v=1,i=7;/wAA/w==\x9cZ".as_slice(),
        ),
        (
            "DEC column mode side effects",
            b"one\r\ntwo\x1b[2;3r\x1b[?3hX\x1b[?3lY".as_slice(),
        ),
        (
            "wide repeat at the margin",
            "\x1b[31;44m界\r\x1b[12G\x1b[3b".as_bytes(),
        ),
        (
            "repeat count above the old tmon cap",
            b"A\x1b[4097b".as_slice(),
        ),
        (
            "zero-width format and combining characters",
            "a\u{200d}b c\u{093f}d e\u{feff}f g\u{1e944}h".as_bytes(),
        ),
        (
            "encoded C1 controls are ignored",
            "A\u{85}B C\u{9b}D".as_bytes(),
        ),
        (
            "width-three scalar uses native insert shift",
            "AB\r\x1b[4h\u{17d8}".as_bytes(),
        ),
        (
            "C1 cursor and line controls",
            b"abc\x85x\x9b2;3H@\x84y\x8dz".as_slice(),
        ),
        (
            "cancel and substitute interrupted CSI",
            b"\x1b[31\x18mX\x1b[32\x1aY".as_slice(),
        ),
        (
            "escape interrupts OSC strings",
            b"abc\x1b]2;unfinished\x1b[2;3HZ".as_slice(),
        ),
        (
            "escape interrupts DCS strings",
            b"abc\x1bP$qm\x1b[2;3HZ".as_slice(),
        ),
        (
            "C1 ST before a DCS final byte is ignored",
            b"\x1bP\x9cABC\x1b\\Z".as_slice(),
        ),
        (
            "escape interrupts ignored strings",
            b"abc\x1b^ignored\x1b[2;3HZ".as_slice(),
        ),
        (
            "C1 ST does not terminate ignored APC strings",
            b"\x1b_Xignored\x9cZ\x1b\\Q".as_slice(),
        ),
        (
            "non-Kitty 8-bit APC is forwarded as native text",
            b"\x9fXpayload\x9cZ".as_slice(),
        ),
        (
            "C1 ST remains OSC payload in UTF-8 mode",
            b"\x1b]2;A\x9cZ\x07Q".as_slice(),
        ),
        (
            "CSI with an unrelated intermediate is ignored",
            b"abc\x1b[31 mZ".as_slice(),
        ),
        (
            "CSI parameters after intermediates are ignored",
            b"abc\x1b[31 ;mZ".as_slice(),
        ),
        (
            "CSI subparameters stay within their parameter",
            b"\x1b[2:9;4HX".as_slice(),
        ),
        (
            "duplicate CSI private markers are ignored",
            b"abc\x1b[?25?lZ".as_slice(),
        ),
        (
            "multiple CSI intermediates are ignored",
            b"abc\x1b[5  qZ".as_slice(),
        ),
        (
            "C0 controls preserve the escape state",
            b"abc\x1b\x07[2;3HZ".as_slice(),
        ),
        (
            "C0 controls preserve escape intermediates",
            b"\x1b(\x070l".as_slice(),
        ),
        (
            "multiple escape intermediates are ignored",
            b"abc\x1b((0l".as_slice(),
        ),
        (
            "unknown charset designators preserve the active charset",
            b"\x1b(0q\x1b(Xq".as_slice(),
        ),
        (
            "OSC dispatches before a cancel control",
            b"\x1b]8;;https://example.com\x18X".as_slice(),
        ),
        (
            "backward tab honors column zero and an empty tab table",
            b"\x1b[6G\x1b[Z\x1b[3g\x1b[6G\x1b[Z".as_slice(),
        ),
        (
            "origin mode horizontal addressing follows the active region",
            b"\x1b[?6h\x1b[2;4r\x1b[9G".as_slice(),
        ),
    ] {
        assert_engines_match(name, fixture);
    }
}

#[test]
fn extended_underline_styles_match_the_alacritty_engine() {
    let terminal_size = test_size(12, 4);
    let native = termy_terminal_ui::Terminal::new_display(terminal_size, None);
    let tmon = tmon::Terminal::new_display(size(terminal_size), tmon::Config::default());
    let bytes = b"\x1b[4mA\x1b[4:2mB\x1b[4:3mC\x1b[4:4mD\x1b[4:5mE\x1b[4:0mF\x1b[4:0:1mG\x1b[24mH";
    native.feed_output(bytes);
    tmon.feed_output(bytes);

    let native_styles = native.with_term(|term| {
        (&term.grid()[Line(0)])
            .into_iter()
            .take(8)
            .map(|cell| native_underline_style(cell.flags))
            .collect::<Vec<_>>()
    });
    let tmon_styles = tmon.snapshot().cells[..8]
        .iter()
        .map(|cell| cell.attributes.underline_style())
        .collect::<Vec<_>>();

    assert_eq!(tmon_styles, native_styles);
}

#[test]
fn compact_underline_metadata_survives_reflow_scrollback_and_screen_swaps() {
    let initial = test_size(10, 3);
    let mut native = termy_terminal_ui::Terminal::new_display(initial, None);
    let tmon = tmon::Terminal::new_display(size(initial), tmon::Config::default());
    let combining = "\u{301}\u{302}\u{303}\u{304}\u{305}\u{306}\u{307}\u{308}\u{309}";
    let primary = format!(
        "\x1b]8;id=meta;https://example.com/meta\x1b\\\x1b[4:3;58:2::12:34:56me{combining}\x1b[0mB\x1b]8;;\x1b\\ tail text"
    );
    native.feed_output(primary.as_bytes());
    tmon.feed_output(primary.as_bytes());
    assert_terminal_states_match(&native, &tmon, "combined metadata before reflow");
    assert!(tmon_grid_lines(&tmon).iter().flatten().any(|cell| {
        cell.character == 'e'
            && cell.combining == combining
            && cell.hyperlink
            && cell.underline == tmon::UnderlineStyle::Curly
            && cell.underline_color == Some(RawColor::Rgb(12, 34, 56))
    }));

    for resized in [test_size(6, 4), test_size(13, 2), initial] {
        native.resize(resized);
        tmon.resize(size(resized));
        assert_terminal_states_match(
            &native,
            &tmon,
            &format!(
                "combined metadata after {}x{} reflow",
                resized.cols, resized.rows
            ),
        );
    }

    native.feed_output(b"\r\none\r\ntwo\r\nthree\r\nfour");
    tmon.feed_output(b"\r\none\r\ntwo\r\nthree\r\nfour");
    assert_terminal_states_match(&native, &tmon, "combined metadata in scrollback");

    let alternate = format!(
        "\x1b[?1049h\x1b]8;id=alt;https://example.com/alt\x1b\\\x1b[4:5;58;5;201mA{combining}\x1b]8;;\x1b\\"
    );
    native.feed_output(alternate.as_bytes());
    tmon.feed_output(alternate.as_bytes());
    assert_terminal_states_match(&native, &tmon, "combined metadata on alternate screen");
    native.feed_output(b"\x1b[?1049l");
    tmon.feed_output(b"\x1b[?1049l");
    assert_terminal_states_match(&native, &tmon, "combined metadata after screen restore");

    let saved = b"\x1b]8;id=saved;https://example.com/saved\x1b\\\x1b[4:4;58;5;99m\x1b7\x1b[0m\x1b]8;;\x1b\\\x1b8S";
    native.feed_output(saved);
    tmon.feed_output(saved);
    assert_terminal_states_match(&native, &tmon, "saved cursor restores underline metadata");
}

#[test]
fn reflow_ignores_trailing_spaces_with_only_link_and_underline_color_metadata() {
    let initial = test_size(8, 2);
    let mut native = termy_terminal_ui::Terminal::new_display(initial, None);
    let tmon = tmon::Terminal::new_display(size(initial), tmon::Config::default());
    let bytes = b"A\x1b]8;;https://example.com/trailing\x1b\\\x1b[58;2;1;2;3m   \x1b]8;;\x1b\\";
    native.feed_output(bytes);
    tmon.feed_output(bytes);
    assert_terminal_states_match(&native, &tmon, "trailing metadata before reflow");

    for resized in [test_size(4, 3), test_size(10, 2)] {
        native.resize(resized);
        tmon.resize(size(resized));
        assert_terminal_states_match(
            &native,
            &tmon,
            &format!(
                "trailing metadata after {}x{} reflow",
                resized.cols, resized.rows
            ),
        );
    }
}

#[test]
fn deterministic_common_vt_traces_match_the_alacritty_engine() {
    let terminal_size = test_size(12, 4);
    let mut random = 0x4d59_5df4_d0f3_3173_u64;

    for trace in 0..32 {
        let native = termy_terminal_ui::Terminal::new_display(terminal_size, None);
        let tmon = tmon::Terminal::new_display(size(terminal_size), tmon::Config::default());
        let mut stream = Vec::new();

        for step in 0..96 {
            random ^= random << 13;
            random ^= random >> 7;
            random ^= random << 17;
            let value = random;
            let count = value as usize % 3 + 1;
            let row = value as usize % 4 + 1;
            let col = (value >> 8) as usize % 12 + 1;
            let action = match (value >> 16) % 56 {
                0 => vec![b'a' + (value as u8 % 26)],
                1 => b"text".to_vec(),
                2 => b"\r".to_vec(),
                3 => b"\n".to_vec(),
                4 => b"\x08".to_vec(),
                5 => b"\t".to_vec(),
                6 => format!("\x1b[{count}A").into_bytes(),
                7 => format!("\x1b[{count}B").into_bytes(),
                8 => format!("\x1b[{count}C").into_bytes(),
                9 => format!("\x1b[{count}D").into_bytes(),
                10 => format!("\x1b[{row};{col}H").into_bytes(),
                11 => format!("\x1b[{}K", value % 3).into_bytes(),
                12 => format!("\x1b[{}J", value % 4).into_bytes(),
                13 => format!("\x1b[{count}X").into_bytes(),
                14 => format!("\x1b[{count}@").into_bytes(),
                15 => format!("\x1b[{count}P").into_bytes(),
                16 => format!("\x1b[{count}L").into_bytes(),
                17 => format!("\x1b[{count}M").into_bytes(),
                18 => format!("\x1b[{count}S").into_bytes(),
                19 => format!("\x1b[{count}T").into_bytes(),
                20 => [
                    b"\x1b[0m".as_slice(),
                    b"\x1b[1;3;4m",
                    b"\x1b[22;23;24m",
                    b"\x1b[7;8;9m",
                    b"\x1b[27;28;29m",
                    b"\x1b[31;44m",
                    b"\x1b[39;49m",
                    b"\x1b[38;5;123;48;2;4;5;6m",
                ][value as usize % 8]
                    .to_vec(),
                21 => if value & 1 == 0 { b"\x1b[s" } else { b"\x1b[u" }.to_vec(),
                22 => {
                    let top = value as usize % 3 + 1;
                    let bottom = top + (value >> 8) as usize % (5 - top) + 1;
                    format!("\x1b[{top};{bottom}r").into_bytes()
                }
                23 => if value & 1 == 0 {
                    b"\x1b[?6h"
                } else {
                    b"\x1b[?6l"
                }
                .to_vec(),
                24 => if value & 1 == 0 {
                    b"\x1b[?7h"
                } else {
                    b"\x1b[?7l"
                }
                .to_vec(),
                25 => if value & 1 == 0 {
                    b"\x1b[4h"
                } else {
                    b"\x1b[4l"
                }
                .to_vec(),
                26 => format!("\x1b[{count}b").into_bytes(),
                27 => "界".as_bytes().to_vec(),
                28 => b"\x1bM".to_vec(),
                29 => b"\x1bE".to_vec(),
                30 => [b"\x1bH".as_slice(), b"\x1b[g", b"\x1b[3g"][value as usize % 3].to_vec(),
                31 => if value & 1 == 0 {
                    b"\x1b[?1049h"
                } else {
                    b"\x1b[?1049l"
                }
                .to_vec(),
                32 => if value & 1 == 0 {
                    b"\x1b[?25h"
                } else {
                    b"\x1b[?25l"
                }
                .to_vec(),
                33 => b"\x1b(0lqk\x1b(B".to_vec(),
                34 => b"\x1b7save\x1b8".to_vec(),
                35 => format!("\x1b[{count}E").into_bytes(),
                36 => format!("\x1b[{count}F").into_bytes(),
                37 => format!("\x1b[{col}G").into_bytes(),
                38 => format!("\x1b[{row}d").into_bytes(),
                39 => if value & 1 == 0 {
                    format!("\x1b[{count}I")
                } else {
                    format!("\x1b[{count}Z")
                }
                .into_bytes(),
                40 => [b"\x1b[g".as_slice(), b"\x1b[3g"][value as usize % 2].to_vec(),
                41 => [
                    b"\x1b[?1000h".as_slice(),
                    b"\x1b[?1002h",
                    b"\x1b[?1003h",
                    b"\x1b[?1000l\x1b[?1002l\x1b[?1003l",
                ][value as usize % 4]
                    .to_vec(),
                42 => b"\x1b[38:2::12:34:56;48:5:201;4:3mX\x1b[0m".to_vec(),
                43 => b"\x1b]8;;https://example.com\x1b\\link\x1b]8;;\x1b\\".to_vec(),
                44 => if value & 1 == 0 {
                    b"\x1b[6 q"
                } else {
                    b"\x1b[2 q"
                }
                .to_vec(),
                45 => if value & 1 == 0 {
                    b"\x1b[?3h"
                } else {
                    b"\x1b[?3l"
                }
                .to_vec(),
                46 => b"\x1b#8\x1b[2J".to_vec(),
                47 => b"\x1b[31\x18mX\x1b[32\x1aY".to_vec(),
                48 => if value & 1 == 0 {
                    b"\x1b[20h"
                } else {
                    b"\x1b[20l"
                }
                .to_vec(),
                49 => b"\x1b[31 mZ".to_vec(),
                50 => [
                    b"\x1b[4m".as_slice(),
                    b"\x1b[4:2m",
                    b"\x1b[4:3m",
                    b"\x1b[4:4m",
                    b"\x1b[4:5m",
                    b"\x1b[4:0m",
                ][value as usize % 6]
                    .to_vec(),
                51 => b"\x1b[58;2;12;34;56m".to_vec(),
                52 => b"\x1b[58:5:201m".to_vec(),
                53 => if value & 1 == 0 {
                    b"\x1b[59m"
                } else {
                    b"\x1b[24m"
                }
                .to_vec(),
                54 => "\x1b]8;id=random;https://example.com/random\x1b\\\x1b[4:3;58:2::1:2:3me\u{301}\x1b[0mX\x1b]8;;\x1b\\"
                    .as_bytes()
                    .to_vec(),
                _ => "\u{301}".as_bytes().to_vec(),
            };

            native.feed_output(&action);
            tmon.feed_output(&action);
            stream.extend_from_slice(&action);
            assert_terminal_states_match(
                &native,
                &tmon,
                &format!("trace {trace}, step {step}, action {action:?}, stream {stream:?}"),
            );
        }
    }
}
