use std::{hint::black_box, time::Instant};

use super::*;

const DEFAULT_SAMPLES: usize = 8;
const DEFAULT_TARGET_MIB: usize = 32;
const WARMUP_MIB: usize = 2;
const MIN_SAMPLES: usize = 4;
const MAX_SAMPLES: usize = 24;
const MIN_TARGET_MIB: usize = 1;
const MAX_TARGET_MIB: usize = 512;
const FEED_CHUNK_BYTES: usize = 64 * 1024;

const WORKLOADS: [Workload; 4] = [
    Workload::ScrollbackPlain,
    Workload::ScrollbackStyled,
    Workload::ScrollbackUnicode,
    Workload::ScrollbackMixed,
];

#[derive(Clone, Copy)]
struct Measurement {
    bytes: u64,
    elapsed_seconds: f64,
}

impl Measurement {
    fn mib_per_second(self) -> f64 {
        self.bytes as f64 / (1024.0 * 1024.0) / self.elapsed_seconds
    }
}

pub(super) fn run() -> Result<(), String> {
    let samples = even_setting(
        "TMON_GHOSTTY_THROUGHPUT_SAMPLES",
        DEFAULT_SAMPLES,
        MIN_SAMPLES,
        MAX_SAMPLES,
    )?;
    let target_mib = setting(
        "TMON_GHOSTTY_THROUGHPUT_TARGET_MIB",
        DEFAULT_TARGET_MIB,
        MIN_TARGET_MIB,
        MAX_TARGET_MIB,
    )?;
    let target_bytes = target_mib
        .checked_mul(1024 * 1024)
        .ok_or_else(|| "throughput target overflowed usize".to_string())?;
    let warmup_bytes = WARMUP_MIB * 1024 * 1024;

    println!("Tmon vs Ghostty terminal feed throughput");
    println!("=========================================");
    println!("Grid: {COLS}x{ROWS}, scrollback: {SCROLLBACK_LIMIT} lines");
    println!("Samples: {samples}, target: {target_mib} MiB/engine/sample");
    println!("Warmup: {WARMUP_MIB} MiB/engine/sample\n");
    println!("Feed chunk: {} KiB\n", FEED_CHUNK_BYTES / 1024);

    println!("Correctness preflight (fresh complete workload)");
    for workload in WORKLOADS {
        preflight(workload)?;
        println!("  {:<28} matched", workload.name());
    }

    println!("\nIntegrated feed throughput (median MiB/s; higher is better)");
    println!(
        "{:<28} {:>12} {:>12} {:>13}",
        "Workload", "Tmon", "Ghostty", "Tmon/Ghostty"
    );
    println!("{}", "-".repeat(70));

    let mut failed = Vec::new();
    for workload in WORKLOADS {
        let chunks = build_chunks(workload)?;
        let mut tmon = Vec::with_capacity(samples);
        let mut ghostty = Vec::with_capacity(samples);
        let mut ratios = Vec::with_capacity(samples);

        for sample in 0..samples {
            let (tmon_measurement, ghostty_measurement) = if sample % 2 == 0 {
                (
                    measure_tmon(&chunks, warmup_bytes, target_bytes)?,
                    measure_ghostty(&chunks, warmup_bytes, target_bytes)?,
                )
            } else {
                let ghostty = measure_ghostty(&chunks, warmup_bytes, target_bytes)?;
                let tmon = measure_tmon(&chunks, warmup_bytes, target_bytes)?;
                (tmon, ghostty)
            };
            let tmon_speed = tmon_measurement.mib_per_second();
            let ghostty_speed = ghostty_measurement.mib_per_second();
            tmon.push(tmon_speed);
            ghostty.push(ghostty_speed);
            ratios.push(tmon_speed / ghostty_speed);
        }

        let tmon_median = median(&tmon);
        let ghostty_median = median(&ghostty);
        let ratio_median = median(&ratios);
        println!(
            "{:<28} {:>12.1} {:>12.1} {:>12.3}x",
            workload.name(),
            tmon_median,
            ghostty_median,
            ratio_median
        );
        println!(
            "  ranges                  {:>7.1}-{:<7.1} {:>7.1}-{:<7.1} {:>7.3}-{:<7.3}x",
            minimum(&tmon),
            maximum(&tmon),
            minimum(&ghostty),
            maximum(&ghostty),
            minimum(&ratios),
            maximum(&ratios)
        );
        if ratio_median <= 1.0 {
            failed.push((workload, ratio_median));
        }
    }

    if failed.is_empty() {
        println!("\nResult: PASS (Tmon median paired ratio exceeds 1.000x everywhere)");
        Ok(())
    } else {
        let failures = failed
            .into_iter()
            .map(|(workload, ratio)| format!("{}={ratio:.3}x", workload.name()))
            .collect::<Vec<_>>()
            .join(", ");
        Err(format!(
            "Tmon did not beat Ghostty for every workload: {failures}"
        ))
    }
}

