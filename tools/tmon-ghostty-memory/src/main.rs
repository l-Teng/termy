use std::{
    collections::{BTreeMap, HashMap},
    env, fmt,
    fmt::Write as _,
    path::Path,
    process::{Command, ExitStatus},
};

use serde::Serialize;
use tmon_current::{Cell, Color, Combining, Config, Size, Terminal, UnderlineStyle};

mod alacritty;
mod throughput;

use alacritty::DirectAlacritty;

const COLS: u16 = 120;
const ROWS: u16 = 40;
const SCROLLBACK_LIMIT: usize = 10_000;
const SCROLLBACK_TOTAL_ROWS: usize = SCROLLBACK_LIMIT + ROWS as usize;
const ALTERNATE_TOTAL_ROWS: usize = ROWS as usize * 16;
const DEFAULT_SAMPLES: usize = 12;
const MIN_SAMPLES: usize = 6;
const MAX_SAMPLES: usize = 60;
const CHILD_PREFIX: &str = "TMON_GHOSTTY_MEMORY_CHILD";
const MEMORY_RECORD_PREFIX: &str = "TERMINAL_ENGINE_BENCH_MEMORY";
const JSON_OUTPUT_ENV: &str = "TERMINAL_ENGINE_BENCH_JSON";
const ENTER_ALTERNATE_SCREEN: &[u8] = b"\x1b[?1049h";
const TUI_REDRAW: &[u8] = concat!(
    "\x1b[H",
    "\x1b[38;2;120;180;255;48;5;234;1mstatus\x1b[0m",
    "\x1b[2;1H\x1b[2Kbuild complete cafe\u{301} \u{754c}",
    "\x1b[3;1H\x1b[2Kprogress: 100%",
    "\x1b[4;1H\x1b[38;5;81;48;5;235;4:3mworker 07 ready\x1b[0m",
    "\x1b[5;1H\x1b[2Klatency 08.4 ms",
)
.as_bytes();
const CURSOR_EDITS: &[u8] = concat!(
    "\x1b[10;20Habcdef\x1b[3DXYZ\x1b[K",
    "\x1b[4;1H\x1b[2Linserted cafe\u{301} \u{754c}",
    "\x1b[1M\x1b[0J\x1b[H",
)
.as_bytes();

const SHIM_OK: i32 = 0;
const SHIM_INVALID_ARGUMENT: i32 = 1;
const SHIM_GHOSTTY_ERROR: i32 = 2;
const SHIM_OUT_OF_MEMORY: i32 = 3;
const SHIM_PROC_ERROR: i32 = 4;

const SHIM_COMPRESSION_COMPLETE: i32 = 0;
const SHIM_COMPRESSION_UNSUPPORTED: i32 = 1;
const SHIM_COMPRESSION_ERROR: i32 = 2;

#[repr(C)]
struct GhosttyTerminalState {
    total: u64,
    offset: u64,
    len: u64,
    cursor_x: u16,
    cursor_y: u16,
    cursor_visible: u8,
    _padding: [u8; 5],
}

unsafe extern "C" {
    fn tmon_ghostty_terminal_new(
        scrollback_max_lines: usize,
        out_terminal: *mut *mut std::ffi::c_void,
    ) -> i32;
    fn tmon_ghostty_terminal_free(terminal: *mut std::ffi::c_void);
    fn tmon_ghostty_terminal_feed(
        terminal: *mut std::ffi::c_void,
        data: *const u8,
        len: usize,
    ) -> i32;
    fn tmon_ghostty_terminal_query_state(
        terminal: *mut std::ffi::c_void,
        out_state: *mut GhosttyTerminalState,
    ) -> i32;
    fn tmon_ghostty_terminal_compress_full(
        terminal: *mut std::ffi::c_void,
        out_status: *mut i32,
    ) -> i32;
    fn tmon_ghostty_terminal_semantic_digest(
        terminal: *mut std::ffi::c_void,
        out_digest: *mut u64,
    ) -> i32;
    fn tmon_ghostty_current_ri_phys_footprint(out_bytes: *mut u64) -> i32;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Ord, PartialOrd)]
enum Engine {
    Tmon,
    Alacritty,
    Ghostty,
    GhosttyCompressed,
}

impl Engine {
    const fn name(self) -> &'static str {
        match self {
            Self::Tmon => "tmon",
            Self::Alacritty => "alacritty",
            Self::Ghostty => "ghostty",
            Self::GhosttyCompressed => "ghostty-compressed",
        }
    }

    fn parse(input: &str) -> Result<Self, String> {
        match input {
            "tmon" => Ok(Self::Tmon),
            "alacritty" => Ok(Self::Alacritty),
            "ghostty" => Ok(Self::Ghostty),
            "ghostty-compressed" => Ok(Self::GhosttyCompressed),
            _ => Err(format!("unknown engine {input:?}")),
        }
    }

    const fn baseline_empty_engine(self) -> Self {
        match self {
            Self::Tmon => Self::Tmon,
            Self::Alacritty => Self::Alacritty,
            Self::Ghostty | Self::GhosttyCompressed => Self::Ghostty,
        }
    }
}

const PRIMARY_ENGINE_ORDERS: [[Engine; 3]; 6] = [
    [Engine::Tmon, Engine::Alacritty, Engine::Ghostty],
    [Engine::Tmon, Engine::Ghostty, Engine::Alacritty],
    [Engine::Alacritty, Engine::Tmon, Engine::Ghostty],
    [Engine::Alacritty, Engine::Ghostty, Engine::Tmon],
    [Engine::Ghostty, Engine::Tmon, Engine::Alacritty],
    [Engine::Ghostty, Engine::Alacritty, Engine::Tmon],
];

const fn primary_engine_order(sample: usize) -> [Engine; 3] {
    PRIMARY_ENGINE_ORDERS[sample % PRIMARY_ENGINE_ORDERS.len()]
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Ord, PartialOrd)]
enum Workload {
    Empty,
    ScreenPlain,
    ScreenStyled,
    ScreenUnicode,
    ScreenMixed,
    AlternatePlain,
    AlternateMixed,
    TuiRedraw,
    CursorEdits,
    ScrollbackPlain,
    ScrollbackStyled,
    ScrollbackUnicode,
    ScrollbackMixed,
}

impl Workload {
    const ALL: [Self; 13] = [
        Self::Empty,
        Self::ScreenPlain,
        Self::ScreenStyled,
        Self::ScreenUnicode,
        Self::ScreenMixed,
        Self::AlternatePlain,
        Self::AlternateMixed,
        Self::TuiRedraw,
        Self::CursorEdits,
        Self::ScrollbackPlain,
        Self::ScrollbackStyled,
        Self::ScrollbackUnicode,
        Self::ScrollbackMixed,
    ];

    fn parse(input: &str) -> Result<Self, String> {
        Self::ALL
            .into_iter()
            .find(|workload| workload.name() == input)
            .ok_or_else(|| format!("unknown workload {input:?}"))
    }

