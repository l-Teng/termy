# Bug Reproducer

## ✅ FIX_PROVEN — Bug reproduced and fix proven

> The same reproducer changed from failing to passing and broader checks passed.

**Project:** Termy  
**Bug:** ANSI italic, underline, and strikethrough are dropped  
**Environment:** macOS 26.5.2 arm64; Termy 0.2.30; rustc 1.96.0; Swift 6.2  
**Generated:** 2026-07-26

## Original report

The supplied screenshot reports that Termy renders SGR italic, underline, and strikethrough text as plain text.

| Contract | Expected | Actual |
|---|---|---|
| Observed behavior | SGR 3 text is italic, SGR 4 text is underlined, and SGR 9 text is struck through. | All four words render with the same plain style in the installed 0.2.30 build. |

## Minimal reproduction

The installed pre-fix app and the staged fixed app were each launched with an isolated config and the same shell fixture, which prints plain, italic, underline, and strike using SGR 0, 3, 4, and 9.

**Confirming signal:** The pre-fix screenshot shows italic, underline, and strike as plain; the fixed screenshot visibly shows all three requested styles.

### Reproduction files approved at Gate 1

- [bug-reproducer-before.jpeg](/Users/lassevestergaard/Documents/dev/termy/outputs/bug-reproducer-before.jpeg:1) — Installed 0.2.30 build rendering the isolated ANSI fixture without styles.
- [bug-reproducer-after.jpeg](/Users/lassevestergaard/Documents/dev/termy/outputs/bug-reproducer-after.jpeg:1) — Rebuilt app rendering italic, underline, and strike correctly.
- [frame.rs](/Users/lassevestergaard/Documents/dev/termy/crates/core/src/frame.rs:320) — Focused red-to-green snapshot regression test.

## Red to green evidence

| Evidence | Before fix | After fix |
|---|---:|---:|
| Exit code | 101 | 0 |
| Timed out | False | False |
| Duration | 735.553 ms | 207.914 ms |
| Same command | — | True |
| Broader suite | — | passed |

### Before — failing evidence

```text
error[E0609]: no field ˋitalicˋ on type ˋTermyCellˋ
   --> crates/core/src/frame.rs:327:32
    |
327 |         assert!(frame.cells[0].italic);
    |                                ^^^^^^ unknown field
    |
    = note: available fields are: ˋcharˋ, ˋfgˋ, ˋbgˋ, ˋuses_terminal_default_bgˋ, ˋboldˋ ... and 3 others

error[E0609]: no field ˋunderlineˋ on type ˋTermyCellˋ
   --> crates/core/src/frame.rs:328:33
    |
328 |         assert!(!frame.cells[0].underline);
    |                                 ^^^^^^^^^ unknown field
    |
    = note: available fields are: ˋcharˋ, ˋfgˋ, ˋbgˋ, ˋuses_terminal_default_bgˋ, ˋboldˋ ... and 3 others

error[E0609]: no field ˋstrikethroughˋ on type ˋTermyCellˋ
   --> crates/core/src/frame.rs:329:33
    |
329 |         assert!(!frame.cells[0].strikethrough);
    |                                 ^^^^^^^^^^^^^ unknown field
    |
    = note: available fields are: ˋcharˋ, ˋfgˋ, ˋbgˋ, ˋuses_terminal_default_bgˋ, ˋboldˋ ... and 3 others

error[E0609]: no field ˋitalicˋ on type ˋTermyCellˋ
   --> crates/core/src/frame.rs:330:33
    |
330 |         assert!(!frame.cells[1].italic);
    |                                 ^^^^^^ unknown field
    |
    = note: available fields are: ˋcharˋ, ˋfgˋ, ˋbgˋ, ˋuses_terminal_default_bgˋ, ˋboldˋ ... and 3 others

error[E0609]: no field ˋunderlineˋ on type ˋTermyCellˋ
   --> crates/core/src/frame.rs:331:32
    |
331 |         assert!(frame.cells[1].underline);
    |                                ^^^^^^^^^ unknown field
    |
    = note: available fields are: ˋcharˋ, ˋfgˋ, ˋbgˋ, ˋuses_terminal_default_bgˋ, ˋboldˋ ... and 3 others

error[E0609]: no field ˋstrikethroughˋ on type ˋTermyCellˋ
   --> crates/core/src/frame.rs:332:33
    |
332 |         assert!(!frame.cells[1].strikethrough);
    |                                 ^^^^^^^^^^^^^ unknown field
    |
    = note: available fields are: ˋcharˋ, ˋfgˋ, ˋbgˋ, ˋuses_terminal_default_bgˋ, ˋboldˋ ... and 3 others

error[E0609]: no field ˋitalicˋ on type ˋTermyCellˋ
   --> crates/core/src/frame.rs:333:33
    |
333 |         assert!(!frame.cells[2].italic);
    |                                 ^^^^^^ unknown field
    |
    = note: available fields are: ˋcharˋ, ˋfgˋ, ˋbgˋ, ˋuses_terminal_default_bgˋ, ˋboldˋ ... and 3 others

error[E0609]: no field ˋunderlineˋ on type ˋTermyCellˋ
   --> crates/core/src/frame.rs:334:33
    |
334 |         assert!(!frame.cells[2].underline);
    |                                 ^^^^^^^^^ unknown field
    |
    = note: available fields are: ˋcharˋ, ˋfgˋ, ˋbgˋ, ˋuses_terminal_default_bgˋ, ˋboldˋ ... and 3 others

error[E0609]: no field ˋstrikethroughˋ on type ˋTermyCellˋ
   --> crates/core/src/frame.rs:335:32
    |
335 |         assert!(frame.cells[2].strikethrough);
    |                                ^^^^^^^^^^^^^ unknown field
    |
    = note: available fields are: ˋcharˋ, ˋfgˋ, ˋbgˋ, ˋuses_terminal_default_bgˋ, ˋboldˋ ... and 3 others

For more information about this error, try ˋrustc --explain E0609ˋ.
error: could not compile ˋtermy_coreˋ (lib test) due to 9 previous errors
```

