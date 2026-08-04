# Tmon performance report

Date: 2026-08-04

This report covers the first focused optimization pass over Tmon. The original
Linux numbers came from commit `03d7ca5c5420ca141afe56f725341faadb71af18`.
All before/after percentages below use an additional same-machine macOS
baseline so host differences are not presented as engine improvements.

## Result

Release benchmark settings: 120x40 grid, 10,000 scrollback lines, 32 MiB per
parser sample, 7 samples, and 250 full-frame API calls per snapshot sample.

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
The benchmark output now labels this explicitly. The useful regression check is
within Tmon itself; its throughput improved instead of consuming the allowed 5%
regression budget.

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
limit, alternating sample order, and release builds. The saved metric uses the
same small payload-sized public `feed_output` calls as the baseline.

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

## Allocation accounting

macOS Allocations could not attach to the profiling binary under the current
System Integrity Protection configuration. Tmon did not add an unsafe global
allocator solely to manufacture a counter, so the figures below are static
hot-path counts rather than whole-process allocation totals.

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
