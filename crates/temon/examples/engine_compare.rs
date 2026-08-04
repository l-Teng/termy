use std::{env, hint::black_box, mem::size_of, time::Instant};

#[cfg(feature = "benchmark-allocations")]
mod allocation_counter {
    use std::{
        alloc::{GlobalAlloc, Layout, System},
        sync::atomic::{AtomicU8, AtomicU64, Ordering},
    };

    const DISABLED: u8 = 0;
    const RESETTING: u8 = 1;
    const ENABLED: u8 = 2;
    const STOPPING: u8 = 3;

    const HOOK_ENABLED: u64 = 1 << 63;
    const HOOK_COUNT_MASK: u64 = u32::MAX as u64;
    const HOOK_GENERATION_STEP: u64 = 1 << 32;
    const HOOK_GENERATION_MASK: u64 = !(HOOK_ENABLED | HOOK_COUNT_MASK);

    static STATE: AtomicU8 = AtomicU8::new(DISABLED);
    // The high bit enables registration, the next 31 bits distinguish
    // measurement generations, and the low 32 bits count registered hooks.
    // Keeping generation and count in one atomic prevents an old hook from
    // registering into a later measurement after the gate briefly closed.
    static HOOK_GATE: AtomicU64 = AtomicU64::new(0);
    static ALLOC_CALLS: AtomicU64 = AtomicU64::new(0);
    static ALLOC_BYTES: AtomicU64 = AtomicU64::new(0);
    static DEALLOC_CALLS: AtomicU64 = AtomicU64::new(0);
    static DEALLOC_BYTES: AtomicU64 = AtomicU64::new(0);
    static REALLOC_CALLS: AtomicU64 = AtomicU64::new(0);
    static REALLOC_OLD_BYTES: AtomicU64 = AtomicU64::new(0);
    static REALLOC_NEW_BYTES: AtomicU64 = AtomicU64::new(0);

    struct CountingSystem;

    #[global_allocator]
    static GLOBAL_ALLOCATOR: CountingSystem = CountingSystem;

    fn register_hook_from(mut gate: u64) -> bool {
        let generation = gate & HOOK_GENERATION_MASK;
        loop {
            if gate & HOOK_ENABLED == 0 {
                return false;
            }
            if gate & HOOK_GENERATION_MASK != generation {
                return false;
            }
            if gate & HOOK_COUNT_MASK == HOOK_COUNT_MASK {
                return false;
            }
            match HOOK_GATE.compare_exchange_weak(
                gate,
                gate + 1,
                Ordering::AcqRel,
                Ordering::Acquire,
            ) {
                Ok(_) => return true,
                Err(current) => gate = current,
            }
        }
    }

    fn register_hook() -> bool {
        register_hook_from(HOOK_GATE.load(Ordering::Acquire))
    }

    fn unregister_hook() {
        HOOK_GATE.fetch_sub(1, Ordering::AcqRel);
    }

    fn disable_and_wait_for_hooks() {
        STATE.store(STOPPING, Ordering::Release);
        HOOK_GATE.fetch_and(!HOOK_ENABLED, Ordering::AcqRel);
        while HOOK_GATE.load(Ordering::Acquire) & HOOK_COUNT_MASK != 0 {
            std::hint::spin_loop();
        }
        STATE.store(DISABLED, Ordering::Release);
    }

    // SAFETY: Every hook delegates to `System` with the caller-provided pointer,
    // layout, and size unchanged. The additional bookkeeping is lock-free atomic
    // arithmetic and cannot call back into the allocator. Hooks register before
    // delegating, and measurement shutdown waits for every registered hook before
    // reading the counters.
    unsafe impl GlobalAlloc for CountingSystem {
        unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
            let counted = register_hook();
            // SAFETY: `GlobalAlloc::alloc` guarantees that `layout` is valid, and
            // it is forwarded to the system allocator without modification.
            let pointer = unsafe { System.alloc(layout) };
            if counted && !pointer.is_null() {
                ALLOC_CALLS.fetch_add(1, Ordering::Relaxed);
                ALLOC_BYTES.fetch_add(layout.size() as u64, Ordering::Relaxed);
            }
            if counted {
                unregister_hook();
            }
            pointer
        }

        unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
            let counted = register_hook();
            // SAFETY: `GlobalAlloc::alloc_zeroed` guarantees that `layout` is
            // valid, and it is forwarded to `System` without modification.
            let pointer = unsafe { System.alloc_zeroed(layout) };
            if counted && !pointer.is_null() {
                ALLOC_CALLS.fetch_add(1, Ordering::Relaxed);
                ALLOC_BYTES.fetch_add(layout.size() as u64, Ordering::Relaxed);
            }
            if counted {
                unregister_hook();
            }
            pointer
        }

        unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
            let counted = register_hook();
            // SAFETY: `GlobalAlloc::dealloc` guarantees that `pointer` and
            // `layout` describe a live allocation from this allocator. Both are
            // forwarded to `System` without modification.
            unsafe { System.dealloc(pointer, layout) };
            if counted {
                DEALLOC_CALLS.fetch_add(1, Ordering::Relaxed);
                DEALLOC_BYTES.fetch_add(layout.size() as u64, Ordering::Relaxed);
                unregister_hook();
            }
        }

        unsafe fn realloc(&self, pointer: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
            let counted = register_hook();
            // SAFETY: `GlobalAlloc::realloc` guarantees that `pointer`, `layout`,
            // and `new_size` satisfy its contract. All three arguments are
            // forwarded to `System` without modification.
            let new_pointer = unsafe { System.realloc(pointer, layout, new_size) };
            if counted && !new_pointer.is_null() {
                REALLOC_CALLS.fetch_add(1, Ordering::Relaxed);
                REALLOC_OLD_BYTES.fetch_add(layout.size() as u64, Ordering::Relaxed);
                REALLOC_NEW_BYTES.fetch_add(new_size as u64, Ordering::Relaxed);
            }
            if counted {
                unregister_hook();
            }
            new_pointer
        }
    }

    #[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
    pub(super) struct AllocationSnapshot {
        pub(super) alloc_calls: u64,
        pub(super) alloc_bytes: u64,
        pub(super) dealloc_calls: u64,
        pub(super) dealloc_bytes: u64,
        pub(super) realloc_calls: u64,
        pub(super) realloc_old_bytes: u64,
        pub(super) realloc_new_bytes: u64,
    }

    impl AllocationSnapshot {
        pub(super) fn net_requested_bytes(self) -> i128 {
            self.alloc_bytes as i128 + self.realloc_new_bytes as i128
                - self.dealloc_bytes as i128
                - self.realloc_old_bytes as i128
        }
    }

    fn reset() {
        ALLOC_CALLS.store(0, Ordering::Relaxed);
        ALLOC_BYTES.store(0, Ordering::Relaxed);
        DEALLOC_CALLS.store(0, Ordering::Relaxed);
        DEALLOC_BYTES.store(0, Ordering::Relaxed);
        REALLOC_CALLS.store(0, Ordering::Relaxed);
        REALLOC_OLD_BYTES.store(0, Ordering::Relaxed);
        REALLOC_NEW_BYTES.store(0, Ordering::Relaxed);
    }

    fn snapshot() -> AllocationSnapshot {
        AllocationSnapshot {
            alloc_calls: ALLOC_CALLS.load(Ordering::Relaxed),
            alloc_bytes: ALLOC_BYTES.load(Ordering::Relaxed),
            dealloc_calls: DEALLOC_CALLS.load(Ordering::Relaxed),
            dealloc_bytes: DEALLOC_BYTES.load(Ordering::Relaxed),
            realloc_calls: REALLOC_CALLS.load(Ordering::Relaxed),
            realloc_old_bytes: REALLOC_OLD_BYTES.load(Ordering::Relaxed),
            realloc_new_bytes: REALLOC_NEW_BYTES.load(Ordering::Relaxed),
        }
    }

    pub(super) struct AllocationMeasurement {
        active: bool,
    }

    impl AllocationMeasurement {
        pub(super) fn begin() -> Result<Self, &'static str> {
            STATE
                .compare_exchange(DISABLED, RESETTING, Ordering::AcqRel, Ordering::Acquire)
                .map_err(|_| "an allocation measurement is already active")?;
            let gate = HOOK_GATE.load(Ordering::Acquire);
            if gate & (HOOK_ENABLED | HOOK_COUNT_MASK) != 0 {
                STATE.store(DISABLED, Ordering::Release);
                return Err("allocator hooks from the previous measurement are still active");
            }
            reset();
            let generation = gate & HOOK_GENERATION_MASK;
            let next_generation =
                generation.wrapping_add(HOOK_GENERATION_STEP) & HOOK_GENERATION_MASK;
            HOOK_GATE.store(next_generation | HOOK_ENABLED, Ordering::Release);
            STATE.store(ENABLED, Ordering::Release);
            Ok(Self { active: true })
        }

        pub(super) fn finish(mut self) -> AllocationSnapshot {
            disable_and_wait_for_hooks();
            self.active = false;
            snapshot()
        }
    }

    impl Drop for AllocationMeasurement {
        fn drop(&mut self) {
            if self.active {
                disable_and_wait_for_hooks();
            }
        }
    }

    #[cfg(test)]
    pub(super) fn register_hook_for_test() -> bool {
        register_hook()
    }

    #[cfg(test)]
    pub(super) fn unregister_hook_for_test() {
        unregister_hook();
    }

    #[cfg(test)]
    pub(super) fn gate_for_test() -> u64 {
        HOOK_GATE.load(Ordering::Acquire)
    }

    #[cfg(test)]
    pub(super) fn register_hook_from_for_test(gate: u64) -> bool {
        register_hook_from(gate)
    }

    #[cfg(test)]
    pub(super) fn stopping_for_test() -> bool {
        STATE.load(Ordering::Acquire) == STOPPING
            && HOOK_GATE.load(Ordering::Acquire) & HOOK_ENABLED == 0
    }
}

use alacritty_terminal::{
    grid::Dimensions,
    index::Line,
    term::cell::{Cell as AlacrittyCell, Flags},
    vte::ansi::{Color as AlacrittyColor, NamedColor},
};
use termy_core::{
    Terminal as AlacrittyTerminal, TerminalCursorStyle, TerminalRuntimeConfig,
    TerminalSize as AlacrittySize,
};
use tmon::{
    Combining as TmonCombining, Config as TmonConfig, CursorStyle as TmonCursorStyle,
    Size as TmonSize, Terminal as TmonTerminal,
};

const MIB: usize = 1024 * 1024;
const COLS: u16 = 120;
const ROWS: u16 = 40;
const SCROLLBACK: usize = 10_000;
const SNAPSHOT_PREFILL_BYTES: usize = 2 * MIB;
const VALIDATION_BYTES_MAX: usize = 32 * MIB;
const MAX_BENCHMARK_MIB_PER_ENGINE: usize = 2_048;
#[cfg(feature = "benchmark-allocations")]
const MAX_ALLOCATION_MIB_TOTAL: usize = 1_024;
const SUPPLIED_SNAPSHOT_RATIO_BASELINE: f64 = 19.88;
#[cfg(feature = "benchmark-allocations")]
const ALLOCATION_WARMUP_BYTES: usize = 2 * MIB;

struct Workload {
    id: &'static str,
    name: &'static str,
    payload: &'static str,
    minimum_ratio: f64,
}

