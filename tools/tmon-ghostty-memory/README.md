# Tmon, Alacritty, and Ghostty engine benchmark

This standalone macOS tool compares three terminal engines directly:

- the current in-tree Tmon crate;
- `alacritty_terminal` 0.26;
- statically linked `libghostty-vt` from Ghostty revision
  `9e30f70f23418fecbdca1088673000417527c4e4`.

It measures integrated VT-feed throughput and fresh-process physical memory.
Ghostty's caller-driven full scrollback compression is reported as
`ghostty-compressed`, a clearly separate memory-only result. It is never used
as a throughput engine.

## Fairness and correctness

All three primary engines use the same 120x40 grid, 10,000-line history limit,
input bytes, input chunks, warmup byte target, and timed byte target. Every six
samples exercise all six engine orders, so sample counts must be a multiple of
six; both modes default to 12 samples.

Throughput calibrates each workload before collecting samples. All three
engines receive the same whole-cycle 64 MiB pilot after the same warmup. The
fastest observed pilot rate is multiplied by 500 ms, then that one shared byte
target is rounded up to a complete workload cycle and used unchanged for every
engine and sample. This provides headroom over the hard 250 ms minimum timed
sample; a run fails validity if any measured sample is shorter. Calibrated
targets are capped at 2 GiB.

Before timing, the harness runs an untimed semantic check for every selected
workload. It replays the exact timed setup and chunk sequence, including the
final CRLF and the actual 64 KiB, 4 KiB, or operation-batch boundaries. It
compares byte count, history size, cursor position, and a normalized digest of
every cell in every logical row, including scrollback.
The exact scrollback preflight stops one row below the configured cap. Sustained
warmup, calibration, and timed feeds cross the cap; every post-timing retained
history count is reported instead of required to be equal because Ghostty
documents its line cap as page-granular and approximate. The history query runs
only after timing stops.
For the memory benchmark, physical footprint is sampled before this full-buffer
traversal so reading compressed history cannot inflate the measured value.

The workloads cover plain, styled, Unicode, mixed, alternate-screen,
full-screen redraw, cursor-edit, and 10,000-row scrollback behavior. This is a
benchmark sanity check, not a complete terminal-compatibility suite.

## Local usage

Requirements:

- macOS;
- Zig 0.16.0;
- a Ghostty checkout at the pinned revision above.

From the Termy repository root, the repository recipe builds the pinned
`libghostty-vt` and runs both modes:

```sh
GHOSTTY_DIR=../ghostty just benchmark-tmon-ghostty-memory
```

If `libghostty-vt` is already installed, run either mode directly:

```sh
GHOSTTY_VT_PREFIX=/tmp/ghostty-vt-install \
cargo run --locked --release \
  --manifest-path tools/tmon-ghostty-memory/Cargo.toml

GHOSTTY_VT_PREFIX=/tmp/ghostty-vt-install \
cargo run --locked --release \
  --manifest-path tools/tmon-ghostty-memory/Cargo.toml -- --throughput
```

`GHOSTTY_VT_PREFIX` must contain `include/ghostty/vt.h` and
`lib/libghostty-vt.a`.

## GitHub Actions

The manually dispatched **Tmon, Alacritty, and Ghostty Benchmark** workflow
checks out the pinned Ghostty revision, builds `libghostty-vt` with Zig 0.16.0,
and runs both benchmark modes. It uploads the text report and raw memory and
throughput JSON as a 30-day artifact. Performance rankings are reported without
failing the job; configuration, build, runtime, semantic, and timing-validity
failures still fail it.

## Configuration

- `TMON_GHOSTTY_MEMORY_SAMPLES` controls fresh-process memory samples.
- `TMON_GHOSTTY_THROUGHPUT_SAMPLES` controls feed-throughput samples.
- `TMON_GHOSTTY_THROUGHPUT_TARGET_MIB` optionally sets a minimum shared target
  before per-workload whole-cycle rounding; calibration may select a larger
  target. The accepted floor is 1 through 2,048 MiB, and the rounded target must
  remain within the 2 GiB safety cap.
- `TMON_GHOSTTY_THROUGHPUT_WORKLOAD` selects one throughput workload.

Both sample-count variables must be between 6 and 60 and divisible by 6.
Throughput rounds its 2 MiB base warmup up to a complete workload cycle and
uses 64 KiB chunks for scrollback streams; interactive traces retain their
natural operation batches.

## Machine-readable results

The normal report includes stable key/value lines prefixed with:

- `TERMINAL_ENGINE_BENCH_MEMORY`;
- `TERMINAL_ENGINE_BENCH_CALIBRATION`;
- `TERMINAL_ENGINE_BENCH_THROUGHPUT`;
- `TERMINAL_ENGINE_BENCH_RATIO`.

For chart generation, prefer JSON so no prose parsing is required. Set
`TERMINAL_ENGINE_BENCH_JSON` to an output path for either invocation:

```sh
TERMINAL_ENGINE_BENCH_JSON=/tmp/memory.json \
GHOSTTY_VT_PREFIX=/tmp/ghostty-vt-install \
cargo run --locked --release \
  --manifest-path tools/tmon-ghostty-memory/Cargo.toml

TERMINAL_ENGINE_BENCH_JSON=/tmp/throughput.json \
GHOSTTY_VT_PREFIX=/tmp/ghostty-vt-install \
cargo run --locked --release \
  --manifest-path tools/tmon-ghostty-memory/Cargo.toml -- --throughput
```

Memory JSON contains every process footprint and its sample/order indices plus
median, p05, and p95 statistics. Throughput JSON contains every sample's byte
count, elapsed seconds, order position, calculated MiB/s, aggregate ranges,
paired Tmon ratios, pilot observations, calibrated targets, and minimum-duration
metadata. Pilot and timed samples also contain the retained history queried
after each timer stops. Performance rankings are reported as data rather than
treated as harness errors; invalid configuration, build/runtime failures,
semantic mismatches, and implausibly short timed samples make the command fail.
