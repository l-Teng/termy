# Tmon

Tmon is Termy's lightweight terminal engine, located in the `crates/tmon`
directory so its implementation stays independent of GPUI and the desktop
application.

The engine has no production Cargo dependencies. It owns its incremental VT
parser, bounded primary/alternate
grids and scrollback, damage tracking, renderer-neutral cell types, and native
process lifecycle. It does not depend on `termy_core` or GPUI. Native process
I/O uses direct platform APIs: a PTY backend on Linux, Android, and macOS, plus
a dependency-free Windows ConPTY backend whose entry points are loaded
dynamically. PNG and Kitty graphics use Tmon's safe, portable, std-only
zlib/DEFLATE decoder.

## Owner

This crate owns the Tmon parser, grids and scrollback, terminal state, damage
model, graphics protocol, and native PTY/ConPTY lifecycle. It must remain
renderer-neutral and independently usable without Termy's desktop application
or `termy_core`.

## Validation

```sh
cargo test -p tmon
cargo clippy -p tmon --all-targets -- -D warnings
```

The isolated three-engine benchmark harness lives in
`tools/tmon-ghostty-memory`, outside the workspace dependency graph. It compares
direct Tmon, Alacritty, and libghostty-vt adapters without making either
competitor a product dependency.

## Forbidden Dependencies

- `gpui` / `gpui_platform`
- `termy_core`
- `termy_terminal_ui`
- `termy_ui`
- `termy_ffi`
- `termy` / `crates/desktop_app`

`alacritty_terminal` and `unicode-width` are development-only dependencies for
semantic parity checks; Tmon's production dependency section remains empty.

Run the desktop app with the default Tmon engine:

```sh
just run
```

Open Termy's inspector and select the **Terminal** tab; the **Engine** row reads
`tmon` for native and display-only terminals. Startup logs and desktop benchmark
summaries record the same engine and selection reason.

Tmux pane displays also use Tmon through `termy_core::Terminal` display mode.
Windows Tmon CI launches live ConPTY children directly through Tmon and verifies
final-output draining before the exit callback, immediate resize plus stdin
round-trip, and batch-wrapper launch in a normalized working directory. It does
not exercise GPUI rendering through a live Windows terminal. Missing platform
APIs, backend initialization failures, and invalid launch or configuration data
are returned to the caller.

## Direction

- Keep the engine independent of GPUI.
- Keep one bounded grid allocation and bounded scrollback.
- Emit damage-scoped frame updates rather than full-grid redraws.
- Keep PTY reads and parsing off the UI thread.
- Keep expanding VT/OSC/DCS and graphics compatibility under differential tests.
- Differential-test common shells and TUIs against isolated reference engines.

Renderers that can move cached rows use `take_render_damage_snapshot()`. Its
scroll operations are chronological and its partial spans describe final
viewport coordinates, so consumers replay every scroll before patching cells.
The accompanying generation must still match when cells are visited; otherwise
the consumer rebuilds a fresh frame. The older `take_damage_snapshot()` remains
source-compatible and deliberately reports scrolls as full damage for callers
that do not understand row movement.

## Compatibility boundary

Tmon is differential-tested against a pinned, dev-only Alacritty oracle for the
VT sequences that oracle implements. It intentionally implements several
standards and xterm extensions that the pinned parser ignores: legacy
alternate-screen/cursor modes 47, 1047, and 1048; DEC selective erase and
DECST8C tab-stop reset; ISO 2022 G1/G2/G3 designation and single shifts; and
DECRQSS/XTGETTCAP replies. OSC 52 additionally accepts valid unpadded or
whitespace-separated base64. These are compatibility extensions, not benchmark
shortcuts, and are tested separately from strict differential fixtures.