fn preflight(workload: Workload) -> Result<(), String> {
    let tmon = run_tmon_child(workload)?;
    let ghostty = run_ghostty_child(Engine::Ghostty, workload)?;
    if tmon.bytes != ghostty.bytes
        || tmon.history != ghostty.history
        || tmon.cursor_row != ghostty.cursor_row
        || tmon.cursor_col != ghostty.cursor_col
        || tmon.digest != ghostty.digest
    {
        return Err(format!(
            "semantic mismatch for {}: tmon bytes={} history={} cursor={},{} digest={:016x}; ghostty bytes={} history={} cursor={},{} digest={:016x}",
            workload.name(),
            tmon.bytes,
            tmon.history,
            tmon.cursor_row,
            tmon.cursor_col,
            tmon.digest,
            ghostty.bytes,
            ghostty.history,
            ghostty.cursor_row,
            ghostty.cursor_col,
            ghostty.digest
        ));
    }
    Ok(())
}

fn build_chunks(workload: Workload) -> Result<Vec<Vec<u8>>, String> {
    let mut stream = Vec::new();
    let mut row = String::new();
    for row_index in 0..workload.row_count() {
        row.clear();
        workload.build_row(row_index, &mut row);
        validate_payload_row(&row, workload, row_index)?;
        row.push_str("\r\n");
        stream.extend_from_slice(row.as_bytes());
    }
    Ok(stream
        .chunks(FEED_CHUNK_BYTES)
        .map(<[u8]>::to_vec)
        .collect())
}

fn measure_tmon(
    chunks: &[Vec<u8>],
    warmup_bytes: usize,
    target_bytes: usize,
) -> Result<Measurement, String> {
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
    feed_until(chunks, warmup_bytes, |chunk| {
        terminal.feed_output(chunk);
        Ok(())
    })?;
    let start = Instant::now();
    let bytes = feed_until(chunks, target_bytes, |chunk| {
        terminal.feed_output(chunk);
        Ok(())
    })?;
    let elapsed_seconds = start.elapsed().as_secs_f64();
    black_box(terminal.state_snapshot().history_size);
    Ok(Measurement {
        bytes,
        elapsed_seconds,
    })
}

fn measure_ghostty(
    chunks: &[Vec<u8>],
    warmup_bytes: usize,
    target_bytes: usize,
) -> Result<Measurement, String> {
    let terminal = GhosttyTerminal::new()?;
    feed_until(chunks, warmup_bytes, |chunk| terminal.feed(chunk))?;
    let start = Instant::now();
    let bytes = feed_until(chunks, target_bytes, |chunk| terminal.feed(chunk))?;
    let elapsed_seconds = start.elapsed().as_secs_f64();
    black_box(terminal.query_state()?);
    Ok(Measurement {
        bytes,
        elapsed_seconds,
    })
}

fn feed_until(
    chunks: &[Vec<u8>],
    target_bytes: usize,
    mut feed: impl FnMut(&[u8]) -> Result<(), String>,
) -> Result<u64, String> {
    if chunks.is_empty() {
        return Err("throughput workload has no chunks".to_string());
    }
    let mut bytes = 0usize;
    while bytes < target_bytes {
        for chunk in chunks {
            feed(chunk)?;
            bytes = bytes
                .checked_add(chunk.len())
                .ok_or_else(|| "throughput byte count overflowed usize".to_string())?;
            if bytes >= target_bytes {
                break;
            }
        }
    }
    u64::try_from(bytes).map_err(|_| "throughput byte count overflowed u64".to_string())
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

fn even_setting(name: &str, default: usize, min: usize, max: usize) -> Result<usize, String> {
    let value = setting(name, default, min, max)?;
    if value % 2 != 0 {
        return Err(format!("{name} must be even, got {value}"));
    }
    Ok(value)
}

fn median(values: &[f64]) -> f64 {
    let mut sorted = values.to_vec();
    sorted.sort_by(f64::total_cmp);
    let middle = sorted.len() / 2;
    (sorted[middle - 1] + sorted[middle]) / 2.0
}

fn minimum(values: &[f64]) -> f64 {
    values.iter().copied().fold(f64::INFINITY, f64::min)
}

fn maximum(values: &[f64]) -> f64 {
    values.iter().copied().fold(f64::NEG_INFINITY, f64::max)
}
