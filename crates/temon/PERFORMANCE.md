# Tmon performance report

Date: 2026-08-04

This report covers the first focused optimization pass over Tmon. The original
Linux numbers came from commit `03d7ca5c5420ca141afe56f725341faadb71af18`.
All before/after percentages below use an additional same-machine macOS
baseline so host differences are not presented as engine improvements.

## Result

The saved release measurements below used a 120x40 grid, 10,000 scrollback
lines, 32 MiB per parser sample, 7 samples, and 250 full-frame API calls per
snapshot sample. Seven is the historical sample count, not the current harness
default; CI now uses eight so every workload has balanced AB/BA sample order.

| Workload | Before Tmon | After Tmon | Final Alacritty | Tmon improvement | Final ratio |
| --- | ---: | ---: | ---: | ---: | ---: |
| Plain scrollback | 94.3 MiB/s | 236.8 MiB/s | 109.9 MiB/s | +151.1% | 2.15x |
| Styled TUI redraw | 158.1 MiB/s | 215.9 MiB/s | 135.1 MiB/s | +36.6% | 1.60x |
| Unicode + wide cells | 102.6 MiB/s | 148.2 MiB/s | 98.1 MiB/s | +44.4% | 1.51x |
| Cursor + line edits | 4.3 MiB/s | 6.6 MiB/s | 5.1 MiB/s | +53.5% | 1.28x |

The same final code was benchmarked twice. Ratios were 2.11x/2.15x for plain,
1.61x/1.60x for styled, 1.35x/1.51x for Unicode, and 1.24x/1.28x for edits.
Raw throughput moved with temperature and competing system load, but every
target remained above its required margin in both runs.

| Metric | Before | After | Change |
| --- | ---: | ---: | ---: |
| Tmon current full-frame API throughput | 125,310.6 calls/s | 268,084.4 calls/s | +113.9% |
| Static `Cell` size | 32 bytes | 24 bytes | -25.0% |
| Maximum cell storage at benchmark dimensions | about 36.9 MiB | about 27.7 MiB | -9.23 MiB |

The full-frame API result does not compare equivalent raw work. Tmon copies its
internal cells, while `termy_core` converts Alacritty cells into a `TermyFrame`.
The benchmark output now labels this explicitly. The saved same-machine
before/after result shows that Tmon's own throughput improved; it does not make
the cross-engine ratio a valid hosted-CI regression gate.

## Supplied Linux baseline

The target baseline used Linux x86_64 and rustc 1.97.1:

| Workload | Tmon | Alacritty | Ratio |
| --- | ---: | ---: | ---: |
| Plain scrollback | 36.8 MiB/s | 49.6 MiB/s | 0.74x |
| Styled TUI redraw | 72.3 MiB/s | 68.0 MiB/s | 1.06x |
| Unicode + wide cells | 46.9 MiB/s | 60.9 MiB/s | 0.77x |
| Cursor + line edits | 3.2 MiB/s | 3.3 MiB/s | 0.98x |

The repository's `Tmon vs Alacritty Benchmark` workflow remains the authority
for a Linux after-table. It runs on every push, is manually dispatchable, and
uploads `tmon-alacritty-benchmark.txt`.

## Benchmark and profile audit

Both engines receive identical bytes, a 120x40 grid, a 10,000-line history
limit, alternating sample order, and release builds. The current default is
eight samples; odd values are rejected so every workload contains the same
number of Tmon-first and Alacritty-first pairs. Before any timed samples, the
benchmark feeds up to 32 MiB of every workload into fresh terminals and compares
every normalized grid and scrollback cell, cursor state, scroll state,
alternate-screen mode, and bracketed-paste mode. The saved metric uses the same
small payload-sized public `feed_output` calls as the baseline.

Parser decisions use the median of the eight per-sample Tmon/Alacritty ratios,
not the ratio of two independently aggregated medians. The report preserves the
individual pair ratios and their order, prints actual ratios and committed
targets to three decimals, and rejects non-finite target values. GitHub Actions
enforces the four parser/grid targets after the benchmark has written its
output.

