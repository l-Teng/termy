# Tmon

Tmon is Termy's experimental lightweight terminal engine, located in the
`crates/temon` directory so its implementation stays independent of GPUI and
the desktop application.

The current prototype is an independent, standard-library-only engine. It owns
its incremental VT parser, bounded primary/alternate grids and scrollback,
damage tracking, renderer-neutral cell types, and Unix PTY lifecycle. It does
not depend on `termy_core` or GPUI.

Run the desktop app with Tmon selected:

```sh
TERMY_EXPERIMENTAL_TMON_ENGINE=1 cargo run -p termy
```

The environment variable affects native terminals only. Tmux display panes
continue to use their existing parser.

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

## Benchmarking

Run the headless Tmon versus Alacritty parser/grid comparison and write
`tmon-alacritty-benchmark.txt`:

```sh
just benchmark-tmon
```

The `Tmon vs Alacritty Benchmark` GitHub Actions workflow runs on every push and
can also be started manually with configurable workload sizes. Each run uploads
the same text report as a 30-day artifact and includes it in the job summary.
The first measured optimization pass is documented in [PERFORMANCE.md](PERFORMANCE.md).

The suite compares identical byte streams, grid dimensions, and scrollback
limits in release mode. It reports integrated parser/grid throughput, current
full-frame API throughput, and static cell sizes. Feed calls intentionally keep
the original small workload payloads so results remain comparable with the
saved baseline. The snapshot APIs perform different conversion work, so that
ratio is not a raw engine-to-engine snapshot comparison. The suite does not
measure PTY I/O, GPUI rendering, whole-process memory, input latency, or
terminal compatibility, and speed ratios are not a feature-parity score.
