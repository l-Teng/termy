# xtask

Repository automation binary.

## Owner

This crate owns maintainer commands that generate or verify repository artifacts, such as generated keybinding and configuration documentation.

Keep product runtime code out of this crate. If an automation command needs shared domain data, depend on the smallest domain crate that owns that data.

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
