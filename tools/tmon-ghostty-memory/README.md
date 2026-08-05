# Tmon–Ghostty memory benchmark

This standalone macOS tool compares the final `ri_phys_footprint` of Tmon and
statically linked `libghostty-vt` terminal state. Every engine/workload sample
runs in a fresh child process from the same executable.

The benchmark uses a 120x40 display, a 10,000-row scrollback workload, and
plain, styled, Unicode, and mixed payloads. It reports Ghostty both before and
after its caller-driven full scrollback compression pass. Cursor, history size,
and sampled semantic cells are checked after memory is sampled so traversing
compressed history cannot change the measured footprint. This is a benchmark
sanity check, not a terminal-compatibility or feature-parity suite.

## GitHub Actions

Run the manually dispatched **Tmon vs Ghostty Memory Benchmark** workflow. It
checks out Ghostty revision `9e30f70f23418fecbdca1088673000417527c4e4`, builds
`libghostty-vt` with Zig 0.16.0, runs ten fresh processes per case by default,
and uploads the complete text report as a 30-day artifact.

## Local usage

Requirements:

- macOS;
- Zig 0.16.0;
- a Ghostty source checkout at the same pinned revision.

From the Termy repository root:

```sh
GHOSTTY_DIR=../ghostty just benchmark-tmon-ghostty-memory
```

Override the sample count or report path when needed:

```sh
TMON_GHOSTTY_MEMORY_SAMPLES=2 \
TMON_GHOSTTY_MEMORY_OUTPUT=/tmp/tmon-ghostty-memory.txt \
GHOSTTY_DIR=../ghostty \
just benchmark-tmon-ghostty-memory
```

If `libghostty-vt` is already installed, run the standalone package directly:

```sh
GHOSTTY_VT_PREFIX=/tmp/ghostty-vt-install \
cargo run --locked --release --manifest-path tools/tmon-ghostty-memory/Cargo.toml
```

`GHOSTTY_VT_PREFIX` must contain `include/ghostty/vt.h` and
`lib/libghostty-vt.a`.