Snapshot throughput remains report-only. The report shows retention against the
supplied 19.880x baseline, but does not enforce the tempting
`19.880x * 0.95 = 18.886x` threshold. That gate is not sound on hosted CI: Tmon
copies raw cells while `termy_core` constructs a `TermyFrame`, so the numerator
and denominator are different operations whose relative cost can move with the
runner. Retention is a trend to investigate, not an equivalent-work pass/fail
claim.

The Actions report is initialized under `runner.temp` before checkout. The
timed comparison and subsequent allocation-only pass append through Bash
`pipefail` pipelines, and an `always()` finalizer records the outcome of
checkout, toolchain setup, cache setup, the comparison, and allocation
accounting. If the runner remains available, the report is still added to the
job summary and uploaded even when an earlier step fails. Allocation accounting
also caps total parser work at 1,024 MiB, so an invalid oversized manual input
fails quickly enough for the finalizer to preserve the report.

It is an integrated runtime comparison, not a raw-parser comparison. Tmon feeds
its engine directly. Alacritty is exercised through `termy_core`, including its
locking and graphics-interception boundary. The payload calls are also smaller
than normal PTY reads. These constraints are now printed in the report rather
than hidden by a broader claim.

Pre-change Time Profiler samples identified these hotspots:

- Plain: `put_char` 28.1% leaf time, row shifting 17.5%, `write_cell_at` 15.4%,
  and parser advance 15.1%. Parser advance was 94.5% inclusive.
- Unicode: `put_char` 25.2%, parser advance 21.4%, row shifting 17.4%, cell
  writes 10.9%, and UTF-8 conversion 4.9%.
- Edits: erase-display 68.5%, upward row shifts 23.1%, and downward shifts
  1.6%.

## Accepted changes

1. Full damage no longer clears every partial-damage slot repeatedly. Cleanup
   happens once when the full damage snapshot is consumed.
2. Upward and downward row shifts rotate and reuse displaced row vectors
   directly. The two temporary vectors on every upward shift and the temporary
   vector on every downward shift are gone.
3. Ground-state text is decoded as valid UTF-8 spans. Malformed data is handled
   in the same linear pass through the existing decoder.
4. `Attributes` and structural cell booleans are packed into two one-byte
   bitfields. This made Tmon cells the same 24-byte size as Alacritty cells.
5. Printable ASCII is written in safe row-local runs. The scalar path still
   handles insert mode, deferred wrapping, charset translation, wide-cell
   cleanup, and every complex destination.

Two adversarial quadratic cases found during review were fixed: long malformed
UTF-8 runs and long ASCII spans in insert/no-wrap modes. No unsafe Rust or new
dependency was introduced.

An erase-display experiment that tried to skip fills of already blank rows was
discarded. Its first version fell to 3.3 MiB/s on edits; an exact-first revision
reached 4.4 MiB/s versus the retained 4.5 MiB/s at that stage.

## Follow-up awaiting CI measurement

The saved after-table predates a small static hot-path follow-up. Printable
ASCII at the start of a ground-state batch now bypasses the control-span scan
and UTF-8 validation pass, wide-cell replacement reuses one scalar-width
calculation, and OSC 8 explicit identities use a reverse hash index. Hyperlink
garbage collection is also amortized after 4,096 live links instead of scanning
the full primary, alternate, and scrollback grids for every later link.

These changes are intentionally not folded into the measured table above. The
next GitHub Actions benchmark run is the acceptance measurement; no newer local
benchmark was run. If the parser ratios or Tmon full-frame regression trend
move backward there, the focused changes should be reconsidered independently.

PTY control traffic was hardened separately from the parser benchmark. Resize
requests now coalesce out of band and close is a sticky cancellation signal, so
neither waits behind a saturated input queue. A deterministic PTY test fills
the old queue-sized backlog, verifies that the newest resize reaches the child,
and confirms that every accepted input block remains in FIFO order. This is a
correctness and tail-latency safeguard; the saved parser/grid table does not
measure it.

## Allocation accounting

