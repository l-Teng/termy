# Tmon

Tmon is Termy's experimental lightweight terminal engine, located in the
`crates/temon` directory so its implementation stays independent of GPUI and
the desktop application.

The current prototype is an independent engine with no production Cargo
dependencies. It owns its incremental VT parser, bounded primary/alternate
grids and scrollback, damage tracking, renderer-neutral cell types, and Unix
PTY lifecycle. It does not depend on `termy_core` or GPUI; Unix PTY operations
and PNG validation use direct platform/libz FFI.

Run the desktop app with Tmon selected:

```sh
TERMY_EXPERIMENTAL_TMON_ENGINE=1 just run
```

Open Termy's inspector and select the **Terminal** tab; the **Engine** row reads
`tmon` when the experimental backend is active and `alacritty` otherwise. The
startup log also prints `using experimental Tmon terminal engine` when selected.

The environment variable affects native terminals only. Tmux display panes
continue to use their existing parser. The independent PTY is currently
Unix-only; on Windows an exact `1` request logs a warning and falls back to the
unchanged Alacritty-backed native engine.

## Direction

- Keep the engine independent of GPUI.
- Keep one bounded grid allocation and bounded scrollback.
- Emit damage-scoped frame updates rather than full-grid redraws.
- Keep PTY reads and parsing off the UI thread.
- Keep expanding VT/OSC/DCS and graphics compatibility behind the opt-in gate.
- Differential-test common shells and TUIs against the existing native engine.

Tmon is still experimental. The existing Alacritty-backed native engine remains
the default and tmux panes keep their existing behavior unless the environment
variable is exactly `1`.

## Compatibility boundary

Tmon is differential-tested against Termy's Alacritty-backed native wrapper for
the VT sequences that wrapper implements. It intentionally implements several
standards and xterm extensions that the pinned parser ignores: legacy
alternate-screen/cursor modes 47, 1047, and 1048; DEC selective erase and
DECST8C tab-stop reset; ISO 2022 G1/G2/G3 designation and single shifts; and
DECRQSS/XTGETTCAP replies. OSC 52 additionally accepts valid unpadded or
whitespace-separated base64. These are compatibility extensions, not benchmark
shortcuts, and are tested separately from strict differential fixtures.

Tmon also keeps combining marks available to selection and search. Termy's
existing Alacritty cell adapter omits that out-of-line text; its default behavior
is deliberately unchanged by the experiment. OSC/DCS payloads are bounded to
64 KiB so an unterminated control string cannot grow without limit.

## Benchmarking

Run the headless Tmon versus Alacritty parser/grid comparison and write
`tmon-alacritty-benchmark.txt`:

```sh
just benchmark-tmon
```

The `Tmon vs Alacritty Benchmark` GitHub Actions workflow runs on every push and
can also be started manually with configurable workload sizes. When the runner
remains available, the report is uploaded as a 30-day artifact and included in
the job summary; its finalizer records setup outcomes even if the comparison
cannot start. CI defaults to eight samples and rejects odd counts so each
workload has four Tmon-first and four Alacritty-first pairs. It fails when the
median of a workload's paired ratios misses its committed target; local runs
print the same verdict without failing unless `TMON_BENCH_ENFORCE_TARGETS=1` is
set. After the timed comparison, both `just benchmark-tmon` and CI append a
report-only allocation pass to the same text report. Manual inputs are bounded
so an oversized request fails before it can consume the job timeout and prevent
report upload. The first measured optimization pass is documented in
[PERFORMANCE.md](PERFORMANCE.md).

The suite compares identical byte streams, grid dimensions, and scrollback
limits in release mode. Before timing, an untimed preflight compares up to 32
MiB of every workload across every normalized grid and scrollback cell plus
cursor, scroll, and screen state. It reports integrated Termy backend
throughput, current full-frame API throughput, and static cell sizes. Feed calls
intentionally keep the original small workload payloads so results remain
comparable with the saved baseline. The snapshot APIs perform different
conversion work, so that ratio is not a raw engine-to-engine snapshot
comparison. Its ratio and retention against the supplied 19.880x baseline are
report-only: enforcing `19.880x * 0.95` on hosted CI would not be sound because
the APIs do different work and can react differently to runner noise. The suite
does not measure PTY I/O, GPUI rendering, input latency, or complete terminal
compatibility, and speed ratios are not a feature-parity score.

The allocation pass rebuilds only the example with the
`benchmark-allocations` feature and requires
`TMON_BENCH_ALLOCATIONS_ONLY=1`; that feature build refuses timed mode. Its
`System` allocator wrapper is absent from the normal throughput binary, so the
timed measurements have no counter overhead. For every engine and workload it
warms 2 MiB on an uncounted terminal, constructs a fresh terminal uncounted, and
then counts process-wide allocator requests only during the same complete-payload
`feed_output` loop as the timed samples. It reports the exact processed byte
count, which can slightly exceed the configured target. Snapshots and terminal
destruction happen after the guard has disabled new hooks and waited for any
in-flight hooks. The report includes
successful allocation calls and requested bytes (including `alloc_zeroed`),
deallocation calls and requested bytes, successful reallocation calls with
old/new requested bytes, and their signed net requested-byte change. These are
allocator-request figures, not RSS, retained or usable heap, allocator metadata,
or physical memory; allocations from concurrent process activity can also be
included while the guard is active. All allocation results are report-only.
