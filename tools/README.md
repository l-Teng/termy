# Tools

Standalone development and benchmark packages that are intentionally outside
the main Cargo workspace.

- [`tmon-revision-gate/`](tmon-revision-gate/README.md): compares current Tmon
  snapshot throughput with a pinned immutable revision.
- [`tmon-ghostty-memory/`](tmon-ghostty-memory/README.md): compares Tmon and a
  pinned `libghostty-vt` build for memory use and feed throughput.

Use the root `justfile` recipes where available:

```sh
just benchmark-tmon
GHOSTTY_DIR=/path/to/ghostty just benchmark-tmon-ghostty-memory
```
