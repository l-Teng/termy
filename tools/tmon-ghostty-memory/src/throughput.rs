use std::{hint::black_box, time::Instant};

use serde::Serialize;

use super::*;

const DEFAULT_SAMPLES: usize = 12;
const WARMUP_MIB: usize = 2;
const CALIBRATION_PILOT_MIB: usize = 64;
const CALIBRATION_TARGET_SECONDS: f64 = 0.500;
const MINIMUM_TIMED_SAMPLE_SECONDS: f64 = 0.250;
const MIN_SAMPLES: usize = 6;
const MAX_SAMPLES: usize = 60;
const MIN_TARGET_MIB: usize = 1;
const MAX_TARGET_MIB: usize = 2 * 1024;
const FEED_CHUNK_BYTES: usize = 64 * 1024;
const THROUGHPUT_RECORD_PREFIX: &str = "TERMINAL_ENGINE_BENCH_THROUGHPUT";
const RATIO_RECORD_PREFIX: &str = "TERMINAL_ENGINE_BENCH_RATIO";
const CALIBRATION_RECORD_PREFIX: &str = "TERMINAL_ENGINE_BENCH_CALIBRATION";

const WORKLOADS: [Workload; 8] = [
    Workload::ScrollbackPlain,
    Workload::ScrollbackStyled,
    Workload::ScrollbackUnicode,
    Workload::ScrollbackMixed,
    Workload::AlternatePlain,
    Workload::AlternateMixed,
    Workload::TuiRedraw,
    Workload::CursorEdits,
];

