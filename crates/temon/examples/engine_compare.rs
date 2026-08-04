use std::{env, hint::black_box, mem::size_of, time::Instant};

use termy_core::{
    Terminal as AlacrittyTerminal, TerminalRuntimeConfig, TerminalSize as AlacrittySize,
};
use tmon::{Config as TmonConfig, Size as TmonSize, Terminal as TmonTerminal};

const MIB: usize = 1024 * 1024;
const COLS: u16 = 120;
const ROWS: u16 = 40;
const SCROLLBACK: usize = 10_000;
const SNAPSHOT_PREFILL_BYTES: usize = 2 * MIB;

struct Workload {
    name: &'static str,
    payload: &'static str,
}

const WORKLOADS: [Workload; 4] = [
    Workload {
        name: "plain scrollback",
        payload: "the quick brown fox jumps over the lazy dog 0123456789\r\n",
    },
    Workload {
        name: "styled TUI redraw",
        payload: concat!(
            "\x1b[H\x1b[38;2;120;180;255;48;5;234;1mstatus\x1b[0m",
            "\x1b[2;1H\x1b[Kbuild complete\x1b[3;1Hprogress: 100%",
        ),
    },
    Workload {
        name: "Unicode + wide cells",
        payload: "ASCII café Ελληνικά 日本語 한글 🙂界\r\n",
    },
    Workload {
        name: "cursor + line edits",
        payload: "\x1b[10;20Habcdef\x1b[3DXYZ\x1b[K\x1b[4;1H\x1b[2Linserted\x1b[1M\x1b[2J\x1b[H",
    },
];

#[derive(Clone, Copy)]
enum Engine {
    Tmon,
    Alacritty,
}

#[derive(Clone, Copy)]
struct Stats {
    median: f64,
    min: f64,
    max: f64,
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

fn collect_parse_samples(
    workload: &Workload,
    target_bytes: usize,
    sample_count: usize,
) -> (Stats, Stats) {
    let warmup_bytes = target_bytes.min(2 * MIB);
    black_box(parse_sample(Engine::Tmon, workload, warmup_bytes));
    black_box(parse_sample(Engine::Alacritty, workload, warmup_bytes));

    let mut tmon = Vec::with_capacity(sample_count);
    let mut alacritty = Vec::with_capacity(sample_count);
    for index in 0..sample_count {
        if index.is_multiple_of(2) {
            tmon.push(parse_sample(Engine::Tmon, workload, target_bytes));
            alacritty.push(parse_sample(Engine::Alacritty, workload, target_bytes));
        } else {
            alacritty.push(parse_sample(Engine::Alacritty, workload, target_bytes));
            tmon.push(parse_sample(Engine::Tmon, workload, target_bytes));
        }
    }
    (stats(&tmon), stats(&alacritty))
}

fn collect_snapshot_samples(snapshot_count: usize, sample_count: usize) -> (Stats, Stats) {
    black_box(snapshot_sample(Engine::Tmon, 10));
    black_box(snapshot_sample(Engine::Alacritty, 10));

    let mut tmon = Vec::with_capacity(sample_count);
    let mut alacritty = Vec::with_capacity(sample_count);
    for index in 0..sample_count {
        if index.is_multiple_of(2) {
            tmon.push(snapshot_sample(Engine::Tmon, snapshot_count));
            alacritty.push(snapshot_sample(Engine::Alacritty, snapshot_count));
        } else {
            alacritty.push(snapshot_sample(Engine::Alacritty, snapshot_count));
            tmon.push(snapshot_sample(Engine::Tmon, snapshot_count));
        }
    }
    (stats(&tmon), stats(&alacritty))
}

fn main() {
    let target_mib = setting("TMON_BENCH_TARGET_MIB", 32, 1, 1024);
    let sample_count = setting("TMON_BENCH_SAMPLES", 7, 3, 25);
    let snapshot_count = setting("TMON_BENCH_SNAPSHOTS", 250, 10, 10_000);
    let target_bytes = target_mib * MIB;

    println!("Tmon vs Alacritty terminal engine benchmark");
    println!("============================================");
    println!("Platform: {}/{}", env::consts::OS, env::consts::ARCH);
    println!("Grid: {COLS}x{ROWS}, scrollback: {SCROLLBACK} lines");
    println!("Samples: {sample_count}, parse target: {target_mib} MiB/sample");
    println!("Full snapshots: {snapshot_count}/sample");
    println!();
    println!("Static grid cell size (excludes optional heap allocations):");
    println!("  Tmon:       {} bytes", size_of::<tmon::Cell>());
    println!(
        "  Alacritty:  {} bytes",
        size_of::<alacritty_terminal::term::cell::Cell>()
    );
    println!();
    println!("Parser + grid throughput (median MiB/s; higher is better)");
    println!(
        "{:<24} {:>12} {:>12} {:>12}",
        "Workload", "Tmon", "Alacritty", "Tmon/Alac"
    );
    println!("{}", "-".repeat(63));

    let mut ratios = Vec::with_capacity(WORKLOADS.len());
    for workload in &WORKLOADS {
        let (tmon, alacritty) = collect_parse_samples(workload, target_bytes, sample_count);
        let ratio = tmon.median / alacritty.median;
        ratios.push(ratio);
        println!(
            "{:<24} {:>12.1} {:>12.1} {:>11.2}x",
            workload.name, tmon.median, alacritty.median, ratio
        );
        println!(
            "  range                  {:>7.1}-{:<7.1} {:>7.1}-{:<7.1}",
            tmon.min, tmon.max, alacritty.min, alacritty.max
        );
    }

    let parse_geomean =
        (ratios.iter().map(|ratio| ratio.ln()).sum::<f64>() / ratios.len() as f64).exp();
    let (tmon_snapshot, alacritty_snapshot) =
        collect_snapshot_samples(snapshot_count, sample_count);
    let snapshot_ratio = tmon_snapshot.median / alacritty_snapshot.median;

    println!();
    println!("Full visible-frame snapshot throughput (median snapshots/s)");
    println!("  Tmon:       {:>12.1}", tmon_snapshot.median);
    println!("  Alacritty:  {:>12.1}", alacritty_snapshot.median);
    println!("  Tmon/Alac:  {:>11.2}x", snapshot_ratio);
    println!();
    println!("Summary");
    println!("  Parse throughput geometric mean: {parse_geomean:.2}x");
    println!("  Full snapshot throughput:        {snapshot_ratio:.2}x");
    println!();
    println!("Notes");
    println!("  - Ratios above 1.00x favor Tmon; below 1.00x favor Alacritty.");
    println!("  - Alacritty is measured through Termy's termy_core wrapper.");
    println!("  - This excludes PTY I/O, GPUI rendering, input latency, RSS, and feature parity.");
    println!("  - Compare trends across runs; shared GitHub runners have timing noise.");
}
