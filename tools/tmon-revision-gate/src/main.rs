use std::{
    env,
    hint::black_box,
    process::ExitCode,
    sync::OnceLock,
    time::{Duration, Instant},
};

const BASELINE_SHA: &str = "03d7ca5c5420ca141afe56f725341faadb71af18";
const COLS: u16 = 120;
const ROWS: u16 = 40;
const SCROLLBACK: usize = 10_000;
const MIB: usize = 1024 * 1024;
const PREFILL_BYTES: usize = 2 * MIB;
const BLOCK_COUNT: usize = 20;
const RUNS_PER_BLOCK: usize = 4;
const MIN_MEASUREMENT_DURATION: Duration = Duration::from_millis(250);
const CALIBRATION_TARGET_DURATION: Duration = Duration::from_millis(275);
const INITIAL_CALLS: usize = 64;
const MAX_CALLS_PER_MEASUREMENT: usize = 50_000_000;
const ADAPTIVE_HEADROOM: f64 = 1.10;
const THRESHOLD: f64 = 0.95;
const LOWER_INDEX: usize = 5;
const UPPER_INDEX: usize = 14;

const ATTR_BOLD: u16 = 1 << 0;
const ATTR_DIM: u16 = 1 << 1;
const ATTR_ITALIC: u16 = 1 << 2;
const ATTR_UNDERLINE: u16 = 1 << 3;
const ATTR_INVERSE: u16 = 1 << 4;
const ATTR_HIDDEN: u16 = 1 << 5;
const ATTR_STRIKETHROUGH: u16 = 1 << 6;

const CELL_PROTECTED: u16 = 1 << 0;
const CELL_WIDE_SPACER: u16 = 1 << 1;
const CELL_LEADING_WIDE_SPACER: u16 = 1 << 2;
const CELL_WRAPPED: u16 = 1 << 3;
const CELL_HYPERLINK: u16 = 1 << 4;
const CELL_COMBINING: u16 = 1 << 5;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Engine {
    Current,
    Reference,
}