#[derive(Clone, Copy)]
struct Measurement {
    bytes: u64,
    elapsed_seconds: f64,
    retained_history_rows: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct SemanticResult {
    bytes: u64,
    history: usize,
    cursor_row: usize,
    cursor_col: usize,
    digest: u64,
}

struct WorkloadInput {
    setup: &'static [u8],
    chunks: Vec<Vec<u8>>,
}

impl WorkloadInput {
    fn cycle_bytes(&self) -> Result<usize, String> {
        self.chunks.iter().try_fold(0usize, |total, chunk| {
            total
                .checked_add(chunk.len())
                .ok_or_else(|| "throughput workload cycle byte count overflowed".to_string())
        })
    }
}

#[derive(Clone, Serialize)]
struct ThroughputSample {
    sample_index: usize,
    order_position: usize,
    bytes: u64,
    elapsed_seconds: f64,
    mib_per_second: f64,
    retained_history_rows: usize,
}

#[derive(Serialize)]
struct ThroughputReport {
    schema_version: u8,
    mode: &'static str,
    parameters: ThroughputParameters,
    calibrations: Vec<WorkloadCalibration>,
    results: Vec<ThroughputEngineResult>,
    ratios: Vec<ThroughputRatioResult>,
}

#[derive(Serialize)]
struct ThroughputParameters {
    samples: usize,
    cols: u16,
    rows: u16,
    scrollback_limit: usize,
    requested_minimum_target_bytes: Option<usize>,
    calibration_pilot_bytes: usize,
    calibration_target_seconds: f64,
    minimum_timed_sample_seconds: f64,
    maximum_target_bytes: usize,
    base_warmup_bytes: usize,
    feed_chunk_bytes: usize,
}

#[derive(Clone, Serialize)]
struct WorkloadCalibration {
    workload: &'static str,
    cycle_bytes: usize,
    pilot_bytes: usize,
    warmup_bytes: usize,
    fastest_engine: &'static str,
    fastest_mib_per_second: f64,
    shared_target_bytes: usize,
    observations: Vec<CalibrationObservation>,
}

#[derive(Clone, Serialize)]
struct CalibrationObservation {
    engine: &'static str,
    bytes: u64,
    elapsed_seconds: f64,
    mib_per_second: f64,
    retained_history_rows: usize,
}

#[derive(Serialize)]
struct ThroughputEngineResult {
    workload: &'static str,
    engine: &'static str,
    target_bytes: usize,
    warmup_bytes: usize,
    samples: Vec<ThroughputSample>,
    median_mib_per_second: f64,
    minimum_mib_per_second: f64,
    maximum_mib_per_second: f64,
    minimum_retained_history_rows: usize,
    maximum_retained_history_rows: usize,
}

#[derive(Serialize)]
struct ThroughputRatioResult {
    workload: &'static str,
    numerator: &'static str,
    denominator: &'static str,
    samples: Vec<f64>,
    median: f64,
    minimum: f64,
    maximum: f64,
}

impl Measurement {
    fn mib_per_second(self) -> f64 {
        self.bytes as f64 / (1024.0 * 1024.0) / self.elapsed_seconds
    }
}

pub(super) fn run() -> Result<(), String> {
    let samples = balanced_sample_setting(
        "TMON_GHOSTTY_THROUGHPUT_SAMPLES",
        DEFAULT_SAMPLES,
        MIN_SAMPLES,
        MAX_SAMPLES,
    )?;
    let requested_minimum_target_bytes = optional_target_bytes()?;
    let base_warmup_bytes = WARMUP_MIB * 1024 * 1024;
    let calibration_pilot_bytes = CALIBRATION_PILOT_MIB * 1024 * 1024;
    let maximum_target_bytes = MAX_TARGET_MIB * 1024 * 1024;
    let workloads = selected_workloads()?;
    let workload_inputs = workloads
        .iter()
        .copied()
        .map(|workload| build_input(workload).map(|input| (workload, input)))
        .collect::<Result<Vec<_>, _>>()?;

    println!("Tmon vs Alacritty vs Ghostty terminal feed throughput");
    println!("======================================================");
    println!("Grid: {COLS}x{ROWS}, scrollback: {SCROLLBACK_LIMIT} lines");
    println!("Samples: {samples}; shared target calibrated independently per workload");
    println!(
        "Calibration: {CALIBRATION_PILOT_MIB} MiB pilot/engine, fastest observed rate x {CALIBRATION_TARGET_SECONDS:.3}s"
    );
    println!("Validity floor: every timed sample must be >= {MINIMUM_TIMED_SAMPLE_SECONDS:.3}s");
    if let Some(target_bytes) = requested_minimum_target_bytes {
        println!(
            "Requested target floor: {:.2} MiB before whole-cycle rounding",
            target_bytes as f64 / (1024.0 * 1024.0)
        );
    }
    println!("Warmup: {WARMUP_MIB} MiB/engine/sample");
    println!("Order: all six three-engine permutations per six samples");
    println!(
        "Feed chunks: 64 KiB scrollback, 4 KiB alternate rows, one operation batch for traces\n"
    );

    println!("Correctness preflight (fresh complete workload, every logical cell)");
    for (workload, input) in &workload_inputs {
        preflight(*workload, input)?;
        println!("  {:<28} matched", workload.name());
    }

    println!("\nIntegrated feed throughput (median MiB/s; higher is better)");
    let mut failures = Vec::new();
    let mut short_samples = Vec::new();
    let mut json_calibrations = Vec::new();
    let mut json_results = Vec::new();
    let mut json_ratios = Vec::new();

    for (workload_index, (workload, input)) in workload_inputs.iter().enumerate() {
        let calibration = calibrate_workload(
            *workload,
            workload_index,
            input,
            base_warmup_bytes,
            calibration_pilot_bytes,
            requested_minimum_target_bytes,
            maximum_target_bytes,
        )?;
        let workload_target = calibration.shared_target_bytes;
        let workload_warmup = calibration.warmup_bytes;
        println!(
            "\nCalibration {}: cycle={} pilot={} fastest={} {:.1} MiB/s shared_target={} ({:.2} MiB, {:.3}s target)",
            workload.name(),
            calibration.cycle_bytes,
            calibration.pilot_bytes,
            calibration.fastest_engine,
            calibration.fastest_mib_per_second,
            calibration.shared_target_bytes,
            calibration.shared_target_bytes as f64 / (1024.0 * 1024.0),
            CALIBRATION_TARGET_SECONDS,
        );
        for observation in &calibration.observations {
            println!(
                "  pilot {:<10} {:>10.1} MiB/s ({:.6}s, {} bytes, retained_history={} rows)",
                observation.engine,
                observation.mib_per_second,
                observation.elapsed_seconds,
                observation.bytes,
                observation.retained_history_rows,
            );
        }
        println!(
            "{CALIBRATION_RECORD_PREFIX} schema=1 workload={} cycle_bytes={} pilot_bytes={} warmup_bytes={} fastest_engine={} fastest_mib_per_second={:.9} calibration_target_seconds={:.3} minimum_sample_seconds={:.3} shared_target_bytes={}",
            workload.name(),
            calibration.cycle_bytes,
            calibration.pilot_bytes,
            calibration.warmup_bytes,
            calibration.fastest_engine,
            calibration.fastest_mib_per_second,
            CALIBRATION_TARGET_SECONDS,
            MINIMUM_TIMED_SAMPLE_SECONDS,
            calibration.shared_target_bytes,
        );
        json_calibrations.push(calibration);
        let mut measurements = BTreeMap::<Engine, Vec<ThroughputSample>>::new();

        for sample_index in 0..samples {
            for (order_position, engine) in
                primary_engine_order(sample_index).into_iter().enumerate()
            {
                let measurement = measure_engine(engine, input, workload_warmup, workload_target)?;
                if measurement.elapsed_seconds < MINIMUM_TIMED_SAMPLE_SECONDS {
                    short_samples.push(format!(
                        "{} {} sample {} lasted {:.6}s below {:.3}s",
                        workload.name(),
                        engine.name(),
                        sample_index,
                        measurement.elapsed_seconds,
                        MINIMUM_TIMED_SAMPLE_SECONDS,
                    ));
                }
                measurements
                    .entry(engine)
                    .or_default()
                    .push(ThroughputSample {
                        sample_index,
                        order_position,
                        bytes: measurement.bytes,
                        elapsed_seconds: measurement.elapsed_seconds,
                        mib_per_second: measurement.mib_per_second(),
                        retained_history_rows: measurement.retained_history_rows,
                    });
            }
        }

        println!(
            "\n{} target={:.2} MiB warmup={:.2} MiB",
            workload.name(),
            workload_target as f64 / (1024.0 * 1024.0),
            workload_warmup as f64 / (1024.0 * 1024.0),
        );
        for engine in [Engine::Tmon, Engine::Alacritty, Engine::Ghostty] {
            let engine_samples = measurements.get(&engine).ok_or_else(|| {
                format!(
                    "missing {} throughput samples for {}",
                    engine.name(),
                    workload.name()
                )
            })?;
            let speeds = engine_samples
                .iter()
                .map(|sample| sample.mib_per_second)
                .collect::<Vec<_>>();
            let engine_median = median(&speeds);
            let engine_minimum = minimum(&speeds);
            let engine_maximum = maximum(&speeds);
            let minimum_retained_history_rows = engine_samples
                .iter()
                .map(|sample| sample.retained_history_rows)
                .min()
                .ok_or_else(|| "throughput sample set was empty".to_string())?;
            let maximum_retained_history_rows = engine_samples
                .iter()
                .map(|sample| sample.retained_history_rows)
                .max()
                .ok_or_else(|| "throughput sample set was empty".to_string())?;
            let retained_history_rows = engine_samples
                .iter()
                .map(|sample| sample.retained_history_rows)
                .collect::<Vec<_>>();
            println!(
                "  {:<10} median={:>10.1} range={:>10.1}..{:>10.1} retained_history={}..{}",
                engine.name(),
                engine_median,
                engine_minimum,
                engine_maximum,
                minimum_retained_history_rows,
                maximum_retained_history_rows,
            );
            println!(
                "{THROUGHPUT_RECORD_PREFIX} schema=1 workload={} engine={} samples={} target_bytes={} warmup_bytes={} median_mib_per_second={:.9} minimum_mib_per_second={:.9} maximum_mib_per_second={:.9} minimum_retained_history_rows={} maximum_retained_history_rows={} raw_mib_per_second={} raw_retained_history_rows={}",
                workload.name(),
                engine.name(),
                engine_samples.len(),
                workload_target,
                workload_warmup,
                engine_median,
                engine_minimum,
                engine_maximum,
                minimum_retained_history_rows,
                maximum_retained_history_rows,
                RawFloats(&speeds),
                RawUsize(&retained_history_rows),
            );
            json_results.push(ThroughputEngineResult {
                workload: workload.name(),
                engine: engine.name(),
                target_bytes: workload_target,
                warmup_bytes: workload_warmup,
                samples: engine_samples.clone(),
                median_mib_per_second: engine_median,
                minimum_mib_per_second: engine_minimum,
                maximum_mib_per_second: engine_maximum,
                minimum_retained_history_rows,
                maximum_retained_history_rows,
            });
        }

        for denominator in [Engine::Alacritty, Engine::Ghostty] {
            let ratios = paired_ratios(&measurements, Engine::Tmon, denominator)?;
            let ratio_median = median(&ratios);
            let ratio_minimum = minimum(&ratios);
            let ratio_maximum = maximum(&ratios);
            println!(
                "  tmon/{:<10} median={:.3}x range={:.3}x..{:.3}x",
                denominator.name(),
                ratio_median,
                ratio_minimum,
                ratio_maximum,
            );
            println!(
                "{RATIO_RECORD_PREFIX} schema=1 workload={} numerator=tmon denominator={} samples={} median={:.9} minimum={:.9} maximum={:.9} raw={}",
                workload.name(),
                denominator.name(),
                ratios.len(),
                ratio_median,
                ratio_minimum,
                ratio_maximum,
                RawFloats(&ratios),
            );
            json_ratios.push(ThroughputRatioResult {
                workload: workload.name(),
                numerator: Engine::Tmon.name(),
                denominator: denominator.name(),
                samples: ratios,
                median: ratio_median,
                minimum: ratio_minimum,
                maximum: ratio_maximum,
            });
            if ratio_median <= 1.0 {
                failures.push((workload, denominator, ratio_median));
            }
        }
    }

    write_json_output(&ThroughputReport {
        schema_version: 1,
        mode: "throughput",
        parameters: ThroughputParameters {
            samples,
            cols: COLS,
            rows: ROWS,
            scrollback_limit: SCROLLBACK_LIMIT,
            requested_minimum_target_bytes,
            calibration_pilot_bytes,
            calibration_target_seconds: CALIBRATION_TARGET_SECONDS,
            minimum_timed_sample_seconds: MINIMUM_TIMED_SAMPLE_SECONDS,
            maximum_target_bytes,
            base_warmup_bytes,
            feed_chunk_bytes: FEED_CHUNK_BYTES,
        },
        calibrations: json_calibrations,
        results: json_results,
        ratios: json_ratios,
    })?;

    if failures.is_empty() {
        println!(
            "TERMINAL_ENGINE_BENCH_SUMMARY schema=1 mode=throughput tmon_strictly_fastest_everywhere=true"
        );
    } else {
        let failures = failures
            .into_iter()
            .map(|(workload, engine, ratio)| {
                format!("{}/{}={ratio:.3}x", workload.name(), engine.name())
            })
            .collect::<Vec<_>>()
            .join(", ");
        println!(
            "TERMINAL_ENGINE_BENCH_SUMMARY schema=1 mode=throughput tmon_strictly_fastest_everywhere=false"
        );
        println!("Tmon was not strictly faster in every throughput comparison: {failures}");
    }
    if short_samples.is_empty() {
        println!(
            "TERMINAL_ENGINE_BENCH_VALIDITY schema=1 mode=throughput minimum_sample_seconds={MINIMUM_TIMED_SAMPLE_SECONDS:.3} status=PASS"
        );
        Ok(())
    } else {
        println!(
            "TERMINAL_ENGINE_BENCH_VALIDITY schema=1 mode=throughput minimum_sample_seconds={MINIMUM_TIMED_SAMPLE_SECONDS:.3} status=FAIL short_samples={}",
            short_samples.len()
        );
        Err(format!(
            "{} timed samples were too short for decision-grade comparison:\n{}",
            short_samples.len(),
            short_samples.join("\n")
        ))
    }
}

fn selected_workloads() -> Result<Vec<Workload>, String> {
    match env::var("TMON_GHOSTTY_THROUGHPUT_WORKLOAD") {
        Ok(name) => {
            let workload = Workload::parse(&name)?;
            if !WORKLOADS.contains(&workload) {
                return Err(format!(
                    "workload {name:?} is not part of the throughput benchmark"
                ));
            }
            Ok(vec![workload])
        }
        Err(env::VarError::NotPresent) => Ok(WORKLOADS.to_vec()),
        Err(error) => Err(format!(
            "could not read TMON_GHOSTTY_THROUGHPUT_WORKLOAD: {error}"
        )),
    }
}

fn optional_target_bytes() -> Result<Option<usize>, String> {
    let value = match env::var("TMON_GHOSTTY_THROUGHPUT_TARGET_MIB") {
        Ok(value) => Some(value.parse::<usize>().map_err(|error| {
            format!("TMON_GHOSTTY_THROUGHPUT_TARGET_MIB must be an integer: {error}")
        })?),
        Err(env::VarError::NotPresent) => None,
        Err(error) => {
            return Err(format!(
                "could not read TMON_GHOSTTY_THROUGHPUT_TARGET_MIB: {error}"
            ));
        }
    };
    value
        .map(|value| {
            if !(MIN_TARGET_MIB..=MAX_TARGET_MIB).contains(&value) {
                return Err(format!(
                    "TMON_GHOSTTY_THROUGHPUT_TARGET_MIB must be between {MIN_TARGET_MIB} and {MAX_TARGET_MIB}, got {value}"
                ));
            }
            value
                .checked_mul(1024 * 1024)
                .ok_or_else(|| "throughput target floor overflowed usize".to_string())
        })
        .transpose()
}

fn preflight(workload: Workload, input: &WorkloadInput) -> Result<(), String> {
    let baseline = semantic_result(Engine::Tmon, input)?;
    for (engine, candidate) in [
        (
            Engine::Alacritty,
            semantic_result(Engine::Alacritty, input)?,
        ),
        (Engine::Ghostty, semantic_result(Engine::Ghostty, input)?),
    ] {
        if baseline.bytes != candidate.bytes
            || baseline.history != candidate.history
            || baseline.cursor_row != candidate.cursor_row
            || baseline.cursor_col != candidate.cursor_col
            || baseline.digest != candidate.digest
        {
            return Err(format!(
                "semantic mismatch for {}: tmon bytes={} history={} cursor={},{} digest={:016x}; {} bytes={} history={} cursor={},{} digest={:016x}",
                workload.name(),
                baseline.bytes,
                baseline.history,
                baseline.cursor_row,
                baseline.cursor_col,
                baseline.digest,
                engine.name(),
                candidate.bytes,
                candidate.history,
                candidate.cursor_row,
                candidate.cursor_col,
                candidate.digest
            ));
        }
    }
    Ok(())
}

fn semantic_result(engine: Engine, input: &WorkloadInput) -> Result<SemanticResult, String> {
    match engine {
        Engine::Tmon => {
            let terminal = new_tmon_terminal();
            let bytes = feed_timed_input_once(input, |chunk| {
                terminal.feed_output(chunk);
                Ok(())
            })?;
            let state = terminal.state_snapshot();
            let cursor = state
                .cursor_state
                .ok_or_else(|| "tmon throughput preflight has no cursor state".to_string())?;
            Ok(SemanticResult {
                bytes,
                history: state.history_size,
                cursor_row: cursor.row,
                cursor_col: cursor.col,
                digest: tmon_semantic_digest(&terminal)?,
            })
        }
        Engine::Alacritty => {
            let mut terminal = DirectAlacritty::new();
            let bytes = feed_timed_input_once(input, |chunk| {
                terminal.feed(chunk);
                Ok(())
            })?;
            let state = terminal.state()?;
            Ok(SemanticResult {
                bytes,
                history: state.history,
                cursor_row: state.cursor_row,
                cursor_col: state.cursor_col,
                digest: terminal.semantic_digest()?,
            })
        }
        Engine::Ghostty => {
            let terminal = GhosttyTerminal::new()?;
            let bytes = feed_timed_input_once(input, |chunk| terminal.feed(chunk))?;
            let state = terminal.query_state()?;
            Ok(SemanticResult {
                bytes,
                history: ghostty_history_rows(&state)?,
                cursor_row: usize::from(state.cursor_y),
                cursor_col: usize::from(state.cursor_x),
                digest: terminal.semantic_digest()?,
            })
        }
        Engine::GhosttyCompressed => {
            Err("ghostty-compressed is memory-only and cannot run throughput".to_string())
        }
    }
}

fn ghostty_history_rows(state: &GhosttyTerminalState) -> Result<usize, String> {
    let history = state
        .total
        .checked_sub(state.len)
        .ok_or_else(|| "ghostty terminal history underflowed".to_string())?;
    usize::try_from(history).map_err(|_| "ghostty terminal history overflowed usize".to_string())
}

fn feed_timed_input_once(
    input: &WorkloadInput,
    mut feed: impl FnMut(&[u8]) -> Result<(), String>,
) -> Result<u64, String> {
    let mut bytes = 0usize;
    if !input.setup.is_empty() {
        feed(input.setup)?;
        bytes = input.setup.len();
    }
    bytes = bytes
        .checked_add(feed_chunks_once(&input.chunks, &mut feed)?)
        .ok_or_else(|| "throughput preflight byte count overflowed".to_string())?;
    u64::try_from(bytes).map_err(|_| "throughput preflight byte count overflowed u64".to_string())
}

fn build_input(workload: Workload) -> Result<WorkloadInput, String> {
    let setup = if workload.enters_alternate_screen() {
        ENTER_ALTERNATE_SCREEN
    } else {
        &[]
    };
    if let Some((trace, _)) = workload.trace() {
        return Ok(WorkloadInput {
            setup,
            chunks: vec![trace.to_vec()],
        });
    }
    let alternate_rows = workload.enters_alternate_screen();
    // A trailing CRLF advances to one more logical row than the memory
    // workload's between-row separators. Keep the exact one-cycle semantic
    // preflight one row below the shared cap; sustained timed feeds report
    // each engine's retained history after crossing it.
    let row_count = if workload.expected_history() > 0 {
        workload.row_count() - 2
    } else {
        workload.row_count()
    };
    let mut stream = Vec::new();
    let mut chunks = Vec::new();
    let mut row = String::new();
    for row_index in 0..row_count {
        row.clear();
        workload.build_row(row_index, &mut row);
        validate_payload_row(&row, workload, row_index)?;
        row.push_str("\r\n");
        if alternate_rows {
            if !stream.is_empty() && stream.len() + row.len() > 4 * 1024 {
                chunks.push(std::mem::take(&mut stream));
            }
            stream.extend_from_slice(row.as_bytes());
        } else {
            stream.extend_from_slice(row.as_bytes());
        }
    }
    if alternate_rows {
        if !stream.is_empty() {
            chunks.push(stream);
        }
    } else {
        chunks = stream
            .chunks(FEED_CHUNK_BYTES)
            .map(<[u8]>::to_vec)
            .collect();
    }
    Ok(WorkloadInput { setup, chunks })
}

fn calibrate_workload(
    workload: Workload,
    workload_index: usize,
    input: &WorkloadInput,
    base_warmup_bytes: usize,
    base_pilot_bytes: usize,
    requested_minimum_target_bytes: Option<usize>,
    maximum_target_bytes: usize,
) -> Result<WorkloadCalibration, String> {
    let cycle_bytes = input.cycle_bytes()?;
    if cycle_bytes == 0 {
        return Err(format!(
            "throughput workload {} has an empty input cycle",
            workload.name()
        ));
    }
    let warmup_bytes = round_up_to_cycle(base_warmup_bytes, cycle_bytes)?;
    let pilot_bytes = round_up_to_cycle(base_pilot_bytes, cycle_bytes)?;
    let mut observations = Vec::with_capacity(3);
    for engine in primary_engine_order(workload_index) {
        let measurement = measure_engine(engine, input, warmup_bytes, pilot_bytes)?;
        let mib_per_second = measurement.mib_per_second();
        if !mib_per_second.is_finite() || mib_per_second <= 0.0 {
            return Err(format!(
                "{} calibration for {} produced invalid rate {mib_per_second:?}",
                engine.name(),
                workload.name()
            ));
        }
        observations.push(CalibrationObservation {
            engine: engine.name(),
            bytes: measurement.bytes,
            elapsed_seconds: measurement.elapsed_seconds,
            mib_per_second,
            retained_history_rows: measurement.retained_history_rows,
        });
    }
    let (fastest_engine, fastest_mib_per_second) = observations
        .iter()
        .max_by(|left, right| left.mib_per_second.total_cmp(&right.mib_per_second))
        .map(|observation| (observation.engine, observation.mib_per_second))
        .ok_or_else(|| "throughput calibration produced no observations".to_string())?;
    let shared_target_bytes = derive_shared_target(
        &observations,
        cycle_bytes,
        requested_minimum_target_bytes,
        maximum_target_bytes,
    )?;
    Ok(WorkloadCalibration {
        workload: workload.name(),
        cycle_bytes,
        pilot_bytes,
        warmup_bytes,
        fastest_engine,
        fastest_mib_per_second,
        shared_target_bytes,
        observations,
    })
}

fn derive_shared_target(
    observations: &[CalibrationObservation],
    cycle_bytes: usize,
    requested_minimum_target_bytes: Option<usize>,
    maximum_target_bytes: usize,
) -> Result<usize, String> {
    let fastest_mib_per_second = observations
        .iter()
        .map(|observation| observation.mib_per_second)
        .max_by(f64::total_cmp)
        .ok_or_else(|| "cannot derive a target without calibration observations".to_string())?;
    if !fastest_mib_per_second.is_finite() || fastest_mib_per_second <= 0.0 {
        return Err(format!(
            "cannot derive a target from invalid fastest rate {fastest_mib_per_second:?}"
        ));
    }
    let calibrated_bytes =
        (fastest_mib_per_second * 1024.0 * 1024.0 * CALIBRATION_TARGET_SECONDS).ceil();
    if calibrated_bytes > usize::MAX as f64 {
        return Err("calibrated throughput target overflowed usize".to_string());
    }
    let target_bytes = (calibrated_bytes as usize)
        .max(requested_minimum_target_bytes.unwrap_or(0))
        .max(cycle_bytes);
    let target_bytes = round_up_to_cycle(target_bytes, cycle_bytes)?;
    if target_bytes > maximum_target_bytes {
        return Err(format!(
            "calibrated throughput target {target_bytes} exceeds safe maximum {maximum_target_bytes} bytes"
        ));
    }
    Ok(target_bytes)
}

fn round_up_to_cycle(bytes: usize, cycle_bytes: usize) -> Result<usize, String> {
    if cycle_bytes == 0 {
        return Err("cannot round a target to a zero-byte workload cycle".to_string());
    }
    bytes
        .div_ceil(cycle_bytes)
        .checked_mul(cycle_bytes)
        .ok_or_else(|| "whole-cycle throughput target overflowed usize".to_string())
}

fn measure_engine(
    engine: Engine,
    input: &WorkloadInput,
    warmup_bytes: usize,
    target_bytes: usize,
) -> Result<Measurement, String> {
    match engine {
        Engine::Tmon => measure_tmon(input, warmup_bytes, target_bytes),
        Engine::Alacritty => measure_alacritty(input, warmup_bytes, target_bytes),
        Engine::Ghostty => measure_ghostty(input, warmup_bytes, target_bytes),
        Engine::GhosttyCompressed => {
            Err("ghostty-compressed is memory-only and cannot run throughput".to_string())
        }
    }
}

fn measure_tmon(
    input: &WorkloadInput,
    warmup_bytes: usize,
    target_bytes: usize,
) -> Result<Measurement, String> {
    let terminal = new_tmon_terminal();
    if !input.setup.is_empty() {
        terminal.feed_output(input.setup);
    }
    feed_exact_cycles(input, warmup_bytes, |chunk| {
        terminal.feed_output(chunk);
        Ok(())
    })?;
    let start = Instant::now();
    let bytes = feed_exact_cycles(input, target_bytes, |chunk| {
        terminal.feed_output(chunk);
        Ok(())
    })?;
    let elapsed_seconds = start.elapsed().as_secs_f64();
    let retained_history_rows = terminal.state_snapshot().history_size;
    black_box(retained_history_rows);
    Ok(Measurement {
        bytes,
        elapsed_seconds,
        retained_history_rows,
    })
}

fn new_tmon_terminal() -> Terminal {
    Terminal::new_display(
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
    )
}

fn measure_alacritty(
    input: &WorkloadInput,
    warmup_bytes: usize,
    target_bytes: usize,
) -> Result<Measurement, String> {
    let mut terminal = DirectAlacritty::new();
    if !input.setup.is_empty() {
        terminal.feed(input.setup);
    }
    feed_exact_cycles(input, warmup_bytes, |chunk| {
        terminal.feed(chunk);
        Ok(())
    })?;
    let start = Instant::now();
    let bytes = feed_exact_cycles(input, target_bytes, |chunk| {
        terminal.feed(chunk);
        Ok(())
    })?;
    let elapsed_seconds = start.elapsed().as_secs_f64();
    let retained_history_rows = terminal.state()?.history;
    black_box(retained_history_rows);
    Ok(Measurement {
        bytes,
        elapsed_seconds,
        retained_history_rows,
    })
}

fn measure_ghostty(
    input: &WorkloadInput,
    warmup_bytes: usize,
    target_bytes: usize,
) -> Result<Measurement, String> {
    let terminal = GhosttyTerminal::new()?;
    if !input.setup.is_empty() {
        terminal.feed(input.setup)?;
    }
    feed_exact_cycles(input, warmup_bytes, |chunk| terminal.feed(chunk))?;
    let start = Instant::now();
    let bytes = feed_exact_cycles(input, target_bytes, |chunk| terminal.feed(chunk))?;
    let elapsed_seconds = start.elapsed().as_secs_f64();
    let state = terminal.query_state()?;
    let retained_history_rows = ghostty_history_rows(&state)?;
    black_box(retained_history_rows);
    Ok(Measurement {
        bytes,
        elapsed_seconds,
        retained_history_rows,
    })
}

fn feed_exact_cycles(
    input: &WorkloadInput,
    target_bytes: usize,
    mut feed: impl FnMut(&[u8]) -> Result<(), String>,
) -> Result<u64, String> {
    let cycle_bytes = input.cycle_bytes()?;
    if cycle_bytes == 0 {
        return Err("throughput workload has no chunks".to_string());
    }
    if !target_bytes.is_multiple_of(cycle_bytes) {
        return Err(format!(
            "throughput target {target_bytes} is not a whole multiple of {cycle_bytes}-byte input cycle"
        ));
    }
    let mut bytes = 0usize;
    for _ in 0..target_bytes / cycle_bytes {
        bytes = bytes
            .checked_add(feed_chunks_once(&input.chunks, &mut feed)?)
            .ok_or_else(|| "throughput byte count overflowed usize".to_string())?;
    }
    u64::try_from(bytes).map_err(|_| "throughput byte count overflowed u64".to_string())
}

fn feed_chunks_once(
    chunks: &[Vec<u8>],
    feed: &mut impl FnMut(&[u8]) -> Result<(), String>,
) -> Result<usize, String> {
    let mut bytes = 0usize;
    for chunk in chunks {
        feed(chunk)?;
        bytes = bytes
            .checked_add(chunk.len())
            .ok_or_else(|| "throughput cycle byte count overflowed usize".to_string())?;
    }
    Ok(bytes)
}

fn paired_ratios(
    measurements: &BTreeMap<Engine, Vec<ThroughputSample>>,
    numerator: Engine,
    denominator: Engine,
) -> Result<Vec<f64>, String> {
    let numerator = measurements
        .get(&numerator)
        .ok_or_else(|| format!("missing {} samples", numerator.name()))?;
    let denominator = measurements
        .get(&denominator)
        .ok_or_else(|| format!("missing {} samples", denominator.name()))?;
    if numerator.len() != denominator.len() {
        return Err(format!(
            "paired sample count mismatch: {} versus {}",
            numerator.len(),
            denominator.len()
        ));
    }
    numerator
        .iter()
        .zip(denominator)
        .map(|(numerator, denominator)| {
            if numerator.sample_index != denominator.sample_index {
                return Err(format!(
                    "paired sample index mismatch: {} versus {}",
                    numerator.sample_index, denominator.sample_index
                ));
            }
            Ok(numerator.mib_per_second / denominator.mib_per_second)
        })
        .collect()
}

fn median(values: &[f64]) -> f64 {
    let mut sorted = values.to_vec();
    sorted.sort_by(f64::total_cmp);
    let middle = sorted.len() / 2;
    if sorted.len().is_multiple_of(2) {
        (sorted[middle - 1] + sorted[middle]) / 2.0
    } else {
        sorted[middle]
    }
}

fn minimum(values: &[f64]) -> f64 {
    values.iter().copied().fold(f64::INFINITY, f64::min)
}

fn maximum(values: &[f64]) -> f64 {
    values.iter().copied().fold(f64::NEG_INFINITY, f64::max)
}

struct RawFloats<'a>(&'a [f64]);