Whole-process allocator requests are now measured by a separate, report-only
pass after the timed comparison. The `benchmark-allocations` feature installs a
`System` wrapper only in the example binary and requires
`TMON_BENCH_ALLOCATIONS_ONLY=1`; a feature build refuses timed mode, while a
normal build rejects allocation-only mode. The normal throughput executable
therefore contains no allocator-counter overhead.

For each engine/workload pair, the pass warms 2 MiB on an uncounted terminal,
constructs a fresh terminal while counting is disabled, resets and enables the
counter with an unwind-safe non-nestable guard, and counts only the same
complete-payload `feed_output` loop used by timed samples. It reports the exact
processed byte count, which can be slightly above the configured target. It
atomically closes registration and waits for in-flight allocator hooks before
observing a snapshot and before dropping the terminal. The output reports raw
allocation totals (including zeroed allocations),
deallocation totals, and successful realloc request totals plus a signed net
requested-byte change; no allocation threshold is enforced.

This is scoped allocator-request accounting, not a memory-usage measurement.
The process-global wrapper also sees concurrent allocator traffic while the
guard is active, and the signed net is requested bytes in that interval, not
RSS, retained or usable heap, allocator metadata, or physical memory. Terminal
construction, the warmup, snapshot creation, and terminal destruction are
deliberately outside the count.

Before this path existed, macOS Allocations could not attach to the profiling
binary under the current System Integrity Protection configuration. The static
hot-path estimates from that earlier audit remain useful context:

| Workload | Removed temporary row-vector allocations per 32 MiB sample |
| --- | ---: |
| Plain scrollback | about 1.20 million |
| Styled TUI redraw | none from row shifting |
| Unicode + wide cells | about 1.22 million |
| Cursor + line edits | about 3.17 million |

The counts follow directly from the old paths: two temporary vectors per
upward row shift and one per downward shift. Parser span processing and packed
flags allocate nothing. Normal rows are still allocated while history first
grows, then evicted row storage is reused.

## Correctness and compatibility

Coverage added or exercised includes scrollback rollover, multi-row shifts,
damage reset, packed flag independence, malformed and split UTF-8, long ASCII
slow modes, ASCII/scalar equivalence through wrap and scrollback, wide-cell
fallback, and REP after a batched span. Existing randomized parser invariants
and deterministic Alacritty differential traces remain enabled.

Packing is an intentional source-level API change in this experimental crate:
direct flag fields become getters/setters, and full external `Cell` struct
literals must become `Cell::default()` followed by field/setter updates. Public
setters retain the ability to construct every former flag state.

No terminal behavior was deliberately removed or weakened for speed. The only
accepted compatibility tradeoff is the source-level `Cell` construction change
above; the engine remains experimental and its broader VT/graphics parity is
tracked separately from these benchmark results.

## Remaining hotspots

- `put_char`, cell writes, and row movement remain the largest plain/Unicode
  costs. Future changes should be driven by fresh per-workload profiles because
  the current targets already clear their required margins.
- Erase-display and row shifts still dominate the edit trace. The rejected
  blank-row shortcut shows that extra state checks can cost more than a bulk
  fill, so another attempt needs an adversarial differential test and repeatable
  measurements.
- The two full-frame APIs perform different work. A truly comparable snapshot
  benchmark needs a shared renderer-neutral output contract; the current ratio
  must not be presented as a raw engine advantage.
- Normal full-viewport scrolls currently become full damage at the desktop
  adapter, so the renderer rebuilds the visible rows. Optimizing this needs a
  renderer-inclusive benchmark and an ordered scroll-cache design; the current
  parser/grid suite cannot validate the tradeoff.
- Whole-process allocator requests are counted only during the allocation
  pass's `feed_output` scopes. RSS, retained heap, allocator fragmentation and
  metadata, terminal construction/destruction, snapshots, PTY I/O, and renderer
  allocations remain outside what this report can claim.
- Shared GitHub runners report a median and range but cannot guarantee CPU
  frequency or isolation. Cross-push results are useful trend signals, not
  laboratory-grade absolute measurements.