const WORKLOADS: [Workload; 4] = [
    Workload {
        id: "plain",
        name: "plain scrollback",
        payload: "the quick brown fox jumps over the lazy dog 0123456789\r\n",
        minimum_ratio: 1.05,
    },
    Workload {
        id: "styled",
        name: "styled TUI redraw",
        payload: concat!(
            "\x1b[H\x1b[38;2;120;180;255;48;5;234;1mstatus\x1b[0m",
            "\x1b[2;1H\x1b[Kbuild complete\x1b[3;1Hprogress: 100%",
        ),
        minimum_ratio: 1.05,
    },
    Workload {
        id: "unicode",
        name: "Unicode + wide cells",
        payload: "ASCII café Ελληνικά 日本語 한글 🙂界\r\n",
        minimum_ratio: 1.00,
    },
    Workload {
        id: "edits",
        name: "cursor + line edits",
        payload: "\x1b[10;20Habcdef\x1b[3DXYZ\x1b[K\x1b[4;1H\x1b[2Linserted\x1b[1M\x1b[2J\x1b[H",
        minimum_ratio: 1.00,
    },
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Engine {
    Tmon,
    Alacritty,
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct Stats {
    median: f64,
    min: f64,
    max: f64,
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct PairedSample {
    first: Engine,
    tmon: f64,
    alacritty: f64,
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct PairedStats {
    tmon: Stats,
    alacritty: Stats,
    ratio: Stats,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum RawColor {
    Default,
    Indexed(u8),
    Rgb(u8, u8, u8),
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct SemanticCell {
    character: char,
    combining: String,
    foreground: RawColor,
    background: RawColor,
    bold: bool,
    dim: bool,
    italic: bool,
    underline: bool,
    inverse: bool,
    hidden: bool,
    strikethrough: bool,
    hyperlink: bool,
    wide_spacer: bool,
    wrapped: bool,
}

fn setting(name: &str, default: usize, min: usize, max: usize) -> usize {
    let Ok(raw) = env::var(name) else {
        return default;
    };
    let value = raw
        .parse::<usize>()
        .unwrap_or_else(|_| panic!("{name} must be an integer, got {raw:?}"));
    assert!(
        (min..=max).contains(&value),
        "{name} must be between {min} and {max}, got {value}"
    );
    value
}

fn require_even_sample_count(sample_count: usize) -> usize {
    assert!(
        sample_count.is_multiple_of(2),
        "TMON_BENCH_SAMPLES must be even so every workload has balanced Tmon-first and \
         Alacritty-first pairs, got {sample_count}"
    );
    sample_count
}

fn sample_order(index: usize, tmon_first: bool) -> [Engine; 2] {
    if index.is_multiple_of(2) == tmon_first {
        [Engine::Tmon, Engine::Alacritty]
    } else {
        [Engine::Alacritty, Engine::Tmon]
    }
}

fn validate_target_ratio(target: f64) -> Result<(), &'static str> {
    if !target.is_finite() {
        return Err("target ratio must be finite");
    }
    if target <= 0.0 {
        return Err("target ratio must be greater than zero");
    }
    Ok(())
}

fn meets_target(actual: f64, target: f64) -> bool {
    validate_target_ratio(target).unwrap_or_else(|reason| panic!("{reason}, got {target:?}"));
    actual.is_finite() && actual >= target
}

fn tmon_terminal() -> TmonTerminal {
    TmonTerminal::new_display(
        TmonSize {
            cols: COLS,
            rows: ROWS,
            ..TmonSize::default()
        },
        TmonConfig {
            scrollback_history: SCROLLBACK,
            ..TmonConfig::default()
        },
    )
}

fn alacritty_terminal() -> AlacrittyTerminal {
    let config = TerminalRuntimeConfig {
        scrollback_history: SCROLLBACK,
        ..TerminalRuntimeConfig::default()
    };
    AlacrittyTerminal::new_display(
        AlacrittySize {
            cols: COLS,
            rows: ROWS,
            ..AlacrittySize::default()
        },
        Some(&config),
    )
}

#[cfg(feature = "benchmark-allocations")]
fn feed_tmon_target(terminal: &TmonTerminal, payload: &[u8], target_bytes: usize) {
    for _ in 0..iterations_for(target_bytes, payload) {
        terminal.feed_output(black_box(payload));
    }
}

#[cfg(feature = "benchmark-allocations")]
fn feed_alacritty_target(terminal: &AlacrittyTerminal, payload: &[u8], target_bytes: usize) {
    for _ in 0..iterations_for(target_bytes, payload) {
        terminal.feed_output(black_box(payload));
    }
}

#[cfg(feature = "benchmark-allocations")]
fn warm_up_allocation_path(engine: Engine, workload: &Workload) {
    let payload = workload.payload.as_bytes();
    match engine {
        Engine::Tmon => {
            let terminal = tmon_terminal();
            feed_tmon_target(&terminal, payload, ALLOCATION_WARMUP_BYTES);
            black_box(&terminal);
        }
        Engine::Alacritty => {
            let terminal = alacritty_terminal();
            feed_alacritty_target(&terminal, payload, ALLOCATION_WARMUP_BYTES);
            black_box(&terminal);
        }
    }
}

#[cfg(feature = "benchmark-allocations")]
fn allocation_sample(
    engine: Engine,
    workload: &Workload,
    target_bytes: usize,
) -> allocation_counter::AllocationSnapshot {
    warm_up_allocation_path(engine, workload);
    let payload = workload.payload.as_bytes();

    match engine {
        Engine::Tmon => {
            let terminal = tmon_terminal();
            let measurement =
                allocation_counter::AllocationMeasurement::begin().unwrap_or_else(|reason| {
                    panic!("could not start allocation measurement: {reason}")
                });
            feed_tmon_target(&terminal, payload, target_bytes);
            let allocations = measurement.finish();
            black_box(terminal.snapshot());
            allocations
        }
        Engine::Alacritty => {
            let terminal = alacritty_terminal();
            let measurement =
                allocation_counter::AllocationMeasurement::begin().unwrap_or_else(|reason| {
                    panic!("could not start allocation measurement: {reason}")
                });
            feed_alacritty_target(&terminal, payload, target_bytes);
            let allocations = measurement.finish();
            black_box(terminal.snapshot());
            allocations
        }
    }
}

fn alacritty_color(color: AlacrittyColor) -> RawColor {
    match color {
        AlacrittyColor::Spec(rgb) => RawColor::Rgb(rgb.r, rgb.g, rgb.b),
        AlacrittyColor::Indexed(index) => RawColor::Indexed(index),
        AlacrittyColor::Named(name) if (name as usize) < 16 => RawColor::Indexed(name as u8),
        AlacrittyColor::Named(
            NamedColor::Foreground
            | NamedColor::Background
            | NamedColor::BrightForeground
            | NamedColor::DimForeground,
        ) => RawColor::Default,
        AlacrittyColor::Named(name) => panic!("unexpected named cell color {name:?}"),
    }
}

fn tmon_color(color: tmon::Color) -> RawColor {
    match color {
        tmon::Color::Default => RawColor::Default,
        tmon::Color::Indexed(index) => RawColor::Indexed(index),
        tmon::Color::Rgb { r, g, b } => RawColor::Rgb(r, g, b),
    }
}

fn alacritty_cell(cell: &AlacrittyCell) -> SemanticCell {
    SemanticCell {
        character: cell.c,
        combining: cell.zerowidth().into_iter().flatten().collect(),
        foreground: alacritty_color(cell.fg),
        background: alacritty_color(cell.bg),
        bold: cell.flags.contains(Flags::BOLD),
        dim: cell.flags.contains(Flags::DIM),
        italic: cell.flags.contains(Flags::ITALIC),
        underline: cell.flags.intersects(Flags::ALL_UNDERLINES),
        inverse: cell.flags.contains(Flags::INVERSE),
        hidden: cell.flags.contains(Flags::HIDDEN),
        strikethrough: cell.flags.contains(Flags::STRIKEOUT),
        hyperlink: cell.hyperlink().is_some(),
        wide_spacer: cell
            .flags
            .intersects(Flags::WIDE_CHAR_SPACER | Flags::LEADING_WIDE_CHAR_SPACER),
        wrapped: cell.flags.contains(Flags::WRAPLINE),
    }
}

fn tmon_cell(cell: &tmon::Cell, combining: Option<TmonCombining<'_>>) -> SemanticCell {
    SemanticCell {
        character: cell.character,
        combining: combining.map_or_else(String::new, TmonCombining::to_owned_string),
        foreground: tmon_color(cell.foreground),
        background: tmon_color(cell.background),
        bold: cell.attributes.bold(),
        dim: cell.attributes.dim(),
        italic: cell.attributes.italic(),
        underline: cell.attributes.underline(),
        inverse: cell.attributes.inverse(),
        hidden: cell.attributes.hidden(),
        strikethrough: cell.attributes.strikethrough(),
        hyperlink: cell.hyperlink_id.is_some(),
        wide_spacer: cell.wide_spacer() || cell.leading_wide_spacer(),
        wrapped: cell.wrapped(),
    }
}

fn assert_workload_equivalence(workload: &Workload, validation_bytes: usize) {
    let tmon = tmon_terminal();
    let alacritty = alacritty_terminal();
    let payload = workload.payload.as_bytes();
    for _ in 0..iterations_for(validation_bytes, payload) {
        tmon.feed_output(payload);
        alacritty.feed_output(payload);
    }

    let (tmon_first, tmon_last) = tmon.line_bounds();
    alacritty.with_term(|term| {
        let grid = term.grid();
        let alacritty_first = -(grid.history_size() as i32);
        let alacritty_last = grid.screen_lines() as i32 - 1;
        assert_eq!(
            (tmon_first, tmon_last),
            (alacritty_first, alacritty_last),
            "{}: full-grid bounds differ",
            workload.name
        );

        for line in alacritty_first..=alacritty_last {
            let alacritty_line = (&grid[Line(line)])
                .into_iter()
                .map(alacritty_cell)
                .collect::<Vec<_>>();
            let mut tmon_line = Vec::with_capacity(alacritty_line.len());
            assert!(
                tmon.for_each_line_cell(line, |_, cell, combining| {
                    tmon_line.push(tmon_cell(cell, combining));
                }),
                "{}: Tmon is missing grid line {line}",
                workload.name
            );
            assert_eq!(
                tmon_line.len(),
                alacritty_line.len(),
                "{}: grid line {line} has a different width",
                workload.name
            );
            if let Some((col, (tmon_cell, alacritty_cell))) = tmon_line
                .iter()
                .zip(&alacritty_line)
                .enumerate()
                .find(|(_, (tmon_cell, alacritty_cell))| tmon_cell != alacritty_cell)
            {
                panic!(
                    "{}: terminal state differs at line {line}, column {col}:\n\
                     Tmon: {tmon_cell:?}\nAlacritty: {alacritty_cell:?}",
                    workload.name
                );
            }
        }
    });

    let tmon_cursor = tmon.cursor_state().map(|cursor| {
        (
            cursor.col,
            cursor.row,
            matches!(cursor.style, TmonCursorStyle::Line),
        )
    });
    let alacritty_cursor = alacritty.cursor_state().map(|cursor| {
        (
            cursor.col,
            cursor.row,
            matches!(cursor.style, TerminalCursorStyle::Line),
        )
    });
    assert_eq!(
        tmon_cursor, alacritty_cursor,
        "{}: cursor state differs",
        workload.name
    );
    assert_eq!(
        tmon.scroll_state(),
        alacritty.scroll_state(),
        "{}: scroll state differs",
        workload.name
    );
    assert_eq!(
        tmon.alternate_screen_mode(),
        alacritty.alternate_screen_mode(),
        "{}: screen mode differs",
        workload.name
    );
    assert_eq!(
        tmon.bracketed_paste_mode(),
        alacritty.bracketed_paste_mode(),
        "{}: bracketed-paste mode differs",
        workload.name
    );
}

fn iterations_for(target_bytes: usize, payload: &[u8]) -> usize {
    target_bytes.div_ceil(payload.len())
}

fn parse_sample(engine: Engine, workload: &Workload, target_bytes: usize) -> f64 {
    let payload = workload.payload.as_bytes();
    let iterations = iterations_for(target_bytes, payload);
    let bytes = iterations * payload.len();

    let elapsed = match engine {
        Engine::Tmon => {
            let terminal = tmon_terminal();
            let started = Instant::now();
            for _ in 0..iterations {
                terminal.feed_output(black_box(payload));
            }
            let elapsed = started.elapsed();
            black_box(terminal.snapshot());
            elapsed
        }
        Engine::Alacritty => {
            let terminal = alacritty_terminal();
            let started = Instant::now();
            for _ in 0..iterations {
                terminal.feed_output(black_box(payload));
            }
            let elapsed = started.elapsed();
            black_box(terminal.snapshot());
            elapsed
        }
    };

    bytes as f64 / MIB as f64 / elapsed.as_secs_f64()
}

fn prefill_tmon(terminal: &TmonTerminal) {
    let payload = WORKLOADS[0].payload.as_bytes();
    for _ in 0..iterations_for(SNAPSHOT_PREFILL_BYTES, payload) {
        terminal.feed_output(payload);
    }
}

fn prefill_alacritty(terminal: &AlacrittyTerminal) {
    let payload = WORKLOADS[0].payload.as_bytes();
    for _ in 0..iterations_for(SNAPSHOT_PREFILL_BYTES, payload) {
        terminal.feed_output(payload);
    }
}

fn snapshot_sample(engine: Engine, snapshots: usize) -> f64 {
    let elapsed = match engine {
        Engine::Tmon => {
            let terminal = tmon_terminal();
            prefill_tmon(&terminal);
            let started = Instant::now();
            for _ in 0..snapshots {
                black_box(terminal.snapshot());
            }
            started.elapsed()
        }
        Engine::Alacritty => {
            let terminal = alacritty_terminal();
            prefill_alacritty(&terminal);
            let started = Instant::now();
            for _ in 0..snapshots {
                black_box(terminal.snapshot());
            }
            started.elapsed()
        }
    };
    snapshots as f64 / elapsed.as_secs_f64()
}

fn stats(samples: &[f64]) -> Stats {
    assert!(
        !samples.is_empty(),
        "cannot calculate statistics without samples"
    );
    assert!(
        samples.iter().all(|sample| sample.is_finite()),
        "benchmark samples must be finite"
    );
    let mut sorted = samples.to_vec();
    sorted.sort_by(f64::total_cmp);
    let middle = sorted.len() / 2;
    let median = if sorted.len().is_multiple_of(2) {
        (sorted[middle - 1] + sorted[middle]) / 2.0
    } else {
        sorted[middle]
    };
    Stats {
        median,
        min: sorted[0],
        max: sorted[sorted.len() - 1],
    }
}

impl PairedSample {
    fn ratio(self) -> f64 {
        assert!(
            self.tmon.is_finite() && self.tmon >= 0.0,
            "Tmon sample must be finite and non-negative, got {:?}",
            self.tmon
        );
        assert!(
            self.alacritty.is_finite() && self.alacritty > 0.0,
            "Alacritty sample must be finite and positive, got {:?}",
            self.alacritty
        );
        let ratio = self.tmon / self.alacritty;
        assert!(ratio.is_finite(), "paired throughput ratio must be finite");
        ratio
    }

    fn order_label(self) -> &'static str {
        match self.first {
            Engine::Tmon => "T/A",
            Engine::Alacritty => "A/T",
        }
    }
}

fn measure_pair(order: [Engine; 2], mut measure: impl FnMut(Engine) -> f64) -> PairedSample {
    let first = order[0];
    let (tmon, alacritty) = match order {
        [Engine::Tmon, Engine::Alacritty] => (measure(Engine::Tmon), measure(Engine::Alacritty)),
        [Engine::Alacritty, Engine::Tmon] => {
            let alacritty = measure(Engine::Alacritty);
            let tmon = measure(Engine::Tmon);
            (tmon, alacritty)
        }
        _ => panic!("a sample pair must measure each engine exactly once"),
    };
    PairedSample {
        first,
        tmon,
        alacritty,
    }
}

fn paired_stats(samples: &[PairedSample]) -> PairedStats {
    let tmon = samples.iter().map(|sample| sample.tmon).collect::<Vec<_>>();
    let alacritty = samples
        .iter()
        .map(|sample| sample.alacritty)
        .collect::<Vec<_>>();
    let ratios = samples
        .iter()
        .copied()
        .map(PairedSample::ratio)
        .collect::<Vec<_>>();
    PairedStats {
        tmon: stats(&tmon),
        alacritty: stats(&alacritty),
        ratio: stats(&ratios),
    }
}

fn format_paired_ratios(samples: &[PairedSample]) -> String {
    samples
        .iter()
        .copied()
        .map(|sample| format!("{} {:.3}x", sample.order_label(), sample.ratio()))
        .collect::<Vec<_>>()
        .join(", ")
}

fn collect_parse_samples(
    workload: &Workload,
    target_bytes: usize,
    sample_count: usize,
    tmon_first: bool,
) -> Vec<PairedSample> {
    let warmup_bytes = target_bytes.min(2 * MIB);
    for engine in sample_order(0, tmon_first) {
        black_box(parse_sample(engine, workload, warmup_bytes));
    }

    let mut samples = Vec::with_capacity(sample_count);
    for index in 0..sample_count {
        samples.push(measure_pair(sample_order(index, tmon_first), |engine| {
            parse_sample(engine, workload, target_bytes)
        }));
    }
    samples
}

fn collect_snapshot_samples(
    snapshot_count: usize,
    sample_count: usize,
    tmon_first: bool,
) -> Vec<PairedSample> {
    for engine in sample_order(0, tmon_first) {
        black_box(snapshot_sample(engine, 10));
    }

    let mut samples = Vec::with_capacity(sample_count);
    for index in 0..sample_count {
        samples.push(measure_pair(sample_order(index, tmon_first), |engine| {
            snapshot_sample(engine, snapshot_count)
        }));
    }
    samples
}

fn run_profile_mode(target_bytes: usize) -> bool {
    let Ok(workload_id) = env::var("TMON_PROFILE_WORKLOAD") else {
        return false;
    };
    let workload = WORKLOADS
        .iter()
        .find(|workload| workload.id == workload_id)
        .unwrap_or_else(|| {
            panic!("TMON_PROFILE_WORKLOAD must be plain, styled, unicode, or edits")
        });
    let engine = match env::var("TMON_PROFILE_ENGINE").as_deref() {
        Ok("alacritty") => Engine::Alacritty,
        Ok("tmon") | Err(_) => Engine::Tmon,
        Ok(other) => panic!("TMON_PROFILE_ENGINE must be tmon or alacritty, got {other:?}"),
    };
    let repetitions = setting("TMON_PROFILE_REPETITIONS", 1, 1, 100);
    println!(
        "Profiling {} with {} MiB of {} output per repetition",
        match engine {
            Engine::Tmon => "Tmon",
            Engine::Alacritty => "Alacritty",
        },
        target_bytes / MIB,
        workload.id,
    );
    for _ in 0..repetitions {
        black_box(parse_sample(engine, workload, target_bytes));
    }
    true
}

#[cfg(feature = "benchmark-allocations")]
fn run_allocation_mode() {
    let target_mib = setting("TMON_BENCH_TARGET_MIB", 32, 1, 1024);
    let total_mib = target_mib
        .checked_mul(WORKLOADS.len())
        .and_then(|mib| mib.checked_mul(2))
        .expect("allocation workload size overflowed usize");
    assert!(
        total_mib <= MAX_ALLOCATION_MIB_TOTAL,
        "requested allocation accounting parses {total_mib} MiB across both engines; \
         the CI-safe limit is {MAX_ALLOCATION_MIB_TOTAL} MiB. Reduce \
         TMON_BENCH_TARGET_MIB"
    );
    let target_bytes = target_mib * MIB;

    println!("Tmon vs Alacritty allocation request accounting");
    println!("===============================================");
    println!("Platform: {}/{}", env::consts::OS, env::consts::ARCH);
    println!("Grid: {COLS}x{ROWS}, scrollback: {SCROLLBACK} lines");
    println!("Parse target per engine/workload: {target_bytes} bytes ({target_mib} MiB)");
    println!(
        "Warmup per engine/workload: {} bytes ({} MiB, uncounted)",
        ALLOCATION_WARMUP_BYTES,
        ALLOCATION_WARMUP_BYTES / MIB
    );
    println!();

    for workload in &WORKLOADS {
        let payload = workload.payload.as_bytes();
        let processed_bytes = iterations_for(target_bytes, payload) * payload.len();
        println!("{}", workload.name);
        for engine in [Engine::Tmon, Engine::Alacritty] {
            let allocations = allocation_sample(engine, workload, target_bytes);
            let engine_name = match engine {
                Engine::Tmon => "Tmon",
                Engine::Alacritty => "Alacritty",
            };
            println!("  {engine_name}");
            println!("    processed bytes:       {processed_bytes}");
            println!(
                "    alloc:                 {} successful calls, {} requested bytes",
                allocations.alloc_calls, allocations.alloc_bytes
            );
            println!(
                "    dealloc:               {} calls, {} requested bytes",
                allocations.dealloc_calls, allocations.dealloc_bytes
            );
            println!(
                "    realloc:               {} successful calls, {} old requested bytes, {} new requested bytes",
                allocations.realloc_calls,
                allocations.realloc_old_bytes,
                allocations.realloc_new_bytes
            );
            println!(
                "    net requested change:  {:+} bytes",
                allocations.net_requested_bytes()
            );
        }
        println!();
    }

    println!("Notes");
    println!("  - This mode is report-only and has no performance or allocation gates.");
    println!(
        "  - A fresh terminal is constructed before counting; only its feed_output loop is counted."
    );
    println!("  - Terminal snapshots and terminal destruction happen after counting stops.");
    println!("  - alloc includes successful alloc_zeroed calls and requested bytes.");
    println!("  - Net requested change = alloc + realloc-new - dealloc - realloc-old bytes.");
    println!("  - Counts cover all process allocator traffic while the feed guard is active.");
    println!(
        "  - Requested-byte accounting is not RSS, retained/usable heap, allocator metadata, or physical memory."
    );
}

#[cfg_attr(feature = "benchmark-allocations", allow(dead_code))]
fn run_timed_mode() {
    let target_mib = setting("TMON_BENCH_TARGET_MIB", 32, 1, 1024);
    let target_bytes = target_mib * MIB;
    if run_profile_mode(target_bytes) {
        return;
    }
    let sample_count = require_even_sample_count(setting("TMON_BENCH_SAMPLES", 8, 3, 25));
    let snapshot_count = setting("TMON_BENCH_SNAPSHOTS", 250, 10, 10_000);
    for workload in &WORKLOADS {
        validate_target_ratio(workload.minimum_ratio).unwrap_or_else(|reason| {
            panic!(
                "{} has an invalid target ratio {:?}: {reason}",
                workload.name, workload.minimum_ratio
            )
        });
    }
    let benchmark_mib_per_engine = target_mib
        .checked_mul(sample_count)
        .and_then(|mib| mib.checked_mul(WORKLOADS.len()))
        .expect("benchmark workload size overflowed usize");
    assert!(
        benchmark_mib_per_engine <= MAX_BENCHMARK_MIB_PER_ENGINE,
        "requested benchmark parses {benchmark_mib_per_engine} MiB per engine; \
         the CI-safe limit is {MAX_BENCHMARK_MIB_PER_ENGINE} MiB. Reduce \
         TMON_BENCH_TARGET_MIB or TMON_BENCH_SAMPLES"
    );

    println!("Tmon vs Alacritty terminal engine benchmark");
    println!("============================================");
    println!("Platform: {}/{}", env::consts::OS, env::consts::ARCH);
    println!("Grid: {COLS}x{ROWS}, scrollback: {SCROLLBACK} lines");
    println!("Samples: {sample_count}, parse target: {target_mib} MiB/sample");
    println!("Full snapshots: {snapshot_count}/sample");
    println!();
    println!(
        "Correctness preflight (untimed full-grid comparison, up to {} MiB/workload)",
        VALIDATION_BYTES_MAX / MIB
    );
    for workload in &WORKLOADS {
        assert_workload_equivalence(workload, target_bytes.min(VALIDATION_BYTES_MAX));
        println!("  {:<24} matched", workload.name);
    }
    println!();
    println!("Static grid cell size (excludes optional heap allocations):");
    println!("  Tmon:       {} bytes", size_of::<tmon::Cell>());
    println!(
        "  Alacritty:  {} bytes",
        size_of::<alacritty_terminal::term::cell::Cell>()
    );
    println!();
    println!(
        "Integrated Termy backend throughput (median MiB/s and paired ratio; higher is better)"
    );
    println!(
        "{:<24} {:>12} {:>12} {:>14} {:>10}",
        "Workload", "Tmon", "Alacritty", "Tmon/Alac", "Target"
    );
    println!("{}", "-".repeat(91));

    let mut ratios = Vec::with_capacity(WORKLOADS.len());
    let mut failed_targets = Vec::new();
    for (index, workload) in WORKLOADS.iter().enumerate() {
        let samples = collect_parse_samples(
            workload,
            target_bytes,
            sample_count,
            index.is_multiple_of(2),
        );
        let measurements = paired_stats(&samples);
        let ratio = measurements.ratio.median;
        ratios.push(ratio);
        if !meets_target(ratio, workload.minimum_ratio) {
            failed_targets.push((workload.name, ratio, workload.minimum_ratio));
        }
        println!(
            "{:<24} {:>12.1} {:>12.1} {:>13.3}x {:>9.3}x",
            workload.name,
            measurements.tmon.median,
            measurements.alacritty.median,
            ratio,
            workload.minimum_ratio
        );
        println!(
            "  range                  {:>7.1}-{:<7.1} {:>7.1}-{:<7.1} {:>7.3}-{:<7.3}x",
            measurements.tmon.min,
            measurements.tmon.max,
            measurements.alacritty.min,
            measurements.alacritty.max,
            measurements.ratio.min,
            measurements.ratio.max
        );
        println!(
            "  paired ratios          {}",
            format_paired_ratios(&samples)
        );
    }

    let parse_geomean =
        (ratios.iter().map(|ratio| ratio.ln()).sum::<f64>() / ratios.len() as f64).exp();
    let snapshot_samples = collect_snapshot_samples(snapshot_count, sample_count, false);
    let snapshot = paired_stats(&snapshot_samples);
    let snapshot_ratio = snapshot.ratio.median;
    let snapshot_baseline_retention = snapshot_ratio / SUPPLIED_SNAPSHOT_RATIO_BASELINE * 100.0;

    println!();
    println!("Current full-frame API throughput (different result work; median calls/s)");
    println!(
        "  Tmon:                       {:>12.1}",
        snapshot.tmon.median
    );
    println!(
        "  Alacritty:                  {:>12.1}",
        snapshot.alacritty.median
    );
    println!("  Median paired Tmon/Alac:    {snapshot_ratio:>11.3}x");
    println!("  Supplied baseline:          {SUPPLIED_SNAPSHOT_RATIO_BASELINE:>11.3}x");
    println!("  Supplied-baseline retention:{snapshot_baseline_retention:>11.1}%");
    println!(
        "  Paired ratios:              {}",
        format_paired_ratios(&snapshot_samples)
    );
    println!();
    println!("Summary");
    println!("  Paired parse-ratio geometric mean: {parse_geomean:.3}x");
    println!("  Current full-frame API ratio:      {snapshot_ratio:.3}x");
    if failed_targets.is_empty() {
        println!("  Parser/grid targets:             PASS");
    } else {
        println!("  Parser/grid targets:             FAIL");
        for (name, actual, minimum) in &failed_targets {
            println!("    {name}: {actual:.3}x < {minimum:.3}x");
        }
    }
    println!();
    println!("Notes");
    println!("  - Ratios above 1.000x favor Tmon; below 1.000x favor Alacritty.");
    println!("  - Alacritty is measured through Termy's termy_core wrapper.");
    println!("  - Even sample counts balance T/A and A/T starting order within every workload.");
    println!("  - Parser targets use the median of per-sample paired ratios.");
    println!(
        "  - Parser calls use each workload's small repeated payload, matching the saved baseline."
    );
    println!(
        "  - Tmon snapshots copy raw Cells; termy_core converts Alacritty cells into TermyFrame."
    );
    println!(
        "    Snapshot ratios measure the current public APIs, not equivalent raw engine work."
    );
    println!("  - Snapshot ratio is report-only: a 19.880x * 0.95 hosted-CI gate is not sound");
    println!("    because the APIs differ; retention is only a supplied-baseline trend.");
    println!("  - This excludes PTY I/O, GPUI rendering, input latency, RSS, and feature parity.");
    println!("  - Compare trends across runs; shared GitHub runners have timing noise.");

    if env::var("TMON_BENCH_ENFORCE_TARGETS").as_deref() == Ok("1") && !failed_targets.is_empty() {
        eprintln!("Tmon missed one or more required parser/grid throughput targets");
        std::process::exit(2);
    }
}

#[cfg(not(feature = "benchmark-allocations"))]
fn main() {
    assert!(
        env::var("TMON_BENCH_ALLOCATIONS_ONLY").as_deref() != Ok("1"),
        "TMON_BENCH_ALLOCATIONS_ONLY=1 requires rebuilding engine_compare with \
         --features benchmark-allocations"
    );
    run_timed_mode();
}

#[cfg(feature = "benchmark-allocations")]
fn main() {
    assert!(
        env::var("TMON_BENCH_ALLOCATIONS_ONLY").as_deref() == Ok("1"),
        "engine_compare was built with benchmark-allocations and refuses timed mode; \
         set TMON_BENCH_ALLOCATIONS_ONLY=1"
    );
    run_allocation_mode();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sample_order_is_balanced_for_even_counts() {
        let tmon_first = (0..8)
            .map(|index| sample_order(index, true))
            .collect::<Vec<_>>();
        assert_eq!(tmon_first[0], [Engine::Tmon, Engine::Alacritty]);
        assert_eq!(tmon_first[1], [Engine::Alacritty, Engine::Tmon]);
        assert_eq!(
            tmon_first
                .iter()
                .filter(|order| order[0] == Engine::Tmon)
                .count(),
            4
        );

        let alacritty_first = (0..8)
            .map(|index| sample_order(index, false))
            .collect::<Vec<_>>();
        assert_eq!(alacritty_first[0], [Engine::Alacritty, Engine::Tmon]);
        assert_eq!(
            alacritty_first
                .iter()
                .filter(|order| order[0] == Engine::Alacritty)
                .count(),
            4
        );
    }

    #[test]
    #[should_panic(expected = "TMON_BENCH_SAMPLES must be even")]
    fn odd_sample_counts_are_rejected() {
        require_even_sample_count(7);
    }

    #[test]
    fn stats_calculate_even_median_and_range() {
        assert_eq!(
            stats(&[4.0, 1.0, 3.0, 2.0]),
            Stats {
                median: 2.5,
                min: 1.0,
                max: 4.0,
            }
        );
    }

    #[test]
    fn paired_stats_use_sample_ratios_instead_of_independent_medians() {
        let samples = [
            PairedSample {
                first: Engine::Tmon,
                tmon: 10.0,
                alacritty: 1.0,
            },
            PairedSample {
                first: Engine::Alacritty,
                tmon: 20.0,
                alacritty: 100.0,
            },
            PairedSample {
                first: Engine::Tmon,
                tmon: 30.0,
                alacritty: 10.0,
            },
            PairedSample {
                first: Engine::Alacritty,
                tmon: 40.0,
                alacritty: 20.0,
            },
        ];
        let result = paired_stats(&samples);
        assert_eq!(result.ratio.median, 2.5);
        assert_ne!(
            result.ratio.median,
            result.tmon.median / result.alacritty.median
        );
    }

    #[test]
    fn threshold_helpers_reject_non_finite_values_and_apply_the_boundary() {
        assert_eq!(
            validate_target_ratio(f64::NAN),
            Err("target ratio must be finite")
        );
        assert_eq!(
            validate_target_ratio(f64::INFINITY),
            Err("target ratio must be finite")
        );
        assert_eq!(
            validate_target_ratio(f64::NEG_INFINITY),
            Err("target ratio must be finite")
        );
        assert!(meets_target(1.05, 1.05));
        assert!(!meets_target(1.049, 1.05));
        assert!(!meets_target(f64::NAN, 1.05));
    }

    #[cfg(feature = "benchmark-allocations")]
    fn allocation_test_lock() -> std::sync::MutexGuard<'static, ()> {
        static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
        LOCK.lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    #[cfg(feature = "benchmark-allocations")]
    #[test]
    fn allocation_counter_calculates_signed_requested_byte_change() {
        let positive = allocation_counter::AllocationSnapshot {
            alloc_calls: 2,
            alloc_bytes: 100,
            dealloc_calls: 1,
            dealloc_bytes: 40,
            realloc_calls: 1,
            realloc_old_bytes: 20,
            realloc_new_bytes: 70,
        };
        assert_eq!(positive.net_requested_bytes(), 110);

        let negative = allocation_counter::AllocationSnapshot {
            alloc_calls: 1,
            alloc_bytes: 10,
            dealloc_calls: 2,
            dealloc_bytes: 80,
            realloc_calls: 1,
            realloc_old_bytes: 30,
            realloc_new_bytes: 20,
        };
        assert_eq!(negative.net_requested_bytes(), -80);
    }

    #[cfg(feature = "benchmark-allocations")]
    #[test]
    fn allocation_measurement_rejects_nested_guards() {
        let _test_lock = allocation_test_lock();
        let outer = allocation_counter::AllocationMeasurement::begin()
            .expect("the first allocation measurement should start");
        assert!(
            allocation_counter::AllocationMeasurement::begin().is_err(),
            "a nested allocation measurement must be rejected"
        );
        drop(outer);

        let after_drop = allocation_counter::AllocationMeasurement::begin()
            .expect("dropping the outer guard should release the measurement state");
        drop(after_drop);
    }

    #[cfg(feature = "benchmark-allocations")]
    #[test]
    fn allocation_measurement_disables_during_panic_unwind() {
        let _test_lock = allocation_test_lock();
        let unwind = std::panic::catch_unwind(|| {
            let _measurement = allocation_counter::AllocationMeasurement::begin()
                .expect("allocation measurement should start before the test panic");
            panic!("intentional allocation-guard unwind");
        });
        assert!(unwind.is_err());

        let after_unwind = allocation_counter::AllocationMeasurement::begin()
            .expect("the allocation guard should disable itself while unwinding");
        drop(after_unwind);
    }

    #[cfg(feature = "benchmark-allocations")]
    #[test]
    fn allocation_measurement_waits_for_registered_hooks() {
        let _test_lock = allocation_test_lock();
        let finished = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let finished_by_finisher = std::sync::Arc::clone(&finished);

        let measurement = allocation_counter::AllocationMeasurement::begin()
            .expect("allocation measurement should start");
        assert!(allocation_counter::register_hook_for_test());
        let finisher = std::thread::spawn(move || {
            black_box(measurement.finish());
            finished_by_finisher.store(true, std::sync::atomic::Ordering::Release);
        });

        while !allocation_counter::stopping_for_test() {
            std::hint::spin_loop();
        }
        assert!(
            !finished.load(std::sync::atomic::Ordering::Acquire),
            "finish must wait while an allocator hook remains registered"
        );
        allocation_counter::unregister_hook_for_test();
        finisher.join().expect("the finisher thread should exit");
        assert!(finished.load(std::sync::atomic::Ordering::Acquire));
    }

    #[cfg(feature = "benchmark-allocations")]
    #[test]
    fn stale_hook_registration_does_not_cross_measurement_generations() {
        let _test_lock = allocation_test_lock();
        let first = allocation_counter::AllocationMeasurement::begin()
            .expect("the first allocation measurement should start");
        let stale_gate = allocation_counter::gate_for_test();
        drop(first);

        let second = allocation_counter::AllocationMeasurement::begin()
            .expect("the second allocation measurement should start");
        assert!(
            !allocation_counter::register_hook_from_for_test(stale_gate),
            "a hook that observed the prior generation must remain uncounted"
        );
        drop(second);
    }

    #[cfg(feature = "benchmark-allocations")]
    #[test]
    fn allocation_measurement_counts_zeroed_allocations_and_deallocations() {
        let _test_lock = allocation_test_lock();
        let layout = std::alloc::Layout::from_size_align(4096, 64)
            .expect("the test allocation layout should be valid");
        let measurement = allocation_counter::AllocationMeasurement::begin()
            .expect("allocation measurement should start");
        // SAFETY: `layout` has non-zero size and a valid power-of-two alignment.
        // The returned pointer is checked before it is deallocated exactly once
        // with the same layout.
        let pointer = unsafe { std::alloc::alloc_zeroed(layout) };
        assert!(!pointer.is_null(), "the test allocation should succeed");
        // SAFETY: `pointer` came from `alloc_zeroed(layout)`, is non-null, and
        // has not been deallocated or reallocated.
        unsafe { std::alloc::dealloc(pointer, layout) };
        let snapshot = measurement.finish();

        assert!(snapshot.alloc_calls > 0);
        assert!(snapshot.alloc_bytes >= 4096);
        assert!(snapshot.dealloc_calls > 0);
        assert!(snapshot.dealloc_bytes >= 4096);
    }

    #[cfg(feature = "benchmark-allocations")]
    #[test]
    fn allocation_measurement_counts_successful_reallocations() {
        let _test_lock = allocation_test_lock();
        let old_layout = std::alloc::Layout::from_size_align(1024, 64)
            .expect("the old test allocation layout should be valid");
        // SAFETY: `old_layout` has non-zero size and a valid power-of-two
        // alignment. The pointer is checked and later passed exactly once to
        // `realloc` with its original layout.
        let pointer = unsafe { std::alloc::alloc(old_layout) };
        assert!(
            !pointer.is_null(),
            "the initial test allocation should succeed"
        );

        let measurement = allocation_counter::AllocationMeasurement::begin()
            .expect("allocation measurement should start");
        // SAFETY: `pointer` is a live allocation created with `old_layout`, and
        // 4096 is a valid non-zero new size for the same alignment.
        let new_pointer = unsafe { std::alloc::realloc(pointer, old_layout, 4096) };
        let snapshot = measurement.finish();
        if new_pointer.is_null() {
            // SAFETY: A failed `realloc` leaves the original allocation live,
            // so it must be released with its original layout.
            unsafe { std::alloc::dealloc(pointer, old_layout) };
            panic!("the test reallocation should succeed");
        }
        let new_layout = std::alloc::Layout::from_size_align(4096, 64)
            .expect("the new test allocation layout should be valid");
        // SAFETY: `new_pointer` is the successful result of reallocating to
        // 4096 bytes with alignment 64 and has not otherwise been freed.
        unsafe { std::alloc::dealloc(new_pointer, new_layout) };

        assert!(snapshot.realloc_calls > 0);
        assert!(snapshot.realloc_old_bytes >= 1024);
        assert!(snapshot.realloc_new_bytes >= 4096);
    }
}