Tmon also keeps combining marks available to selection and search. The dev-only
oracle omits that out-of-line text, so combining behavior has focused native
tests instead of being normalized away. OSC/DCS payloads are bounded to
64 KiB so an unterminated control string cannot grow without limit.
Combining text retained by one cell is capped at 4 KiB on a UTF-8 boundary.
An OSC 8 link may retain at most 64 KiB of protocol-ID-plus-target payload, and
all retained hyperlink records share a 2 MiB logical-storage budget. If pruning
unreachable links cannot admit a new link, Tmon closes the hyperlink pen while
keeping existing live links resolvable. Repeated rejected links back off
full-grid pruning exponentially. Each backoff window permits one early recovery
probe after roots change, so reclaimed capacity returns promptly without letting
root churn force an unbounded scan loop.
Width reflow is differential-tested while scrolled and while changing rows and
columns together. Tmon additionally keeps its public one-column grid finite for
wide characters, even though the pinned Alacritty engine supports a minimum of
two columns.
Standalone Tmon keeps the conservative `Osc52::OnlyCopy` default; the Termy
desktop adapter explicitly enables `CopyPaste` so OSC 52 clipboard queries match
the existing native engine and still pass through the application's reply host.
Lifecycle events for shell prompt, command start, execution, and completion have
queue priority over discardable noise, so a bounded event queue preserves a
coherent command cycle under load.

Kitty graphics uploads and inflated image data are capped at 64 MiB per image,
while stored PNG data is kept under a separate 128 MiB cache quota and a 4,096
image-record limit. Raw RGB and RGBA uploads are encoded directly into the final
PNG allocation, avoiding a second full-size filtered or zlib staging buffer.
The limits still admit a 4096x4096 RGBA image. Command controls stop at 4 KiB and
64 fields before string conversion. Valid supplied PNGs are compacted in place
to remove compressed text/color-profile metadata and APNG frames that could
make a downstream image decoder expand data outside the validated static-image
bound. DEFLATE decoding also caps the number of blocks, and chunked transfers
check their remaining decoded capacity before allocating the next chunk. A PNG
with one IDAT chunk is validated directly from its upload buffer; only split
IDAT streams need a joined compressed buffer. File
transfers reject non-regular files before opening them and use nonblocking opens
on supported Unix targets, so a child-provided FIFO cannot stall parsing.
Kitty image numbers (`I`) allocate distinct terminal-selected IDs, resolve only
the newest matching image, and are rejected when combined with an explicit
image ID. Uppercase deletion frees data only for images affected by that
selector and only after their final placement is gone. Image-ID range deletion
is global, while coordinate selectors remain scoped to the active screen.

The Unix PTY captures bytes readable when the direct child exits, then uses a
short bounded nonblocking drain to retain final output without allowing a live
descendant to delay exit forever. Dropping a live Unix PTY sends hangup, allows
a 250 ms grace period, then escalates to `SIGKILL` so a child that ignores
hangup cannot leak indefinitely. Windows uses separate synchronous ConPTY
reader and writer paths and keeps draining output through shutdown. On both
platforms, queued plus active normal input is capped at 8 MiB and 4,096 writes,
including retained `Vec` capacity. A separate priority lane reserves 2 MiB and
256 entries for terminal protocol replies, including OSC 52 query responses.
Replies can preempt normal input at bounded 16 KiB write boundaries; interrupted
normal input resumes at the same offset and keeps its FIFO order. If even the
priority lane cannot accept a required reply, Tmon closes the session instead
of silently leaving a child waiting forever. Resize and close notifications
coalesce to a single wake while the newest resize and sticky close state remain
out of band, so control traffic cannot grow an unbounded queue or wait behind
the input backlog. Rust hosts that need failure handling use `try_write`,
`try_write_owned`, `try_resize`, and `try_nudge_resize`; the original void
methods remain compatibility wrappers. A live resize waits for the platform PTY
to acknowledge the new dimensions before changing the in-memory grid, so a
failed resize leaves both sides at the previous size. Synchronized-update
expiry uses one shared scheduler with at most one queued watchdog task per
engine instead of spawning or retaining an unbounded task stream.

