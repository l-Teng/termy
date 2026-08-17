# xtask

Repository automation binary.

## Owner

This crate owns maintainer commands that generate or verify repository
artifacts, including generated documentation and cross-engine benchmark
reports. Engine-only competitor benchmarks live under `tools/`, outside this
workspace crate.

Keep product runtime code out of this crate. If an automation command needs
shared domain data, depend on the smallest domain crate that owns that data.

## Validation

```sh
cargo test -p xtask
cargo run -p xtask -- generate-keybindings-doc --check
cargo run -p xtask -- generate-config-doc --check
cargo run -p xtask -- check-dependency-policy
```

## Forbidden Dependencies

- `termy_ffi`
- `termy` / `crates/desktop_app`
- product runtime workflows
