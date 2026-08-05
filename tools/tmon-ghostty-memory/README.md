# Tmon–Ghostty memory and throughput benchmark

This standalone macOS tool compares the final `ri_phys_footprint` of Tmon and
statically linked `libghostty-vt` terminal state. Every engine/workload sample
runs in a fresh child process from the same executable.

The benchmark uses a 120x40 display, a 10,000-row scrollback workload, and
plain, styled, Unicode, and mixed payloads. It reports Ghostty both before and
after its caller-driven full scrollback compression pass. Cursor, history size,
and sampled semantic cells are checked after memory is sampled so traversing
compressed history cannot change the measured footprint. This is a benchmark
sanity check, not a terminal-compatibility or feature-parity suite.

The same binary also has a balanced integrated-feed throughput mode. Tmon and
Ghostty receive the same 64 KiB chunks after a 2 MiB warmup, with alternating
sample order and an exact state preflight. It compares Ghostty's normal feed
path; Ghostty scrollback compression is an explicit post-feed operation and is
therefore covered by the memory mode instead.

## GitHub Actions

Run the manually dispatched **Tmon vs Ghostty Benchmark** workflow. It
checks out Ghostty revision `9e30f70f23418fecbdca1088673000417527c4e4`, builds
`libghostty-vt` with Zig 0.16.0, runs ten fresh processes per case by default,
runs eight balanced 32 MiB throughput samples, and uploads the complete text
report as a 30-day artifact. The job fails unless Tmon's median paired
throughput ratio exceeds `1.000x` for every workload and Tmon's median physical
footprint is strictly below every corresponding Ghostty result, including
Ghostty's explicit full-compression result for scrollback workloads.

## Local usage

Requirements:

- macOS;
- Zig 0.16.0;
- a Ghostty source checkout at the same pinned revision.

From the Termy repository root:

```sh
GHOSTTY_DIR=../ghostty just benchmark-tmon-ghostty-memory
```

That recipe runs both the ten-process physical-memory comparison and the
eight-sample feed-throughput comparison after building `libghostty-vt` once.

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

Run only the throughput mode against an existing installation with:

```sh
GHOSTTY_VT_PREFIX=/tmp/ghostty-vt-install \
cargo run --locked --release --manifest-path tools/tmon-ghostty-memory/Cargo.toml -- --throughput
```

`TMON_GHOSTTY_THROUGHPUT_SAMPLES` controls the balanced, even sample count and
`TMON_GHOSTTY_THROUGHPUT_TARGET_MIB` controls timed input per engine/sample.

`GHOSTTY_VT_PREFIX` must contain `include/ghostty/vt.h` and
`lib/libghostty-vt.a`.
