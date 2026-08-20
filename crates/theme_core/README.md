# termy_theme_core

Shared theme data model.

## Owner

This crate owns theme structs, serialization-compatible color types, registry data contracts, and the versioned `theme_registry.cache` representation used by config, bundled themes, docs, and embedders.

Keep bundled theme values in `termy_themes`. Filesystem paths, network fetching, and app-specific cache policy stay with their callers.

## Validation

```sh
cargo test -p termy_theme_core
```

## Forbidden Dependencies

- `gpui`
- `termy_themes`
- `termy_config_core`
- `termy` / `crates/desktop_app`