impl Engine {
    const fn short_name(self) -> &'static str {
        match self {
            Self::Current => "C",
            Self::Reference => "R",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum NormalizedColor {
    Default,
    Indexed(u8),
    Rgb(u8, u8, u8),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum NormalizedCursorStyle {
    Block,
    Line,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct NormalizedCursor {
    col: usize,
    row: usize,
    style: NormalizedCursorStyle,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct NormalizedCell {
    character: char,
    foreground: NormalizedColor,
    background: NormalizedColor,
    attributes: u16,
    combining: String,
    state: u16,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct NormalizedFrame {
    cols: u16,
    rows: u16,
    cells: Vec<NormalizedCell>,
    cursor: Option<NormalizedCursor>,
    display_offset: usize,
    history_size: usize,
}

#[derive(Clone, Copy, Debug)]
struct Measurement {
    engine: Engine,
    calls: usize,
    elapsed: Duration,
}

impl Measurement {
    fn throughput(self) -> f64 {
        self.calls as f64 / self.elapsed.as_secs_f64()
    }
}

#[derive(Clone, Debug)]
struct Block {
    index: usize,
    runs: [Measurement; RUNS_PER_BLOCK],
    current_throughput: f64,
    reference_throughput: f64,
    ratio: f64,
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct RatioSummary {
    median: f64,
    min: f64,
    max: f64,
    lower: f64,
    upper: f64,
    mad: f64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Classification {
    Pass,
    Regression,
    Inconclusive,
}

impl Classification {
    const fn label(self) -> &'static str {
        match self {
            Self::Pass => "PASS",
            Self::Regression => "REGRESSION",
            Self::Inconclusive => "INCONCLUSIVE",
        }
    }

    fn exit_code(self) -> ExitCode {
        match self {
            Self::Pass => ExitCode::SUCCESS,
            Self::Regression => ExitCode::from(1),
            Self::Inconclusive => ExitCode::from(2),
        }
    }
}

fn current_terminal(cols: u16, rows: u16, scrollback: usize) -> tmon_current::Terminal {
    tmon_current::Terminal::new_display(
        tmon_current::Size {
            cols,
            rows,
            ..tmon_current::Size::default()
        },
        tmon_current::Config {
            scrollback_history: scrollback,
            ..tmon_current::Config::default()
        },
    )
}

fn reference_terminal(cols: u16, rows: u16, scrollback: usize) -> tmon_reference::Terminal {
    tmon_reference::Terminal::new_display(
        tmon_reference::Size {
            cols,
            rows,
            ..tmon_reference::Size::default()
        },
        tmon_reference::Config {
            scrollback_history: scrollback,
            ..tmon_reference::Config::default()
        },
    )
}

fn plain_prefill() -> Vec<u8> {
    const RECORD: &[u8] = b"snapshot prefill plain ASCII 0123456789 abcdefghijklmnopqrstuvwxyz\r\n";
    let mut prefill = Vec::with_capacity(PREFILL_BYTES);
    while prefill.len() < PREFILL_BYTES {
        let remaining = PREFILL_BYTES - prefill.len();
        prefill.extend_from_slice(&RECORD[..remaining.min(RECORD.len())]);
    }
    prefill
}

fn mixed_visible_fixture() -> &'static [u8] {
    static FIXTURE: OnceLock<Vec<u8>> = OnceLock::new();
    FIXTURE
        .get_or_init(|| {
            let mut fixture = String::from("\x1b[2J\x1b[H");
            for row in 1..=ROWS {
                fixture.push_str("\x1b[");
                fixture.push_str(&row.to_string());
                fixture.push_str(";1H");
                fixture.push_str(concat!(
                    "\x1b[0mplain ",
                    "\x1b[31mred ",
                    "\x1b[38;5;200mindexed ",
                    "\x1b[38;2;12;34;56mrgb ",
                    "\x1b[48;2;65;43;21mbg\x1b[0m ",
                    "\x1b[1mbold\x1b[0m ",
                    "\x1b[2mdim\x1b[0m ",
                    "\x1b[3mitalic\x1b[0m ",
                    "\x1b[4munderline\x1b[0m ",
                    "\x1b[7minverse\x1b[0m ",
                    "\x1b[8mhidden\x1b[0m ",
                    "\x1b[9mstrike\x1b[0m ",
                    "cafe\u{301} \u{754c} ",
                    "\x1b]8;;https://example.invalid/fixture\x1b\\link\x1b]8;;\x1b\\",
                ));
            }
            fixture.push_str("\x1b[0m\x1b[20;60H");
            fixture.into_bytes()
        })
        .as_slice()
}

fn prepare_workload(current: &tmon_current::Terminal, reference: &tmon_reference::Terminal) {
    let prefill = plain_prefill();
    current.feed_output(&prefill);
    reference.feed_output(&prefill);
    let fixture = mixed_visible_fixture();
    current.feed_output(fixture);
    reference.feed_output(fixture);
}

const fn flag(enabled: bool, bit: u16) -> u16 {
    if enabled { bit } else { 0 }
}

fn current_color(color: tmon_current::Color) -> NormalizedColor {
    match color {
        tmon_current::Color::Default => NormalizedColor::Default,
        tmon_current::Color::Indexed(index) => NormalizedColor::Indexed(index),
        tmon_current::Color::Rgb { r, g, b } => NormalizedColor::Rgb(r, g, b),
    }
}

fn reference_color(color: tmon_reference::Color) -> NormalizedColor {
    match color {
        tmon_reference::Color::Default => NormalizedColor::Default,
        tmon_reference::Color::Indexed(index) => NormalizedColor::Indexed(index),
        tmon_reference::Color::Rgb { r, g, b } => NormalizedColor::Rgb(r, g, b),
    }
}

fn normalize_current_cell(
    cell: &tmon_current::Cell,
    combining: Option<tmon_current::Combining<'_>>,
) -> NormalizedCell {
    let attributes = flag(cell.attributes.bold(), ATTR_BOLD)
        | flag(cell.attributes.dim(), ATTR_DIM)
        | flag(cell.attributes.italic(), ATTR_ITALIC)
        | flag(cell.attributes.underline(), ATTR_UNDERLINE)
        | flag(cell.attributes.inverse(), ATTR_INVERSE)
        | flag(cell.attributes.hidden(), ATTR_HIDDEN)
        | flag(cell.attributes.strikethrough(), ATTR_STRIKETHROUGH);
    let state = flag(cell.protected(), CELL_PROTECTED)
        | flag(cell.wide_spacer(), CELL_WIDE_SPACER)
        | flag(cell.leading_wide_spacer(), CELL_LEADING_WIDE_SPACER)
        | flag(cell.wrapped(), CELL_WRAPPED)
        | flag(cell.has_hyperlink(), CELL_HYPERLINK)
        | flag(cell.has_combining(), CELL_COMBINING);
    NormalizedCell {
        character: cell.character,
        foreground: current_color(cell.foreground),
        background: current_color(cell.background),
        attributes,
        combining: combining.map_or_else(String::new, |value| value.to_owned_string()),
        state,
    }
}

fn normalize_reference_cell(
    cell: &tmon_reference::Cell,
    combining: Option<tmon_reference::Combining<'_>>,
) -> NormalizedCell {
    let attributes = flag(cell.attributes.bold, ATTR_BOLD)
        | flag(cell.attributes.dim, ATTR_DIM)
        | flag(cell.attributes.italic, ATTR_ITALIC)
        | flag(cell.attributes.underline, ATTR_UNDERLINE)
        | flag(cell.attributes.inverse, ATTR_INVERSE)
        | flag(cell.attributes.hidden, ATTR_HIDDEN)
        | flag(cell.attributes.strikethrough, ATTR_STRIKETHROUGH);
    let state = flag(cell.protected, CELL_PROTECTED)
        | flag(cell.wide_spacer, CELL_WIDE_SPACER)
        | flag(cell.leading_wide_spacer, CELL_LEADING_WIDE_SPACER)
        | flag(cell.wrapped, CELL_WRAPPED)
        | flag(cell.hyperlink_id.is_some(), CELL_HYPERLINK)
        | flag(cell.combining_id.is_some(), CELL_COMBINING);
    NormalizedCell {
        character: cell.character,
        foreground: reference_color(cell.foreground),
        background: reference_color(cell.background),
        attributes,
        combining: combining.map_or_else(String::new, |value| value.to_owned_string()),
        state,
    }
}

fn normalize_current(terminal: &tmon_current::Terminal) -> NormalizedFrame {
    let size = terminal.size();
    let mut cells = Vec::with_capacity(usize::from(size.cols) * usize::from(size.rows));
    terminal.for_each_viewport_cell(|_, _, _, cell, combining| {
        cells.push(normalize_current_cell(cell, combining));
    });
    let (display_offset, history_size) = terminal.scroll_state();
    NormalizedFrame {
        cols: size.cols,
        rows: size.rows,
        cells,
        cursor: terminal.cursor_state().map(|cursor| NormalizedCursor {
            col: cursor.col,
            row: cursor.row,
            style: match cursor.style {
                tmon_current::CursorStyle::Block => NormalizedCursorStyle::Block,
                tmon_current::CursorStyle::Line => NormalizedCursorStyle::Line,
            },
        }),
        display_offset,
        history_size,
    }
}

fn normalize_reference(terminal: &tmon_reference::Terminal) -> NormalizedFrame {
    let size = terminal.size();
    let mut cells = Vec::with_capacity(usize::from(size.cols) * usize::from(size.rows));
    terminal.for_each_viewport_cell(|_, _, _, cell, combining| {
        cells.push(normalize_reference_cell(cell, combining));
    });
    let (display_offset, history_size) = terminal.scroll_state();
    NormalizedFrame {
        cols: size.cols,
        rows: size.rows,
        cells,
        cursor: terminal.cursor_state().map(|cursor| NormalizedCursor {
            col: cursor.col,
            row: cursor.row,
            style: match cursor.style {
                tmon_reference::CursorStyle::Block => NormalizedCursorStyle::Block,
                tmon_reference::CursorStyle::Line => NormalizedCursorStyle::Line,
            },
        }),
        display_offset,
        history_size,
    }
}

fn normalize_current_snapshot(snapshot: &tmon_current::Snapshot) -> NormalizedFrame {
    NormalizedFrame {
        cols: snapshot.cols,
        rows: snapshot.rows,
        cells: snapshot
            .cells
            .iter()
            .map(|cell| normalize_current_cell(cell, None))
            .collect(),
        cursor: snapshot.cursor.map(|cursor| NormalizedCursor {
            col: cursor.col,
            row: cursor.row,
            style: match cursor.style {
                tmon_current::CursorStyle::Block => NormalizedCursorStyle::Block,
                tmon_current::CursorStyle::Line => NormalizedCursorStyle::Line,
            },
        }),
        display_offset: snapshot.display_offset,
        history_size: snapshot.history_size,
    }
}

fn normalize_reference_snapshot(snapshot: &tmon_reference::Snapshot) -> NormalizedFrame {
    NormalizedFrame {
        cols: snapshot.cols,
        rows: snapshot.rows,
        cells: snapshot
            .cells
            .iter()
            .map(|cell| normalize_reference_cell(cell, None))
            .collect(),
        cursor: snapshot.cursor.map(|cursor| NormalizedCursor {
            col: cursor.col,
            row: cursor.row,
            style: match cursor.style {
                tmon_reference::CursorStyle::Block => NormalizedCursorStyle::Block,
                tmon_reference::CursorStyle::Line => NormalizedCursorStyle::Line,
            },
        }),
        display_offset: snapshot.display_offset,
        history_size: snapshot.history_size,
    }
}

fn without_combining_text(mut frame: NormalizedFrame) -> NormalizedFrame {
    for cell in &mut frame.cells {
        cell.combining.clear();
    }
    frame
}

fn normalized_difference(current: &NormalizedFrame, reference: &NormalizedFrame) -> String {
    if (
        current.cols,
        current.rows,
        current.cursor,
        current.display_offset,
        current.history_size,
    ) != (
        reference.cols,
        reference.rows,
        reference.cursor,
        reference.display_offset,
        reference.history_size,
    ) {
        return format!(
            "metadata differs: current=({}x{}, cursor={:?}, offset={}, history={}), \
             reference=({}x{}, cursor={:?}, offset={}, history={})",
            current.cols,
            current.rows,
            current.cursor,
            current.display_offset,
            current.history_size,
            reference.cols,
            reference.rows,
            reference.cursor,
            reference.display_offset,
            reference.history_size,
        );
    }
    if current.cells.len() != reference.cells.len() {
        return format!(
            "cell count differs: current={}, reference={}",
            current.cells.len(),
            reference.cells.len()
        );
    }
    if let Some((index, (current_cell, reference_cell))) = current
        .cells
        .iter()
        .zip(&reference.cells)
        .enumerate()
        .find(|(_, (current_cell, reference_cell))| current_cell != reference_cell)
    {
        let row = index / usize::from(current.cols);
        let col = index % usize::from(current.cols);
        return format!(
            "cell differs at row={row}, col={col}: current={current_cell:?}, \
             reference={reference_cell:?}"
        );
    }
    "normalized frames differ for an unknown reason".to_owned()
}

fn semantic_preflight(
    current_terminal: &tmon_current::Terminal,
    reference_terminal: &tmon_reference::Terminal,
) -> Result<NormalizedFrame, String> {
    let current_live = normalize_current(current_terminal);
    let reference_live = normalize_reference(reference_terminal);
    if current_live != reference_live {
        return Err(normalized_difference(&current_live, &reference_live));
    }

    let current_snapshot = normalize_current_snapshot(&current_terminal.snapshot());
    let reference_snapshot = normalize_reference_snapshot(&reference_terminal.snapshot());
    let current_expected = without_combining_text(current_live.clone());
    let reference_expected = without_combining_text(reference_live);
    if current_snapshot != current_expected {
        return Err(format!(
            "current Terminal::snapshot() disagrees with independent terminal state: {}",
            normalized_difference(&current_snapshot, &current_expected)
        ));
    }
    if reference_snapshot != reference_expected {
        return Err(format!(
            "reference Terminal::snapshot() disagrees with independent terminal state: {}",
            normalized_difference(&reference_snapshot, &reference_expected)
        ));
    }
    if current_snapshot != reference_snapshot {
        return Err(format!(
            "current and reference Terminal::snapshot() results differ: {}",
            normalized_difference(&current_snapshot, &reference_snapshot)
        ));
    }

    let expected_cells = usize::from(COLS) * usize::from(ROWS);
    if current_live.cols != COLS
        || current_live.rows != ROWS
        || current_live.cells.len() != expected_cells
        || current_live.display_offset != 0
        || current_live.history_size != SCROLLBACK
    {
        return Err(format!(
            "workload invariants failed: frame={}x{}, cells={}, offset={}, history={}",
            current_live.cols,
            current_live.rows,
            current_live.cells.len(),
            current_live.display_offset,
            current_live.history_size,
        ));
    }
    Ok(current_live)
}

fn measure_current(terminal: &tmon_current::Terminal, calls: usize) -> Measurement {
    let started = Instant::now();
    for _ in 0..calls {
        drop(black_box(terminal.snapshot()));
    }
    Measurement {
        engine: Engine::Current,
        calls,
        elapsed: started.elapsed(),
    }
}

fn measure_reference(terminal: &tmon_reference::Terminal, calls: usize) -> Measurement {
    let started = Instant::now();
    for _ in 0..calls {
        drop(black_box(terminal.snapshot()));
    }
    Measurement {
        engine: Engine::Reference,
        calls,
        elapsed: started.elapsed(),
    }
}

fn measure_engine(
    engine: Engine,
    current: &tmon_current::Terminal,
    reference: &tmon_reference::Terminal,
    calls: usize,
) -> Measurement {
    match engine {
        Engine::Current => measure_current(current, calls),
        Engine::Reference => measure_reference(reference, calls),
    }
}

fn adapted_calls(measurement: Measurement, target: Duration) -> Result<usize, String> {
    if measurement.elapsed >= target {
        return Ok(measurement.calls);
    }
    let proposed = if measurement.elapsed.is_zero() {
        measurement.calls.saturating_mul(16)
    } else {
        let scale = target.as_secs_f64() / measurement.elapsed.as_secs_f64();
        (measurement.calls as f64 * scale * ADAPTIVE_HEADROOM).ceil() as usize
    }
    .max(measurement.calls.saturating_add(1));
    if proposed > MAX_CALLS_PER_MEASUREMENT {
        return Err(format!(
            "adaptive call count {proposed} exceeds hard safety limit \
             {MAX_CALLS_PER_MEASUREMENT}"
        ));
    }
    Ok(proposed)
}

fn calibrate_calls(
    current: &tmon_current::Terminal,
    reference: &tmon_reference::Terminal,
) -> Result<(usize, [Measurement; 2]), String> {
    let mut calls = INITIAL_CALLS;
    loop {
        let current_measurement = measure_current(current, calls);
        let reference_measurement = measure_reference(reference, calls);
        let next_calls = adapted_calls(current_measurement, CALIBRATION_TARGET_DURATION)?.max(
            adapted_calls(reference_measurement, CALIBRATION_TARGET_DURATION)?,
        );
        if next_calls == calls {
            return Ok((calls, [current_measurement, reference_measurement]));
        }
        calls = next_calls;
    }
}

fn block_order(index: usize) -> [Engine; RUNS_PER_BLOCK] {
    if index.is_multiple_of(2) {
        [
            Engine::Current,
            Engine::Reference,
            Engine::Reference,
            Engine::Current,
        ]
    } else {
        [
            Engine::Reference,
            Engine::Current,
            Engine::Current,
            Engine::Reference,
        ]
    }
}

fn geometric_throughput(runs: &[Measurement], engine: Engine) -> Result<f64, String> {
    let selected = runs
        .iter()
        .copied()
        .filter(|measurement| measurement.engine == engine)
        .collect::<Vec<_>>();
    if selected.len() != 2 {
        return Err(format!(
            "balanced block must contain two {} measurements, got {}",
            engine.short_name(),
            selected.len()
        ));
    }
    let log_mean = selected
        .iter()
        .map(|measurement| measurement.throughput().ln())
        .sum::<f64>()
        / 2.0;
    Ok(log_mean.exp())
}

fn collect_block(
    index: usize,
    current: &tmon_current::Terminal,
    reference: &tmon_reference::Terminal,
    initial_calls: usize,
) -> Result<(Block, usize), String> {
    let order = block_order(index);
    let mut calls = initial_calls;
    loop {
        let runs = order.map(|engine| measure_engine(engine, current, reference, calls));
        let next_calls = runs.iter().copied().try_fold(calls, |next, measurement| {
            adapted_calls(measurement, MIN_MEASUREMENT_DURATION).map(|value| next.max(value))
        })?;
        if next_calls != calls {
            calls = next_calls;
            continue;
        }
        let current_throughput = geometric_throughput(&runs, Engine::Current)?;
        let reference_throughput = geometric_throughput(&runs, Engine::Reference)?;
        let ratio = current_throughput / reference_throughput;
        if !ratio.is_finite() || ratio <= 0.0 {
            return Err(format!(
                "block {} produced invalid ratio {ratio:?}",
                index + 1
            ));
        }
        return Ok((
            Block {
                index,
                runs,
                current_throughput,
                reference_throughput,
                ratio,
            },
            calls,
        ));
    }
}

fn median_of_sorted(sorted: &[f64]) -> f64 {
    let middle = sorted.len() / 2;
    if sorted.len().is_multiple_of(2) {
        sorted[middle - 1] / 2.0 + sorted[middle] / 2.0
    } else {
        sorted[middle]
    }
}

fn summarize_ratios(samples: &[f64]) -> Result<RatioSummary, String> {
    if samples.len() != BLOCK_COUNT {
        return Err(format!(
            "ratio summary requires exactly {BLOCK_COUNT} samples, got {}",
            samples.len()
        ));
    }
    if samples
        .iter()
        .any(|sample| !sample.is_finite() || *sample <= 0.0)
    {
        return Err("ratio samples must be finite and positive".to_owned());
    }
    let mut sorted = samples.to_vec();
    sorted.sort_by(f64::total_cmp);
    let median = median_of_sorted(&sorted);
    let mut deviations = samples
        .iter()
        .map(|sample| (sample - median).abs())
        .collect::<Vec<_>>();
    deviations.sort_by(f64::total_cmp);
    Ok(RatioSummary {
        median,
        min: sorted[0],
        max: sorted[BLOCK_COUNT - 1],
        lower: sorted[LOWER_INDEX],
        upper: sorted[UPPER_INDEX],
        mad: median_of_sorted(&deviations),
    })
}

fn classify(lower: f64, upper: f64) -> Classification {
    if lower >= THRESHOLD {
        Classification::Pass
    } else if upper < THRESHOLD {
        Classification::Regression
    } else {
        Classification::Inconclusive
    }
}

fn format_order(order: &[Engine]) -> String {
    order
        .iter()
        .map(|engine| engine.short_name())
        .collect::<Vec<_>>()
        .join("/")
}

fn print_block(block: &Block) {
    let runs = block
        .runs
        .iter()
        .map(|measurement| {
            format!(
                "{} {} calls {:.3}s {:.0}/s",
                measurement.engine.short_name(),
                measurement.calls,
                measurement.elapsed.as_secs_f64(),
                measurement.throughput(),
            )
        })
        .collect::<Vec<_>>()
        .join(" | ");
    println!(
        "block {:02} order={} | {} | geom C={:.0}/s R={:.0}/s ratio={:.4}x",
        block.index + 1,
        format_order(&block.runs.map(|measurement| measurement.engine)),
        runs,
        block.current_throughput,
        block.reference_throughput,
        block.ratio,
    );
}

fn run_gate() -> Result<Classification, String> {
    println!("Tmon immutable dual-revision snapshot regression gate");
    println!("baseline_sha: {BASELINE_SHA}");
    println!("current_source: ../../crates/temon");
    println!(
        "workload: native Terminal::snapshot() + black_box + drop; {COLS}x{ROWS}; \
         scrollback={SCROLLBACK}; plain_prefill={PREFILL_BYTES} bytes; mixed visible fixture; \
         blocks={BLOCK_COUNT}; orders=C/R/R/C,R/C/C/R; min_run={}ms; threshold={THRESHOLD:.2}x",
        MIN_MEASUREMENT_DURATION.as_millis(),
    );
    println!("settings: hardcoded (no CLI or environment overrides)");

    let current = current_terminal(COLS, ROWS, SCROLLBACK);
    let reference = reference_terminal(COLS, ROWS, SCROLLBACK);
    prepare_workload(&current, &reference);
    let frame = semantic_preflight(&current, &reference)
        .map_err(|reason| format!("semantic normalized-state preflight failed: {reason}"))?;
    println!(
        "preflight: PASS cells={} cursor={:?} offset={} history={}",
        frame.cells.len(),
        frame.cursor,
        frame.display_offset,
        frame.history_size,
    );

    let (mut calls, calibration) = calibrate_calls(&current, &reference)?;
    println!(
        "calibration: calls={} C={:.3}s R={:.3}s (target={}ms; probes excluded from blocks)",
        calls,
        calibration[0].elapsed.as_secs_f64(),
        calibration[1].elapsed.as_secs_f64(),
        CALIBRATION_TARGET_DURATION.as_millis(),
    );

    let mut blocks = Vec::with_capacity(BLOCK_COUNT);
    for index in 0..BLOCK_COUNT {
        let (block, retained_calls) = collect_block(index, &current, &reference, calls)?;
        calls = retained_calls;
        print_block(&block);
        blocks.push(block);
    }

    let ratios = blocks.iter().map(|block| block.ratio).collect::<Vec<_>>();
    let summary = summarize_ratios(&ratios)?;
    let classification = classify(summary.lower, summary.upper);
    let mut sorted = ratios;
    sorted.sort_by(f64::total_cmp);
    println!(
        "sorted_ratios: {}",
        sorted
            .iter()
            .map(|ratio| format!("{ratio:.4}"))
            .collect::<Vec<_>>()
            .join(", ")
    );
    println!("median: {:.4}x", summary.median);
    println!("range: {:.4}x..{:.4}x", summary.min, summary.max);
    println!("lower[index {LOWER_INDEX}]: {:.4}x", summary.lower);
    println!("upper[index {UPPER_INDEX}]: {:.4}x", summary.upper);
    println!("MAD: {:.4}x", summary.mad);
    println!(
        "result: {} (threshold={THRESHOLD:.4}x; PASS if lower>=threshold; \
         REGRESSION if upper<threshold; otherwise INCONCLUSIVE)",
        classification.label(),
    );
    Ok(classification)
}

fn main() -> ExitCode {
    if env::args_os().len() != 1 {
        eprintln!("usage: tmon-revision-gate (this gate accepts no arguments)");
        return ExitCode::from(64);
    }
    if cfg!(debug_assertions) {
        eprintln!("result: ERROR: build this timing gate with --release");
        return ExitCode::from(3);
    }
    match run_gate() {
        Ok(classification) => classification.exit_code(),
        Err(reason) => {
            eprintln!("result: ERROR: {reason}");
            ExitCode::from(3)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn statistics_use_required_indices_and_mad() {
        let samples = (1..=BLOCK_COUNT)
            .rev()
            .map(|value| value as f64)
            .collect::<Vec<_>>();
        assert_eq!(
            summarize_ratios(&samples).unwrap(),
            RatioSummary {
                median: 10.5,
                min: 1.0,
                max: 20.0,
                lower: 6.0,
                upper: 15.0,
                mad: 5.0,
            }
        );
    }

    #[test]
    fn statistics_reject_wrong_count_and_invalid_samples() {
        assert!(summarize_ratios(&[1.0; BLOCK_COUNT - 1]).is_err());
        let mut invalid = [1.0; BLOCK_COUNT];
        invalid[3] = f64::NAN;
        assert!(summarize_ratios(&invalid).is_err());
        invalid[3] = 0.0;
        assert!(summarize_ratios(&invalid).is_err());
    }

    #[test]
    fn classification_uses_conservative_band_and_exact_boundary() {
        assert_eq!(classify(THRESHOLD, THRESHOLD), Classification::Pass);
        assert_eq!(
            classify(THRESHOLD - 0.01, THRESHOLD - f64::EPSILON),
            Classification::Regression
        );
        assert_eq!(
            classify(THRESHOLD - 0.01, THRESHOLD),
            Classification::Inconclusive
        );
    }

    #[test]
    fn block_orders_are_balanced_and_alternate() {
        assert_eq!(
            block_order(0),
            [
                Engine::Current,
                Engine::Reference,
                Engine::Reference,
                Engine::Current,
            ]
        );
        assert_eq!(
            block_order(1),
            [
                Engine::Reference,
                Engine::Current,
                Engine::Current,
                Engine::Reference,
            ]
        );
        for index in 0..BLOCK_COUNT {
            let order = block_order(index);
            assert_eq!(
                order
                    .iter()
                    .filter(|engine| **engine == Engine::Current)
                    .count(),
                2
            );
            assert_eq!(
                order
                    .iter()
                    .filter(|engine| **engine == Engine::Reference)
                    .count(),
                2
            );
        }
    }

    #[test]
    fn geometric_ratio_uses_both_measurements_per_engine() {
        let runs = [
            Measurement {
                engine: Engine::Current,
                calls: 100,
                elapsed: Duration::from_secs(1),
            },
            Measurement {
                engine: Engine::Reference,
                calls: 100,
                elapsed: Duration::from_secs(2),
            },
            Measurement {
                engine: Engine::Reference,
                calls: 100,
                elapsed: Duration::from_secs(8),
            },
            Measurement {
                engine: Engine::Current,
                calls: 100,
                elapsed: Duration::from_secs(4),
            },
        ];
        let current = geometric_throughput(&runs, Engine::Current).unwrap();
        let reference = geometric_throughput(&runs, Engine::Reference).unwrap();
        assert!((current / reference - 2.0).abs() < 1e-12);
    }

    #[test]
    fn prefill_is_exactly_two_mib_and_plain() {
        let prefill = plain_prefill();
        assert_eq!(prefill.len(), PREFILL_BYTES);
        assert!(!prefill.contains(&0x1b));
        assert!(prefill.iter().all(u8::is_ascii));
    }

    #[test]
    fn revision_adapters_normalize_the_same_semantics() {
        let current = current_terminal(32, 4, 16);
        let reference = reference_terminal(32, 4, 16);
        let prefill = b"line one\r\nline two\r\nline three\r\nline four\r\nline five\r\n";
        let fixture = concat!(
            "\x1b[2J\x1b[H",
            "\x1b[31;1mred-bold\x1b[0m ",
            "\x1b[38;2;12;34;56;48;5;200;3;4;9mrgb\x1b[0m\r\n",
            "cafe\u{301} \u{754c} ",
            "\x1b]8;;https://example.invalid/\x1b\\link\x1b]8;;\x1b\\",
            "\x1b[3;7H",
        );
        current.feed_output(prefill);
        reference.feed_output(prefill);
        current.feed_output(fixture.as_bytes());
        reference.feed_output(fixture.as_bytes());

        let current = normalize_current(&current);
        let reference = normalize_reference(&reference);
        assert_eq!(current, reference);
        assert!(current.cells.iter().any(|cell| cell.combining == "\u{301}"));
        assert!(
            current
                .cells
                .iter()
                .any(|cell| cell.attributes & ATTR_BOLD != 0)
        );
        assert!(
            current
                .cells
                .iter()
                .any(|cell| cell.state & CELL_HYPERLINK != 0)
        );
    }

    #[test]
    fn production_workload_passes_semantic_preflight_without_timing() {
        let current = current_terminal(COLS, ROWS, SCROLLBACK);
        let reference = reference_terminal(COLS, ROWS, SCROLLBACK);
        prepare_workload(&current, &reference);

        let frame = semantic_preflight(&current, &reference).unwrap();
        assert_eq!(frame.cells.len(), usize::from(COLS) * usize::from(ROWS));
        assert_eq!(frame.history_size, SCROLLBACK);
        assert_eq!(frame.display_offset, 0);
    }
}
