# Tmon immutable revision gate

This standalone Cargo package compares the current `crates/tmon` native
`Terminal::snapshot()` throughput with the immutable Tmon revision
`03d7ca5c5420ca141afe56f725341faadb71af18` in one process.

The gate has no runtime tuning knobs. It always uses:

- a 120x40 display with 10,000 lines of scrollback;
- an exact 2 MiB plain-ASCII prefill followed by the same mixed visible fixture;
- a semantic preflight that checks each actual snapshot against independently
  visited terminal state, then compares both revisions;
- 20 balanced four-run blocks, alternating `C/R/R/C` and `R/C/C/R`;
- at least 250 ms for every retained engine measurement;
- a `0.95x` current/reference throughput threshold.

Run the benchmark as an optimized, locked build from this directory:

```sh
cargo run --release --locked
```

Do not pass arguments. Environment variables cannot lower the workload or
threshold. Exit status is `0` for `PASS`, `1` for `REGRESSION`, `2` for
`INCONCLUSIVE`, and `3` for setup/preflight failure (`64` for invalid usage).

For validation that does not execute the benchmark:

```sh
cargo test --locked
cargo check --locked
```
