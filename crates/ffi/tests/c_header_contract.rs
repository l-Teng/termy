#![cfg(unix)]

use std::{
    fs,
    path::Path,
    process::{Command, Stdio},
};

const C_CONTRACT_SOURCE: &str = r#"
#include <stddef.h>
#include <stdint.h>

#include "termy.h"

_Static_assert(TERMY_FFI_OK == 0, "status enum starts at OK");
_Static_assert(TERMY_FFI_PANICKED == 8, "panic status is stable");
_Static_assert(sizeof(TermyFfiCell) == 20, "cell ABI size is stable");
_Static_assert(offsetof(TermyFfiCell, italic) > offsetof(TermyFfiCell, line_wrapped), "text attributes use trailing cell padding");
_Static_assert(offsetof(TermyFfiFrame, cells_ptr) < offsetof(TermyFfiFrame, cursor), "frame cell storage precedes cursor");
_Static_assert(offsetof(TermyFfiFrameUpdate, damage_kind) < offsetof(TermyFfiFrameUpdate, spans_ptr), "frame update damage metadata precedes spans");
_Static_assert(offsetof(TermyFfiEventBatch, has_more) > offsetof(TermyFfiEventBatch, events_capacity), "event batch has_more follows vector storage");

void termy_header_contract(void) {
  TermyFfiSize size = termy_size_default();
  TermyFfiTerminal *terminal = 0;
  TermyFfiFrame frame = {0};
  TermyFfiKittyGraphicsBatch graphics = {0};
  uint64_t graphics_revision = 0;
  const uint8_t bytes[] = {'o', 'k'};

  TermyFfiStatus status = termy_display_terminal_new(size, &terminal);
  (void)status;
  (void)termy_terminal_feed_output(terminal, bytes, sizeof(bytes));
  (void)termy_terminal_snapshot(terminal, &frame);
  (void)termy_terminal_kitty_graphics_revision(terminal, &graphics_revision);
  (void)termy_terminal_kitty_graphics_placements(terminal, &graphics);
  (void)termy_kitty_graphics_batch_free(&graphics);
  (void)termy_frame_free(&frame);
  (void)termy_terminal_free(terminal);
}
"#;

fn compiler_exists(compiler: &str) -> bool {
    Command::new(compiler)
        .arg("--version")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
}

fn c_compiler() -> Option<String> {
    std::env::var("CC")
        .ok()
        .filter(|compiler| compiler_exists(compiler))
        .or_else(|| {
            ["cc", "clang", "gcc"]
                .into_iter()
                .find(|compiler| compiler_exists(compiler))
                .map(str::to_string)
        })
}

#[test]
fn c_header_compiles_minimal_display_terminal_contract() {
    let compiler = c_compiler().expect("expected CC, cc, clang, or gcc to compile termy.h");
    let temp = tempfile::tempdir().expect("tempdir");
    let source_path = temp.path().join("termy_header_contract.c");
    let object_path = temp.path().join("termy_header_contract.o");
    fs::write(&source_path, C_CONTRACT_SOURCE).expect("write C contract source");

    let header_dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("include");
    let output = Command::new(&compiler)
        .arg("-std=c11")
        .arg("-Wall")
        .arg("-Wextra")
        .arg("-Werror")
        .arg("-I")
        .arg(&header_dir)
        .arg("-c")
        .arg(&source_path)
        .arg("-o")
        .arg(&object_path)
        .output()
        .unwrap_or_else(|error| panic!("failed to run C compiler {compiler}: {error}"));

    assert!(
        output.status.success(),
        "failed to compile C contract against termy.h with {compiler}\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}