### After — fixed evidence

```text
running 1 test
.
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 236 filtered out; finished in 0.00s


running 0 tests

test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 1 filtered out; finished in 0.00s
```

## Root cause

The terminal parser retained the SGR flags, but TermyCell only carried bold into snapshots. Italic, underline, and strike were discarded before both the GPUI and FFI/Swift renderers, and their batching/cache keys therefore could not distinguish styled runs.

## Approved fix

Propagated italic, underline, and strikethrough through TermyCell, the C ABI, GPUI cell batches, and Swift render plans. Both renderers now select italic fonts and apply underline/strike decorations, with cache keys and row reuse accounting for those attributes.

**Why this is causal:** The fix restores the exact missing flags at the snapshot boundary and consumes them at both text-rendering endpoints; the same ANSI fixture changes from plain to visibly styled.

### Production files approved at Gate 2

- [frame.rs](/Users/lassevestergaard/Documents/dev/termy/crates/core/src/frame.rs:33) — Preserves SGR text attributes in terminal snapshots.
- [grid.rs](/Users/lassevestergaard/Documents/dev/termy/crates/terminal_ui/src/grid.rs:24) — Carries attributes through GPUI batching, cache reuse, fonts, and decorations.
- [lib.rs](/Users/lassevestergaard/Documents/dev/termy/crates/ffi/src/lib.rs:77) — Exports the three attributes through the stable C cell layout.
- [TerminalRenderPlan.swift](/Users/lassevestergaard/Documents/dev/termy/macos/Sources/TermySwift/Views/TerminalRenderPlan.swift:27) — Preserves attributes in Swift segments and cache keys.
- [TerminalGridView.swift](/Users/lassevestergaard/Documents/dev/termy/macos/Sources/TermySwift/Views/TerminalGridView.swift:679) — Renders italic fonts plus underline and strike decorations.

## Verification

| Check | Status | Evidence |
|---|---|---|
| Exact red-to-green core reproducer | ✅ passed | Same command changed from compile failure on aa566385 to 1 passing test. |
| Rebuilt GPUI app visual fixture | ✅ passed | Same isolated fixture visibly renders italic, underline, and strike. |
| Rust workspace check | ✅ passed | cargo check --workspace completed successfully. |
| Rust relevant suites | ✅ passed | core lib 237, FFI 37, terminal UI 141, and desktop app 692 tests passed. |
| Swift package suite | ✅ passed | 238 tests passed with 1 environment-only skip. |
| Release app bundle | ✅ passed | cargo bundle --release -p termy produced the staged app used for visual verification. |
| Unrelated baseline checks | ⚠️ warning | The existing resize_clear integration test fails independently, and rustfmt flags that untouched file. |

## Reproduce

```bash
printf '\033[0mplain \033[3mitalic\033[0m \033[4munderline\033[0m \033[9mstrike\033[0m\n'
```
```bash
cargo test -q -p termy_core frame::tests::snapshot_preserves_sgr_text_attributes -- --exact
```

## Limitations

- The full termy_core package run still fails crates/core/tests/resize_clear.rs at resize-to-20; it also fails alone and is unrelated to text attributes.
- cargo fmt --all -- --check reports pre-existing formatting in the untouched resize_clear.rs test.

## Residual risks

- Different underline variants are intentionally collapsed to the existing single-underline renderer style.

## Notes

- The C cell struct remains 20 bytes; the new booleans occupy its previous trailing padding.
- The installed app was used only as the pre-fix baseline and was not overwritten.

---

Generated by `$bug-reproducer`. A fix is proven only by the same red-to-green reproducer plus relevant broader checks.