    const fn name(self) -> &'static str {
        match self {
            Self::Empty => "empty",
            Self::ScreenPlain => "screen-plain",
            Self::ScreenStyled => "screen-styled",
            Self::ScreenUnicode => "screen-unicode",
            Self::ScreenMixed => "screen-mixed",
            Self::AlternatePlain => "alternate-plain",
            Self::AlternateMixed => "alternate-mixed",
            Self::TuiRedraw => "tui-redraw",
            Self::CursorEdits => "cursor-edits",
            Self::ScrollbackPlain => "scrollback-plain",
            Self::ScrollbackStyled => "scrollback-styled",
            Self::ScrollbackUnicode => "scrollback-unicode",
            Self::ScrollbackMixed => "scrollback-mixed",
        }
    }

    const fn row_count(self) -> usize {
        match self {
            Self::Empty => 0,
            Self::ScreenPlain | Self::ScreenStyled | Self::ScreenUnicode | Self::ScreenMixed => {
                ROWS as usize
            }
            Self::AlternatePlain | Self::AlternateMixed => ALTERNATE_TOTAL_ROWS,
            Self::TuiRedraw | Self::CursorEdits => 0,
            Self::ScrollbackPlain
            | Self::ScrollbackStyled
            | Self::ScrollbackUnicode
            | Self::ScrollbackMixed => SCROLLBACK_TOTAL_ROWS,
        }
    }

    const fn expected_history(self) -> usize {
        match self {
            Self::Empty
            | Self::ScreenPlain
            | Self::ScreenStyled
            | Self::ScreenUnicode
            | Self::ScreenMixed
            | Self::AlternatePlain
            | Self::AlternateMixed
            | Self::TuiRedraw
            | Self::CursorEdits => 0,
            Self::ScrollbackPlain
            | Self::ScrollbackStyled
            | Self::ScrollbackUnicode
            | Self::ScrollbackMixed => SCROLLBACK_LIMIT,
        }
    }

    const fn needs_compressed_ghostty(self) -> bool {
        matches!(
            self,
            Self::ScrollbackPlain
                | Self::ScrollbackStyled
                | Self::ScrollbackUnicode
                | Self::ScrollbackMixed
        )
    }

    fn benchmark_engines(self) -> &'static [Engine] {
        if self.needs_compressed_ghostty() {
            &[
                Engine::Tmon,
                Engine::Alacritty,
                Engine::Ghostty,
                Engine::GhosttyCompressed,
            ]
        } else {
            &[Engine::Tmon, Engine::Alacritty, Engine::Ghostty]
        }
    }

    const fn enters_alternate_screen(self) -> bool {
        matches!(
            self,
            Self::AlternatePlain | Self::AlternateMixed | Self::TuiRedraw
        )
    }

    const fn trace(self) -> Option<(&'static [u8], usize)> {
        match self {
            Self::TuiRedraw => Some((TUI_REDRAW, 64)),
            Self::CursorEdits => Some((CURSOR_EDITS, 128)),
            _ => None,
        }
    }

    fn build_row(self, row_index: usize, out: &mut String) {
        match self {
            Self::Empty | Self::TuiRedraw | Self::CursorEdits => {}
            Self::ScreenPlain | Self::AlternatePlain | Self::ScrollbackPlain => {
                build_plain_row(row_index, out)
            }
            Self::ScreenStyled | Self::ScrollbackStyled => build_styled_row(row_index, out),
            Self::ScreenUnicode | Self::ScrollbackUnicode => build_unicode_row(row_index, out),
            Self::ScreenMixed | Self::AlternateMixed | Self::ScrollbackMixed => {
                build_mixed_row(row_index, out)
            }
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CompressionReport {
    None,
    Complete,
    Unsupported,
}

impl CompressionReport {
    const fn as_str(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Complete => "complete",
            Self::Unsupported => "unsupported",
        }
    }

    fn parse(input: &str) -> Result<Self, String> {
        match input {
            "none" => Ok(Self::None),
            "complete" => Ok(Self::Complete),
            "unsupported" => Ok(Self::Unsupported),
            _ => Err(format!("unknown compression status {input:?}")),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct ChildResult {
    bytes: u64,
    history: usize,
    cursor_row: usize,
    cursor_col: usize,
    digest: u64,
    compression: CompressionReport,
    footprint_bytes: u64,
}

struct BenchmarkCase {
    engine: Engine,
    workload: Workload,
}

#[derive(Serialize)]
struct MemoryReport {
    schema_version: u8,
    mode: &'static str,
    parameters: MemoryParameters,
    results: Vec<MemoryEngineResult>,
}

#[derive(Serialize)]
struct MemoryParameters {
    samples: usize,
    cols: u16,
    rows: u16,
    scrollback_limit: usize,
    scrollback_total_rows: usize,
}

#[derive(Serialize)]
struct MemoryEngineResult {
    workload: &'static str,
    engine: &'static str,
    compression: &'static str,
    payload_bytes: u64,
    samples: Vec<MemorySample>,
    median_bytes: f64,
    p05_bytes: f64,
    p95_bytes: f64,
    delta_vs_empty_bytes: f64,
}

#[derive(Serialize)]
struct MemorySample {
    sample_index: usize,
    order_position: usize,
    footprint_bytes: u64,
}

struct GhosttyTerminal {
    raw: *mut std::ffi::c_void,
}

impl GhosttyTerminal {
    fn new() -> Result<Self, String> {
        let mut raw = std::ptr::null_mut();
        let code = unsafe { tmon_ghostty_terminal_new(SCROLLBACK_LIMIT, &mut raw) };
        if code != SHIM_OK {
            return Err(format!(
                "ghostty terminal creation failed: {}",
                shim_error_name(code)
            ));
        }
        if raw.is_null() {
            return Err("ghostty shim returned a null terminal".to_string());
        }
        Ok(Self { raw })
    }

    fn feed(&self, bytes: &[u8]) -> Result<(), String> {
        let code = unsafe { tmon_ghostty_terminal_feed(self.raw, bytes.as_ptr(), bytes.len()) };
        if code == SHIM_OK {
            Ok(())
        } else {
            Err(format!("ghostty feed failed: {}", shim_error_name(code)))
        }
    }

    fn query_state(&self) -> Result<GhosttyTerminalState, String> {
        let mut state = GhosttyTerminalState {
            total: 0,
            offset: 0,
            len: 0,
            cursor_x: 0,
            cursor_y: 0,
            cursor_visible: 0,
            _padding: [0; 5],
        };
        let code = unsafe { tmon_ghostty_terminal_query_state(self.raw, &mut state) };
        if code == SHIM_OK {
            Ok(state)
        } else {
            Err(format!(
                "ghostty state query failed: {}",
                shim_error_name(code)
            ))
        }
    }

    fn compress_full(&self) -> Result<CompressionReport, String> {
        let mut status = SHIM_COMPRESSION_ERROR;
        let code = unsafe { tmon_ghostty_terminal_compress_full(self.raw, &mut status) };
        if code != SHIM_OK {
            return Err(format!(
                "ghostty compression call failed: {}",
                shim_error_name(code)
            ));
        }
        match status {
            SHIM_COMPRESSION_COMPLETE => Ok(CompressionReport::Complete),
            SHIM_COMPRESSION_UNSUPPORTED => Ok(CompressionReport::Unsupported),
            SHIM_COMPRESSION_ERROR => Err("ghostty compression reported error".to_string()),
            _ => Err(format!(
                "ghostty compression returned unknown status {status}"
            )),
        }
    }

    fn semantic_digest(&self) -> Result<u64, String> {
        let mut digest = 0;
        let code = unsafe { tmon_ghostty_terminal_semantic_digest(self.raw, &mut digest) };
        if code == SHIM_OK {
            Ok(digest)
        } else {
            Err(format!("ghostty digest failed: {}", shim_error_name(code)))
        }
    }
}

impl Drop for GhosttyTerminal {
    fn drop(&mut self) {
        unsafe { tmon_ghostty_terminal_free(self.raw) };
    }
}

#[derive(Clone, Copy)]
struct Fnv1a64 {
    state: u64,
}

impl Fnv1a64 {
    const OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
    const PRIME: u64 = 0x0000_0100_0000_01b3;

    fn new() -> Self {
        Self {
            state: Self::OFFSET,
        }
    }

    fn write_u8(&mut self, value: u8) {
        self.state ^= u64::from(value);
        self.state = self.state.wrapping_mul(Self::PRIME);
    }

    fn write_u16(&mut self, value: u16) {
        for byte in value.to_le_bytes() {
            self.write_u8(byte);
        }
    }

    fn write_u32(&mut self, value: u32) {
        for byte in value.to_le_bytes() {
            self.write_u8(byte);
        }
    }

    fn write_u64(&mut self, value: u64) {
        for byte in value.to_le_bytes() {
            self.write_u8(byte);
        }
    }

    fn finish(self) -> u64 {
        self.state
    }
}

fn main() {
    if let Err(error) = real_main() {
        eprintln!("error: {error}");
        std::process::exit(1);
    }
}

fn real_main() -> Result<(), String> {
    let mut args = env::args().skip(1);
    match (
        args.next().as_deref(),
        args.next(),
        args.next(),
        args.next(),
    ) {
        (Some("--throughput"), None, None, None) => throughput::run(),
        (Some("--child"), Some(engine), Some(workload), None) => {
            run_child_mode(Engine::parse(&engine)?, Workload::parse(&workload)?)
        }
        (None, None, None, None) => run_parent_mode(),
        _ => Err(
            "usage: tmon-ghostty-memory [--throughput | --child <tmon|alacritty|ghostty|ghostty-compressed> <workload>]"
                .to_string(),
        ),
    }
}

fn run_parent_mode() -> Result<(), String> {
    let samples = balanced_sample_setting(
        "TMON_GHOSTTY_MEMORY_SAMPLES",
        DEFAULT_SAMPLES,
        MIN_SAMPLES,
        MAX_SAMPLES,
    )?;
    let current_exe = env::current_exe().map_err(|error| error.to_string())?;
    let mut payload_bytes = HashMap::<Workload, u64>::new();
    let mut results = BTreeMap::<(Workload, Engine), Vec<ChildResult>>::new();

    for round in 0..samples {
        let cases = benchmark_cases(round);
        let mut round_checks = HashMap::<Workload, ChildResult>::new();
        for case in cases {
            let child = run_child_process(&current_exe, case.engine, case.workload)?;
            if child.history != case.workload.expected_history() {
                return Err(format!(
                    "{} {} reported history {} but expected {}",
                    case.engine.name(),
                    case.workload.name(),
                    child.history,
                    case.workload.expected_history()
                ));
            }
            let expected_compression = if case.engine == Engine::GhosttyCompressed {
                CompressionReport::Complete
            } else {
                CompressionReport::None
            };
            if child.compression != expected_compression {
                return Err(format!(
                    "{} {} reported compression={} but expected {}",
                    case.engine.name(),
                    case.workload.name(),
                    child.compression.as_str(),
                    expected_compression.as_str()
                ));
            }
            match payload_bytes.entry(case.workload) {
                std::collections::hash_map::Entry::Vacant(entry) => {
                    entry.insert(child.bytes);
                }
                std::collections::hash_map::Entry::Occupied(entry) => {
                    if *entry.get() != child.bytes {
                        return Err(format!(
                            "{} payload bytes changed: expected {}, got {} for {}",
                            case.workload.name(),
                            entry.get(),
                            child.bytes,
                            case.engine.name()
                        ));
                    }
                }
            }
            if let Some(baseline) = round_checks.get(&case.workload) {
                if baseline.cursor_row != child.cursor_row
                    || baseline.cursor_col != child.cursor_col
                    || baseline.digest != child.digest
                {
                    return Err(format!(
                        "full semantic mismatch for {} in round {}: baseline cursor={},{} digest={:016x}; {} cursor={},{} digest={:016x}",
                        case.workload.name(),
                        round,
                        baseline.cursor_row,
                        baseline.cursor_col,
                        baseline.digest,
                        case.engine.name(),
                        child.cursor_row,
                        child.cursor_col,
                        child.digest
                    ));
                }
            } else {
                round_checks.insert(case.workload, child);
            }
            results
                .entry((case.workload, case.engine))
                .or_default()
                .push(child);
        }
    }

    let ghostty_empty_median = stats(
        results
            .get(&(Workload::Empty, Engine::Ghostty))
            .ok_or_else(|| "missing ghostty empty results".to_string())?,
    )
    .median;
    let tmon_empty_median = stats(
        results
            .get(&(Workload::Empty, Engine::Tmon))
            .ok_or_else(|| "missing tmon empty results".to_string())?,
    )
    .median;
    let alacritty_empty_median = stats(
        results
            .get(&(Workload::Empty, Engine::Alacritty))
            .ok_or_else(|| "missing alacritty empty results".to_string())?,
    )
    .median;

    println!(
        "metadata samples={} cols={} rows={} scrollback_limit={} scrollback_total_rows={} ghostty_prefix={} exe={}",
        samples,
        COLS,
        ROWS,
        SCROLLBACK_LIMIT,
        SCROLLBACK_TOTAL_ROWS,
        env::var("GHOSTTY_VT_PREFIX").unwrap_or_else(|_| "<unset>".to_string()),
        current_exe.display()
    );

    let mut json_results = Vec::new();
    for workload in Workload::ALL {
        println!(
            "workload={} payload_bytes={}",
            workload.name(),
            payload_bytes[&workload]
        );
        for &engine in workload.benchmark_engines() {
            let engine_results = results.get(&(workload, engine)).ok_or_else(|| {
                format!("missing {} results for {}", engine.name(), workload.name())
            })?;
            let engine_stats = stats(engine_results);
            let empty_baseline = match engine.baseline_empty_engine() {
                Engine::Tmon => tmon_empty_median,
                Engine::Alacritty => alacritty_empty_median,
                Engine::Ghostty | Engine::GhosttyCompressed => ghostty_empty_median,
            };
            let delta_vs_empty = engine_stats.median - empty_baseline;
            println!(
                "  engine={} median_mib={:.2} p05_mib={:.2} p95_mib={:.2} delta_vs_empty_mib={:.2} raw_bytes={}",
                engine.name(),
                bytes_to_mib(engine_stats.median),
                bytes_to_mib(engine_stats.p05),
                bytes_to_mib(engine_stats.p95),
                bytes_to_mib(delta_vs_empty),
                RawBytes(engine_results)
            );
            println!(
                "{MEMORY_RECORD_PREFIX} schema=1 workload={} engine={} compression={} samples={} payload_bytes={} median_bytes={:.3} p05_bytes={:.3} p95_bytes={:.3} delta_vs_empty_bytes={:.3} raw_bytes={}",
                workload.name(),
                engine.name(),
                engine_results[0].compression.as_str(),
                engine_results.len(),
                payload_bytes[&workload],
                engine_stats.median,
                engine_stats.p05,
                engine_stats.p95,
                delta_vs_empty,
                RawBytes(engine_results),
            );
            json_results.push(MemoryEngineResult {
                workload: workload.name(),
                engine: engine.name(),
                compression: engine_results[0].compression.as_str(),
                payload_bytes: payload_bytes[&workload],
                samples: engine_results
                    .iter()
                    .enumerate()
                    .map(|(sample_index, result)| MemorySample {
                        sample_index,
                        order_position: memory_order_position(sample_index, engine),
                        footprint_bytes: result.footprint_bytes,
                    })
                    .collect(),
                median_bytes: engine_stats.median,
                p05_bytes: engine_stats.p05,
                p95_bytes: engine_stats.p95,
                delta_vs_empty_bytes: delta_vs_empty,
            });
        }
    }

    write_json_output(&MemoryReport {
        schema_version: 1,
        mode: "memory",
        parameters: MemoryParameters {
            samples,
            cols: COLS,
            rows: ROWS,
            scrollback_limit: SCROLLBACK_LIMIT,
            scrollback_total_rows: SCROLLBACK_TOTAL_ROWS,
        },
        results: json_results,
    })?;

    let mut non_wins = Vec::new();
    for workload in Workload::ALL {
        let tmon = stats(
            results
                .get(&(workload, Engine::Tmon))
                .ok_or_else(|| format!("missing tmon results for {}", workload.name()))?,
        );
        for &engine in workload.benchmark_engines() {
            if engine == Engine::Tmon {
                continue;
            }
            let comparison = stats(results.get(&(workload, engine)).ok_or_else(|| {
                format!("missing {} results for {}", engine.name(), workload.name())
            })?);
            if tmon.median >= comparison.median {
                non_wins.push(format!(
                    "{}: tmon {:.0} bytes >= {} {:.0} bytes",
                    workload.name(),
                    tmon.median,
                    engine.name(),
                    comparison.median
                ));
            }
        }
    }
    if non_wins.is_empty() {
        println!(
            "TERMINAL_ENGINE_BENCH_SUMMARY schema=1 mode=memory tmon_strictly_smallest_everywhere=true"
        );
    } else {
        println!(
            "TERMINAL_ENGINE_BENCH_SUMMARY schema=1 mode=memory tmon_strictly_smallest_everywhere=false"
        );
        println!(
            "Tmon was not strictly smaller in every memory comparison:\n{}",
            non_wins.join("\n")
        );
    }
    Ok(())
}

fn memory_order_position(sample_index: usize, engine: Engine) -> usize {
    if engine == Engine::GhosttyCompressed {
        return 3;
    }
    primary_engine_order(sample_index)
        .iter()
        .position(|candidate| *candidate == engine)
        .expect("every primary engine is present in every order")
}

fn run_child_process(
    current_exe: &std::path::Path,
    engine: Engine,
    workload: Workload,
) -> Result<ChildResult, String> {
    let output = Command::new(current_exe)
        .arg("--child")
        .arg(engine.name())
        .arg(workload.name())
        .output()
        .map_err(|error| {
            format!(
                "failed to launch child for {} {}: {error}",
                engine.name(),
                workload.name()
            )
        })?;
    let stdout = String::from_utf8(output.stdout)
        .map_err(|error| format!("child stdout was not UTF-8: {error}"))?;
    let stderr = String::from_utf8(output.stderr)
        .map_err(|error| format!("child stderr was not UTF-8: {error}"))?;
    if !output.status.success() {
        return Err(format!(
            "child failed for {} {} with {}\nstdout:\n{}\nstderr:\n{}",
            engine.name(),
            workload.name(),
            ExitStatusDisplay(output.status),
            stdout,
            stderr
        ));
    }
    let mut parsed = None;
    for line in stdout.lines() {
        if line.starts_with(CHILD_PREFIX) {
            if parsed.is_some() {
                return Err(format!(
                    "child for {} {} printed multiple machine-readable lines\nstdout:\n{}",
                    engine.name(),
                    workload.name(),
                    stdout
                ));
            }
            parsed = Some(parse_child_line(line)?);
        }
    }
    parsed.ok_or_else(|| {
        format!(
            "child for {} {} did not print a machine-readable line\nstdout:\n{}\nstderr:\n{}",
            engine.name(),
            workload.name(),
            stdout,
            stderr
        )
    })
}

fn parse_child_line(line: &str) -> Result<ChildResult, String> {
    let mut fields = HashMap::<&str, &str>::new();
    let mut tokens = line.split_whitespace();
    if tokens.next() != Some(CHILD_PREFIX) {
        return Err(format!("invalid child line prefix in {line:?}"));
    }
    for token in tokens {
        let (key, value) = token
            .split_once('=')
            .ok_or_else(|| format!("invalid child token {token:?}"))?;
        if fields.insert(key, value).is_some() {
            return Err(format!("duplicate child field {key:?}"));
        }
    }
    let cursor = fields
        .get("cursor")
        .ok_or_else(|| "child line missing cursor".to_string())?;
    let (cursor_row, cursor_col) = cursor
        .split_once(',')
        .ok_or_else(|| format!("invalid cursor field {cursor:?}"))?;
    Ok(ChildResult {
        bytes: parse_field(&fields, "bytes")?,
        history: parse_field(&fields, "history")?,
        cursor_row: cursor_row
            .parse::<usize>()
            .map_err(|error| format!("invalid cursor row: {error}"))?,
        cursor_col: cursor_col
            .parse::<usize>()
            .map_err(|error| format!("invalid cursor col: {error}"))?,
        digest: u64::from_str_radix(
            fields
                .get("full_digest")
                .ok_or_else(|| "child line missing full_digest".to_string())?,
            16,
        )
        .map_err(|error| format!("invalid full_digest: {error}"))?,
        compression: CompressionReport::parse(
            fields
                .get("compression")
                .ok_or_else(|| "child line missing compression".to_string())?,
        )?,
        footprint_bytes: parse_field(&fields, "footprint")?,
    })
}

fn parse_field<T>(fields: &HashMap<&str, &str>, key: &str) -> Result<T, String>
where
    T: std::str::FromStr,
    T::Err: fmt::Display,
{
    fields
        .get(key)
        .ok_or_else(|| format!("child line missing {key}"))?
        .parse::<T>()
        .map_err(|error| format!("invalid {key}: {error}"))
}

fn setting(name: &str, default: usize, min: usize, max: usize) -> Result<usize, String> {
    let value = match env::var(name) {
        Ok(value) => value
            .parse::<usize>()
            .map_err(|error| format!("{name} must be an integer: {error}"))?,
        Err(env::VarError::NotPresent) => default,
        Err(error) => return Err(format!("could not read {name}: {error}")),
    };
    if !(min..=max).contains(&value) {
        return Err(format!(
            "{name} must be between {min} and {max}, got {value}"
        ));
    }
    Ok(value)
}

fn balanced_sample_setting(
    name: &str,
    default: usize,
    min: usize,
    max: usize,
) -> Result<usize, String> {
    let value = setting(name, default, min, max)?;
    require_balanced_sample_count(name, value)?;
    Ok(value)
}

fn require_balanced_sample_count(name: &str, value: usize) -> Result<(), String> {
    if !value.is_multiple_of(PRIMARY_ENGINE_ORDERS.len()) {
        return Err(format!(
            "{name} must be a multiple of {} so all three-engine orders are balanced, got {value}",
            PRIMARY_ENGINE_ORDERS.len()
        ));
    }
    Ok(())
}

fn write_json_output(report: &impl Serialize) -> Result<(), String> {
    let path = match env::var_os(JSON_OUTPUT_ENV) {
        Some(path) if !path.is_empty() => path,
        Some(_) => return Err(format!("{JSON_OUTPUT_ENV} must not be empty")),
        None => return Ok(()),
    };
    let mut json = serde_json::to_vec_pretty(report)
        .map_err(|error| format!("could not serialize benchmark JSON: {error}"))?;
    json.push(b'\n');
    std::fs::write(Path::new(&path), json).map_err(|error| {
        format!(
            "could not write benchmark JSON to {}: {error}",
            Path::new(&path).display()
        )
    })
}

fn benchmark_cases(round: usize) -> Vec<BenchmarkCase> {
    let mut cases = Vec::new();
    for workload in Workload::ALL {
        for engine in primary_engine_order(round) {
            cases.push(BenchmarkCase { engine, workload });
        }
        if workload.needs_compressed_ghostty() {
            cases.push(BenchmarkCase {
                engine: Engine::GhosttyCompressed,
                workload,
            });
        }
    }
    cases
}

fn run_child_mode(engine: Engine, workload: Workload) -> Result<(), String> {
    let result = match engine {
        Engine::Tmon => run_tmon_child(workload)?,
        Engine::Alacritty => run_alacritty_child(workload)?,
        Engine::Ghostty | Engine::GhosttyCompressed => run_ghostty_child(engine, workload)?,
    };
    println!(
        "{CHILD_PREFIX} bytes={} history={} cursor={},{} full_digest={:016x} compression={} footprint={}",
        result.bytes,
        result.history,
        result.cursor_row,
        result.cursor_col,
        result.digest,
        result.compression.as_str(),
        result.footprint_bytes
    );
    Ok(())
}

fn run_alacritty_child(workload: Workload) -> Result<ChildResult, String> {
    let mut terminal = DirectAlacritty::new();
    let bytes = feed_workload(workload, |chunk| {
        terminal.feed(chunk);
        Ok(())
    })?;
    let state = terminal.state()?;
    let footprint = current_ri_phys_footprint()?;
    let digest = terminal.semantic_digest()?;
    Ok(ChildResult {
        bytes,
        history: state.history,
        cursor_row: state.cursor_row,
        cursor_col: state.cursor_col,
        digest,
        compression: CompressionReport::None,
        footprint_bytes: footprint,
    })
}

fn run_tmon_child(workload: Workload) -> Result<ChildResult, String> {
    let terminal = Terminal::new_display(
        Size {
            cols: COLS,
            rows: ROWS,
            cell_width: 9.0,
            cell_height: 18.0,
        },
        Config {
            scrollback_history: SCROLLBACK_LIMIT,
            ..Config::default()
        },
    );
    let bytes = feed_workload(workload, |chunk| {
        terminal.feed_output(chunk);
        Ok(())
    })?;
    let state = terminal.state_snapshot();
    let cursor = state
        .cursor_state
        .ok_or_else(|| "tmon child has no cursor state".to_string())?;
    let footprint = current_ri_phys_footprint()?;
    let digest = tmon_semantic_digest(&terminal)?;
    Ok(ChildResult {
        bytes,
        history: state.history_size,
        cursor_row: cursor.row,
        cursor_col: cursor.col,
        digest,
        compression: CompressionReport::None,
        footprint_bytes: footprint,
    })
}

fn run_ghostty_child(engine: Engine, workload: Workload) -> Result<ChildResult, String> {
    let terminal = GhosttyTerminal::new()?;
    let bytes = feed_workload(workload, |chunk| terminal.feed(chunk))?;
    let compression = match engine {
        Engine::Ghostty => CompressionReport::None,
        Engine::GhosttyCompressed => terminal.compress_full()?,
        Engine::Tmon | Engine::Alacritty => unreachable!(),
    };
    let state = terminal.query_state()?;
    let history = state
        .total
        .checked_sub(state.len)
        .ok_or_else(|| "ghostty scrollbar underflowed".to_string())?;
    let footprint = current_ri_phys_footprint()?;
    let digest = terminal.semantic_digest()?;
    Ok(ChildResult {
        bytes,
        history: usize::try_from(history)
            .map_err(|_| "ghostty history overflowed usize".to_string())?,
        cursor_row: usize::from(state.cursor_y),
        cursor_col: usize::from(state.cursor_x),
        digest,
        compression,
        footprint_bytes: footprint,
    })
}

fn feed_workload(
    workload: Workload,
    mut feed: impl FnMut(&[u8]) -> Result<(), String>,
) -> Result<u64, String> {
    if workload == Workload::Empty {
        return Ok(0);
    }
    let mut total_bytes = 0u64;
    if workload.enters_alternate_screen() {
        feed(ENTER_ALTERNATE_SCREEN)?;
        total_bytes = ENTER_ALTERNATE_SCREEN.len() as u64;
    }
    if let Some((trace, repetitions)) = workload.trace() {
        for _ in 0..repetitions {
            feed(trace)?;
            total_bytes = total_bytes
                .checked_add(trace.len() as u64)
                .ok_or_else(|| "payload byte count overflowed".to_string())?;
        }
        return Ok(total_bytes);
    }
    let mut row = String::new();
    for row_index in 0..workload.row_count() {
        row.clear();
        workload.build_row(row_index, &mut row);
        validate_payload_row(&row, workload, row_index)?;
        if row_index + 1 != workload.row_count() {
            row.push_str("\r\n");
        }
        feed(row.as_bytes())?;
        total_bytes = total_bytes
            .checked_add(
                u64::try_from(row.len())
                    .map_err(|_| "payload row length overflowed".to_string())?,
            )
            .ok_or_else(|| "payload byte count overflowed".to_string())?;
    }
    Ok(total_bytes)
}

fn validate_payload_row(row: &str, workload: Workload, row_index: usize) -> Result<(), String> {
    if row.contains(['\r', '\n']) {
        return Err(format!(
            "payload row contains CR/LF for {} row {}",
            workload.name(),
            row_index
        ));
    }
    let width = conservative_visible_width(row);
    if width >= usize::from(COLS) {
        return Err(format!(
            "payload row width {} is not safely below {} for {} row {}",
            width,
            COLS,
            workload.name(),
            row_index
        ));
    }
    Ok(())
}

fn current_ri_phys_footprint() -> Result<u64, String> {
    let mut bytes = 0;
    let code = unsafe { tmon_ghostty_current_ri_phys_footprint(&mut bytes) };
    if code == SHIM_OK {
        Ok(bytes)
    } else {
        Err(format!(
            "ri_phys_footprint query failed: {}",
            shim_error_name(code)
        ))
    }
}

fn tmon_semantic_digest(terminal: &Terminal) -> Result<u64, String> {
    let (first_line, last_line) = terminal.line_bounds();
    if last_line < first_line {
        return Err(format!(
            "invalid tmon line bounds {}..{}",
            first_line, last_line
        ));
    }
    let total_rows_i64 = i64::from(last_line) - i64::from(first_line) + 1;
    let total_rows = usize::try_from(total_rows_i64)
        .map_err(|_| format!("invalid total row count {total_rows_i64}"))?;
    let mut hash = Fnv1a64::new();
    hash.write_u64(total_rows as u64);
    hash.write_u16(COLS);
    hash.write_u64(total_rows as u64);

    for row_offset in full_row_offsets(total_rows) {
        let line = first_line
            .checked_add(
                i32::try_from(row_offset).map_err(|_| "row offset overflowed i32".to_string())?,
            )
            .ok_or_else(|| "line computation overflowed".to_string())?;
        hash.write_u64(row_offset as u64);
        let mut seen_columns = 0usize;
        let mut callback_error = None::<String>;
        let range = terminal.for_each_line_cell_range(
            line,
            line,
            |range, visited_line, col, cell, combining| {
                if callback_error.is_some() {
                    return;
                }
                if range.columns != usize::from(COLS) {
                    callback_error = Some(format!(
                        "tmon digest expected {} columns, got {}",
                        COLS, range.columns
                    ));
                    return;
                }
                if visited_line != line {
                    callback_error = Some(format!(
                        "tmon digest visited unexpected line {} while hashing {}",
                        visited_line, line
                    ));
                    return;
                }
                if col != seen_columns {
                    callback_error = Some(format!(
                        "tmon digest expected column {}, got {} on line {}",
                        seen_columns, col, line
                    ));
                    return;
                }
                hash_tmon_cell(&mut hash, cell, combining);
                seen_columns += 1;
            },
        );
        if let Some(error) = callback_error {
            return Err(error);
        }
        if range.columns != usize::from(COLS) {
            return Err(format!(
                "tmon digest range expected {} columns, got {}",
                COLS, range.columns
            ));
        }
        if seen_columns != usize::from(COLS) {
            return Err(format!(
                "tmon digest expected {} cells on line {}, saw {}",
                COLS, line, seen_columns
            ));
        }
    }

    Ok(hash.finish())
}

fn hash_tmon_cell(hash: &mut Fnv1a64, cell: &Cell, combining: Option<Combining<'_>>) {
    let role = if cell.leading_wide_spacer() {
        2
    } else if cell.wide_spacer() {
        1
    } else {
        0
    };
    hash.write_u8(role);

    let mut codepoints = Vec::new();
    if cell.character != '\0' {
        codepoints.push(cell.character as u32);
    }
    if let Some(combining) = combining {
        match combining {
            Combining::Character(character) => codepoints.push(character as u32),
            Combining::Text(text) => {
                codepoints.extend(text.chars().map(|character| character as u32))
            }
        }
    }
    if codepoints.is_empty() {
        codepoints.push(0x20);
    }
    hash.write_u64(codepoints.len() as u64);
    for codepoint in codepoints {
        hash.write_u32(codepoint);
    }

    hash_tmon_color(hash, Some(cell.foreground));
    hash_tmon_color(hash, Some(cell.background));
    hash_tmon_color(hash, cell.underline_color);
    hash.write_u8(u8::from(cell.attributes.bold()));
    hash.write_u8(u8::from(cell.attributes.dim()));
    hash.write_u8(u8::from(cell.attributes.italic()));
    hash.write_u8(u8::from(cell.attributes.inverse()));
    hash.write_u8(u8::from(cell.attributes.hidden()));
    hash.write_u8(u8::from(cell.attributes.strikethrough()));
    hash.write_u32(underline_style_code(cell.attributes.underline_style()));
}

fn hash_tmon_color(hash: &mut Fnv1a64, color: Option<Color>) {
    match color {
        None | Some(Color::Default) => hash.write_u8(0),
        Some(Color::Indexed(index)) => {
            hash.write_u8(1);
            hash.write_u8(index);
        }
        Some(Color::Rgb { r, g, b }) => {
            hash.write_u8(2);
            hash.write_u8(r);
            hash.write_u8(g);
            hash.write_u8(b);
        }
    }
}

const fn underline_style_code(style: UnderlineStyle) -> u32 {
    match style {
        UnderlineStyle::None => 0,
        UnderlineStyle::Single => 1,
        UnderlineStyle::Double => 2,
        UnderlineStyle::Curly => 3,
        UnderlineStyle::Dotted => 4,
        UnderlineStyle::Dashed => 5,
    }
}

fn full_row_offsets(total_rows: usize) -> std::ops::Range<usize> {
    0..total_rows
}

fn build_plain_row(row_index: usize, out: &mut String) {
    let hash = mix64(row_index as u64 ^ 0x0f51_4f7d_2d3c_9ab1);
    let mut letters = String::new();
    append_codebook(&mut letters, hash, 12);
    let _ = write!(
        out,
        "plain {:05} h={:08x} code={} grp={:03}-{:03}-{:03} tail={:02}",
        row_index,
        hash as u32,
        letters,
        (hash >> 5) % 997,
        (hash >> 17) % 997,
        (hash >> 29) % 997,
        (hash >> 41) % 89
    );
}

fn build_unicode_row(row_index: usize, out: &mut String) {
    let hash = mix64(row_index as u64 ^ 0x92c2_5f37_18aa_0d71);
    let marks = [
        "e\u{0301}",
        "a\u{0308}",
        "o\u{0302}",
        "u\u{0304}",
        "n\u{0303}",
        "i\u{0307}",
    ];
    let wide = ["界", "漢", "語", "猫", "山", "海", "星", "橋"];
    let _ = write!(
        out,
        "uni {:05} {} {} {} {} pair={}{} mix={:04x}",
        row_index,
        marks[(hash as usize) % marks.len()],
        wide[((hash >> 7) as usize) % wide.len()],
        marks[((hash >> 13) as usize) % marks.len()],
        wide[((hash >> 19) as usize) % wide.len()],
        wide[((hash >> 25) as usize) % wide.len()],
        wide[((hash >> 31) as usize) % wide.len()],
        (hash >> 40) as u16
    );
}

fn build_styled_row(row_index: usize, out: &mut String) {
    let hash = mix64(row_index as u64 ^ 0x38d8_7bce_5fa1_2261);
    let underline = 1 + (hash % 5) as u8;
    let fg0 = 16 + (hash % 200) as u8;
    let bg0 = 16 + ((hash >> 8) % 200) as u8;
    let ul0 = 16 + ((hash >> 16) % 200) as u8;
    let _ = write!(
        out,
        "\x1b[38;5;{fg0};48;5;{bg0};58;5;{ul0};1;4:{underline}mS{row_index:05}"
    );
    append_codebook(out, hash, 6);
    out.push_str("\x1b[0m ");

    let r1 = ((hash >> 24) & 0xff) as u8;
    let g1 = ((hash >> 32) & 0xff) as u8;
    let b1 = ((hash >> 40) & 0xff) as u8;
    let r2 = ((hash >> 48) & 0xff) as u8;
    let g2 = ((hash >> 12) & 0xff) as u8;
    let b2 = ((hash >> 20) & 0xff) as u8;
    let ur = ((hash >> 28) & 0xff) as u8;
    let ug = ((hash >> 36) & 0xff) as u8;
    let ub = ((hash >> 44) & 0xff) as u8;
    let _ = write!(
        out,
        "\x1b[38:2::{r1}:{g1}:{b1};48:2::{r2}:{g2}:{b2};58:2::{ur}:{ug}:{ub};2;3;7mR{:04x}\x1b[0m ",
        (hash >> 8) as u16
    );

    let fg2 = 16 + ((hash >> 4) % 200) as u8;
    let bg2 = 16 + ((hash >> 10) % 200) as u8;
    let _ = write!(
        out,
        "\x1b[38;5;{fg2};48;5;{bg2};8;9mI{:03}\x1b[0m",
        (hash >> 52) % 997
    );
}

fn build_mixed_row(row_index: usize, out: &mut String) {
    build_styled_row(row_index, out);
    let hash = mix64(row_index as u64 ^ 0x74b3_ae91_0d5f_c224);
    let wide = ["界", "漢", "語", "猫", "山", "海", "星", "橋"];
    let marks = [
        "e\u{0301}",
        "a\u{0308}",
        "o\u{0302}",
        "u\u{0304}",
        "n\u{0303}",
    ];
    let _ = write!(
        out,
        " mix {} {}{}",
        marks[(hash as usize) % marks.len()],
        wide[((hash >> 11) as usize) % wide.len()],
        wide[((hash >> 19) as usize) % wide.len()]
    );
}

fn append_codebook(out: &mut String, mut hash: u64, len: usize) {
    const TABLE: &[u8] = b"ABCDEFGHJKLMNPQRSTUVWXYZ23456789";
    for _ in 0..len {
        out.push(char::from(TABLE[(hash as usize) % TABLE.len()]));
        hash = mix64(hash ^ 0x9e37_79b9_7f4a_7c15);
    }
}

fn mix64(mut value: u64) -> u64 {
    value = value.wrapping_add(0x9e37_79b9_7f4a_7c15);
    value = (value ^ (value >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    value = (value ^ (value >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    value ^ (value >> 31)
}

fn conservative_visible_width(row: &str) -> usize {
    let mut width = 0usize;
    let mut chars = row.chars();
    while let Some(character) = chars.next() {
        if character == '\u{1b}' {
            if matches!(chars.next(), Some('[')) {
                for next in chars.by_ref() {
                    if ('\u{40}'..='\u{7e}').contains(&next) {
                        break;
                    }
                }
            }
            continue;
        }
        if is_combining_mark(character) {
            continue;
        }
        width += if character.is_ascii() { 1 } else { 2 };
    }
    width
}

const fn is_combining_mark(character: char) -> bool {
    matches!(
        character as u32,
        0x0300..=0x036f | 0x1ab0..=0x1aff | 0x1dc0..=0x1dff | 0x20d0..=0x20ff | 0xfe20..=0xfe2f
    )
}

fn shim_error_name(code: i32) -> &'static str {
    match code {
        SHIM_OK => "ok",
        SHIM_INVALID_ARGUMENT => "invalid-argument",
        SHIM_GHOSTTY_ERROR => "ghostty-error",
        SHIM_OUT_OF_MEMORY => "out-of-memory",
        SHIM_PROC_ERROR => "proc-error",
        _ => "unknown",
    }
}

struct ExitStatusDisplay(ExitStatus);

impl fmt::Display for ExitStatusDisplay {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.0.code() {
            Some(code) => write!(formatter, "exit code {code}"),
            None => formatter.write_str("terminated by signal"),
        }
    }
}

struct RawBytes<'a>(&'a [ChildResult]);

impl fmt::Display for RawBytes<'_> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("[")?;
        for (index, result) in self.0.iter().enumerate() {
            if index != 0 {
                formatter.write_str(",")?;
            }
            write!(formatter, "{}", result.footprint_bytes)?;
        }
        formatter.write_str("]")
    }
}

#[derive(Clone, Copy)]
struct Stats {
    median: f64,
    p05: f64,
    p95: f64,
}

fn stats(results: &[ChildResult]) -> Stats {
    let mut samples = results
        .iter()
        .map(|result| result.footprint_bytes as f64)
        .collect::<Vec<_>>();
    samples.sort_by(|left, right| left.partial_cmp(right).unwrap());
    Stats {
        median: percentile_sorted(&samples, 0.5),
        p05: percentile_sorted(&samples, 0.05),
        p95: percentile_sorted(&samples, 0.95),
    }
}

fn percentile_sorted(sorted: &[f64], percentile: f64) -> f64 {
    if sorted.is_empty() {
        return 0.0;
    }
    if sorted.len() == 1 {
        return sorted[0];
    }
    let clamped = percentile.clamp(0.0, 1.0);
    let position = clamped * (sorted.len() - 1) as f64;
    let lower = position.floor() as usize;
    let upper = position.ceil() as usize;
    if lower == upper {
        sorted[lower]
    } else {
        let fraction = position - lower as f64;
        sorted[lower] + (sorted[upper] - sorted[lower]) * fraction
    }
}

fn bytes_to_mib(bytes: f64) -> f64 {
    bytes / (1024.0 * 1024.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn workloads_have_expected_names_and_row_counts() {
        let expected = [
            (Workload::Empty, "empty", 0),
            (Workload::ScreenPlain, "screen-plain", 40),
            (Workload::ScreenStyled, "screen-styled", 40),
            (Workload::ScreenUnicode, "screen-unicode", 40),
            (Workload::ScreenMixed, "screen-mixed", 40),
            (
                Workload::AlternatePlain,
                "alternate-plain",
                ALTERNATE_TOTAL_ROWS,
            ),
            (
                Workload::AlternateMixed,
                "alternate-mixed",
                ALTERNATE_TOTAL_ROWS,
            ),
            (Workload::TuiRedraw, "tui-redraw", 0),
            (Workload::CursorEdits, "cursor-edits", 0),
            (Workload::ScrollbackPlain, "scrollback-plain", 10_040),
            (Workload::ScrollbackStyled, "scrollback-styled", 10_040),
            (Workload::ScrollbackUnicode, "scrollback-unicode", 10_040),
            (Workload::ScrollbackMixed, "scrollback-mixed", 10_040),
        ];
        for (workload, name, rows) in expected {
            assert_eq!(workload.name(), name);
            assert_eq!(workload.row_count(), rows);
            assert_eq!(Workload::parse(name).unwrap(), workload);
        }
    }

    #[test]
    fn percentile_and_median_are_interpolated() {
        let sorted = [10.0, 20.0, 30.0, 40.0];
        assert!((percentile_sorted(&sorted, 0.5) - 25.0).abs() < f64::EPSILON);
        assert!((percentile_sorted(&sorted, 0.05) - 11.5).abs() < f64::EPSILON);
        assert!((percentile_sorted(&sorted, 0.95) - 38.5).abs() < f64::EPSILON);
    }

    #[test]
    fn semantic_verification_selects_every_logical_row() {
        assert_eq!(full_row_offsets(0).collect::<Vec<_>>(), Vec::<usize>::new());
        assert_eq!(full_row_offsets(5).collect::<Vec<_>>(), vec![0, 1, 2, 3, 4]);
        let rows = full_row_offsets(SCROLLBACK_TOTAL_ROWS);
        assert_eq!(rows.len(), SCROLLBACK_TOTAL_ROWS);
        assert_eq!(rows.start, 0);
        assert_eq!(rows.end, 10_040);
    }

    #[test]
    fn three_engine_orders_are_complete_and_position_balanced() {
        let unique = PRIMARY_ENGINE_ORDERS
            .into_iter()
            .collect::<std::collections::HashSet<_>>();
        assert_eq!(unique.len(), 6);

        for engine in [Engine::Tmon, Engine::Alacritty, Engine::Ghostty] {
            for position in 0..3 {
                assert_eq!(
                    PRIMARY_ENGINE_ORDERS
                        .iter()
                        .filter(|order| order[position] == engine)
                        .count(),
                    2,
                    "{} was not balanced at position {position}",
                    engine.name()
                );
            }
        }
        assert!(require_balanced_sample_count("SAMPLES", 12).is_ok());
        assert!(require_balanced_sample_count("SAMPLES", 10).is_err());

        let cases = benchmark_cases(0);
        assert_eq!(
            cases
                .iter()
                .filter(|case| case.engine == Engine::GhosttyCompressed)
                .count(),
            4
        );
        assert!(!cases.iter().any(|case| {
            case.engine == Engine::GhosttyCompressed && !case.workload.needs_compressed_ghostty()
        }));
    }

    #[test]
    fn child_result_parser_requires_full_digest_and_unique_fields() {
        let parsed = parse_child_line(concat!(
            "TMON_GHOSTTY_MEMORY_CHILD bytes=42 history=7 cursor=3,9 ",
            "full_digest=0123456789abcdef compression=none footprint=99"
        ))
        .unwrap();
        assert_eq!(
            parsed,
            ChildResult {
                bytes: 42,
                history: 7,
                cursor_row: 3,
                cursor_col: 9,
                digest: 0x0123_4567_89ab_cdef,
                compression: CompressionReport::None,
                footprint_bytes: 99,
            }
        );
        assert!(
            parse_child_line(concat!(
                "TMON_GHOSTTY_MEMORY_CHILD bytes=1 bytes=2 history=0 cursor=0,0 ",
                "full_digest=0 compression=none footprint=1"
            ))
            .unwrap_err()
            .contains("duplicate")
        );
        assert!(
            parse_child_line(concat!(
                "TMON_GHOSTTY_MEMORY_CHILD bytes=1 history=0 cursor=0,0 ",
                "sampled_digest=0 compression=none footprint=1"
            ))
            .unwrap_err()
            .contains("full_digest")
        );
    }

    #[test]
    fn memory_json_contains_raw_process_and_order_fields() {
        let report = MemoryReport {
            schema_version: 1,
            mode: "memory",
            parameters: MemoryParameters {
                samples: 6,
                cols: COLS,
                rows: ROWS,
                scrollback_limit: SCROLLBACK_LIMIT,
                scrollback_total_rows: SCROLLBACK_TOTAL_ROWS,
            },
            results: vec![MemoryEngineResult {
                workload: "test",
                engine: "tmon",
                compression: "none",
                payload_bytes: 10,
                samples: vec![MemorySample {
                    sample_index: 0,
                    order_position: 2,
                    footprint_bytes: 42,
                }],
                median_bytes: 42.0,
                p05_bytes: 42.0,
                p95_bytes: 42.0,
                delta_vs_empty_bytes: 1.0,
            }],
        };

        let json = serde_json::to_value(report).unwrap();
        assert_eq!(json["results"][0]["samples"][0]["footprint_bytes"], 42);
        assert_eq!(json["results"][0]["samples"][0]["sample_index"], 0);
        assert_eq!(json["results"][0]["samples"][0]["order_position"], 2);
    }

    #[test]
    fn generated_payload_rows_are_safe() {
        for workload in Workload::ALL {
            let mut row = String::new();
            for row_index in 0..workload.row_count() {
                row.clear();
                workload.build_row(row_index, &mut row);
                assert!(
                    !row.contains(['\r', '\n']),
                    "{} row {} contains CR/LF",
                    workload.name(),
                    row_index
                );
                assert!(
                    conservative_visible_width(&row) < usize::from(COLS),
                    "{} row {} width {} is too wide: {}",
                    workload.name(),
                    row_index,
                    conservative_visible_width(&row),
                    row
                );
            }
        }
    }

    #[test]
    fn interactive_traces_cover_alternate_and_edit_paths() {
        assert!(Workload::TuiRedraw.enters_alternate_screen());
        assert!(!Workload::CursorEdits.enters_alternate_screen());
        assert_eq!(Workload::TuiRedraw.trace(), Some((TUI_REDRAW, 64)));
        assert_eq!(Workload::CursorEdits.trace(), Some((CURSOR_EDITS, 128)));
        assert!(TUI_REDRAW.windows(2).any(|window| window == b"[H"));
        assert!(CURSOR_EDITS.windows(2).any(|window| window == b"[K"));
    }
}