Extended underline state is preserved rather than collapsed to a boolean. Tmon
tracks single, double, curly, dotted, and dashed SGR 4 styles plus indexed and
RGB SGR 58 colors; SGR 24, 59, and 0 reset the same independent pieces as the
pinned VTE parser. Underline color remains directly readable from a copied cell.
Combining text and OSC 8 identity share one tagged rare-metadata word, keeping
`Cell` at 24 bytes even when both features are supported together.
The desktop core adapter carries that state into Termy's renderer for native and
tmux cells: single and curly underlines use GPUI's native straight/wavy
decoration, while double, dotted, and dashed styles use bounded batched paths.

## Compact scrollback

Primary and alternate screens remain dense so parsing and rendering keep direct
cell access. A row is compacted only when it enters primary-screen scrollback:
its logical width and stored cell count are kept separately, an exact-default
suffix is omitted, and ordinary characters are stored directly as UTF-8.
Sparse bytecode commands carry style transitions, structural cell state, wide
spacers, inline combining marks, and rare metadata. Colored and attributed
rows use the same lossless encoding instead of falling back to 24-byte cells.

History reads use a logical row view that supplies default cells for the omitted
suffix and can visit cells without allocating. Resize and reflow materialize
dense storage only while rebuilding rows; cross-row wide-spacer repair updates
the compact encoding immediately. Evicted byte storage is reused, pooled
combining metadata is released with its owning row, and
clearing or shrinking history releases unused compact storage. Live row extents
avoid rescanning known-default tails when scrolling without changing the dense
live-grid representation.

For complete default-style `CRLF` batches, a guarded parser/grid path moves cold
primary rows directly into compact history and rebuilds only the final
viewport. On the alternate screen, the ASCII form rotates and rewrites the
dense rows in place without allocating or recording history. The primary path
also handles validated UTF-8 rows with wide cells and one inline combining mark
per cell. It activates only at the bottom of a full-screen scroll region with
default modes and style; partial or more complex input, custom charsets, and
every ambiguous state keep using the normal scalar path. Complete ordinary CSI
sequences also bypass the byte-at-a-time state machine; private mode changes
stay on the scalar path so synchronized-update boundaries remain unchanged.

The ten-sample macOS ARM64 results and tradeoffs are recorded in
[PERFORMANCE.md](PERFORMANCE.md). The compact uncompressed representation meets
the current memory target, so no cold-page compression or new dependency is
included.

## Benchmarking

The standalone macOS harness compares direct Tmon with isolated
`alacritty_terminal` and pinned, statically linked `libghostty-vt` adapters.
Neither competitor is linked into Termy. Memory mode uses fresh child processes;
throughput mode uses identical inputs, chunks, warmups, calibrated byte targets,
and balanced six-permutation sample ordering. An untimed preflight compares
cursor, history, screen state, and every logical cell before timing.

```sh
GHOSTTY_DIR=../ghostty just benchmark-tmon-ghostty-memory
```

The **Tmon, Alacritty, and Ghostty Benchmark** workflow runs the same isolated
harness. The comparison measures parser/grid throughput and engine-state memory,
not PTY I/O, GPUI painting, input latency, or complete terminal compatibility.
Speed ratios are not a feature-parity score.

The revision gate is a different comparison: current Tmon and the pinned
reference call the same native `Terminal::snapshot()` API on identical terminal
state in one process. Before timing, an exact preflight verifies each actual
snapshot against independently visited terminal state and the other revision.
Twenty balanced interleaved blocks run each side for at least 250 ms per
measurement; the gate uses conservative order statistics over paired throughput
ratios. Current Tmon passes only when the lower bound is at least 95% of the
immutable reference. A clear regression or an inconclusive noisy result fails
CI instead of silently accepting uncertainty. The reference SHA is ratcheted
only by an explicit source change after a reviewed measurement, never to the
previous push automatically.