impl fmt::Display for RawFloats<'_> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("[")?;
        for (index, value) in self.0.iter().enumerate() {
            if index != 0 {
                formatter.write_str(",")?;
            }
            write!(formatter, "{value:.9}")?;
        }
        formatter.write_str("]")
    }
}

struct RawUsize<'a>(&'a [usize]);

impl fmt::Display for RawUsize<'_> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("[")?;
        for (index, value) in self.0.iter().enumerate() {
            if index != 0 {
                formatter.write_str(",")?;
            }
            write!(formatter, "{value}")?;
        }
        formatter.write_str("]")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn alternate_input_keeps_setup_outside_row_aligned_chunks() {
        let input = build_input(Workload::AlternatePlain).unwrap();

        assert_eq!(input.setup, ENTER_ALTERNATE_SCREEN);
        assert!(input.chunks.len() > 1);
        assert!(input.chunks.iter().all(|chunk| {
            chunk.len() <= 4 * 1024
                && chunk.ends_with(b"\r\n")
                && !chunk
                    .windows(ENTER_ALTERNATE_SCREEN.len())
                    .any(|window| window == ENTER_ALTERNATE_SCREEN)
        }));
    }

    #[test]
    fn interactive_input_uses_one_operation_batch() {
        let redraw = build_input(Workload::TuiRedraw).unwrap();
        assert_eq!(redraw.setup, ENTER_ALTERNATE_SCREEN);
        assert_eq!(redraw.chunks, [TUI_REDRAW]);

        let edits = build_input(Workload::CursorEdits).unwrap();
        assert!(edits.setup.is_empty());
        assert_eq!(edits.chunks, [CURSOR_EDITS]);
    }

    #[test]
    fn calibration_uses_fastest_rate_for_one_whole_cycle_target() {
        let mib = 1024 * 1024;
        let observations = [
            CalibrationObservation {
                engine: "tmon",
                bytes: 1,
                elapsed_seconds: 1.0,
                mib_per_second: 80.0,
                retained_history_rows: 0,
            },
            CalibrationObservation {
                engine: "alacritty",
                bytes: 1,
                elapsed_seconds: 1.0,
                mib_per_second: 100.0,
                retained_history_rows: 0,
            },
            CalibrationObservation {
                engine: "ghostty",
                bytes: 1,
                elapsed_seconds: 1.0,
                mib_per_second: 60.0,
                retained_history_rows: 0,
            },
        ];
        let cycle_bytes = 3 * mib;
        let target = derive_shared_target(&observations, cycle_bytes, None, 100 * mib).unwrap();
        assert_eq!(target, 51 * mib);
        assert!(target.is_multiple_of(cycle_bytes));

        let forced =
            derive_shared_target(&observations, cycle_bytes, Some(52 * mib), 100 * mib).unwrap();
        assert_eq!(forced, 54 * mib);
        let per_engine_targets = [Engine::Tmon, Engine::Alacritty, Engine::Ghostty].map(|_| forced);
        assert!(per_engine_targets.iter().all(|value| *value == forced));
    }

    #[test]
    fn preflight_replays_exact_timed_setup_and_chunk_boundaries() {
        let scrollback = build_input(Workload::ScrollbackPlain).unwrap();
        let mut calls = Vec::new();
        let bytes = feed_timed_input_once(&scrollback, |chunk| {
            calls.push(chunk.to_vec());
            Ok(())
        })
        .unwrap();
        assert_eq!(calls, scrollback.chunks);
        assert!(
            calls[..calls.len() - 1]
                .iter()
                .all(|chunk| chunk.len() == FEED_CHUNK_BYTES)
        );
        assert!(calls.last().unwrap().ends_with(b"\r\n"));
        assert_eq!(bytes, scrollback.cycle_bytes().unwrap() as u64);
        assert_eq!(
            semantic_result(Engine::Tmon, &scrollback).unwrap().history,
            SCROLLBACK_LIMIT - 1
        );

        let alternate = build_input(Workload::AlternatePlain).unwrap();
        calls.clear();
        let bytes = feed_timed_input_once(&alternate, |chunk| {
            calls.push(chunk.to_vec());
            Ok(())
        })
        .unwrap();
        assert_eq!(calls[0], ENTER_ALTERNATE_SCREEN);
        assert_eq!(calls[1..], alternate.chunks);
        assert!(calls.last().unwrap().ends_with(b"\r\n"));
        assert_eq!(
            bytes,
            (alternate.setup.len() + alternate.cycle_bytes().unwrap()) as u64
        );
    }

    #[test]
    fn exact_timed_preflight_matches_all_engines_for_every_workload() {
        for workload in WORKLOADS {
            let input = build_input(workload).unwrap();
            preflight(workload, &input).unwrap();
        }
    }

    #[test]
    fn exact_cycle_feeder_never_stops_mid_cycle() {
        let input = WorkloadInput {
            setup: &[],
            chunks: vec![vec![1, 2], vec![3, 4, 5]],
        };
        let mut calls = Vec::new();
        let bytes = feed_exact_cycles(&input, 10, |chunk| {
            calls.push(chunk.to_vec());
            Ok(())
        })
        .unwrap();
        assert_eq!(bytes, 10);
        assert_eq!(
            calls,
            [vec![1, 2], vec![3, 4, 5], vec![1, 2], vec![3, 4, 5]]
        );
        assert!(feed_exact_cycles(&input, 9, |_| Ok(())).is_err());
    }

    #[test]
    fn throughput_json_contains_raw_timing_fields() {
        let report = ThroughputReport {
            schema_version: 1,
            mode: "throughput",
            parameters: ThroughputParameters {
                samples: 6,
                cols: COLS,
                rows: ROWS,
                scrollback_limit: SCROLLBACK_LIMIT,
                requested_minimum_target_bytes: None,
                calibration_pilot_bytes: 10,
                calibration_target_seconds: CALIBRATION_TARGET_SECONDS,
                minimum_timed_sample_seconds: MINIMUM_TIMED_SAMPLE_SECONDS,
                maximum_target_bytes: 100,
                base_warmup_bytes: 2,
                feed_chunk_bytes: FEED_CHUNK_BYTES,
            },
            calibrations: vec![WorkloadCalibration {
                workload: "test",
                cycle_bytes: 10,
                pilot_bytes: 10,
                warmup_bytes: 10,
                fastest_engine: "tmon",
                fastest_mib_per_second: 40.0,
                shared_target_bytes: 10,
                observations: vec![CalibrationObservation {
                    engine: "tmon",
                    bytes: 10,
                    elapsed_seconds: 0.25,
                    mib_per_second: 40.0,
                    retained_history_rows: 9,
                }],
            }],
            results: vec![ThroughputEngineResult {
                workload: "test",
                engine: "tmon",
                target_bytes: 10,
                warmup_bytes: 2,
                samples: vec![ThroughputSample {
                    sample_index: 0,
                    order_position: 1,
                    bytes: 10,
                    elapsed_seconds: 0.25,
                    mib_per_second: 40.0,
                    retained_history_rows: 9,
                }],
                median_mib_per_second: 40.0,
                minimum_mib_per_second: 40.0,
                maximum_mib_per_second: 40.0,
                minimum_retained_history_rows: 9,
                maximum_retained_history_rows: 9,
            }],
            ratios: Vec::new(),
        };

        let json = serde_json::to_value(report).unwrap();
        assert_eq!(json["results"][0]["samples"][0]["bytes"], 10);
        assert_eq!(json["results"][0]["samples"][0]["elapsed_seconds"], 0.25);
        assert_eq!(json["results"][0]["samples"][0]["order_position"], 1);
        assert_eq!(json["calibrations"][0]["shared_target_bytes"], 10);
        assert_eq!(
            json["calibrations"][0]["observations"][0]["retained_history_rows"],
            9
        );
        assert_eq!(json["results"][0]["samples"][0]["retained_history_rows"], 9);
        assert_eq!(
            json["parameters"]["minimum_timed_sample_seconds"],
            MINIMUM_TIMED_SAMPLE_SECONDS
        );
    }
}
