---
name: build-termy-plugins
description: Build, debug, review, optimize, and validate Termy v1 plugins written in TypeScript or TSX for Bun. Use when creating or changing plugin.json, plugin.ts, plugin.tsx, command-palette commands, native inputs, typed settings, lifecycle events, plugin storage, keybindings, returned actions, or allowlisted GPUI-backed native JSX views; also use for Termy plugin installation, development mode, API limits, security, or performance work.
---

# Build Termy Plugins

Build small, native-feeling Termy extensions against the real v1 contract. Keep the
plugin scoped, use the fewest capabilities, and verify behavior inside Termy when
the user authorizes installation or development mode.

## Start from evidence

1. Inspect repository instructions, the dirty state, and existing plugin files.
   Preserve unrelated work.
2. Read the plugin's managed `termy.d.ts` when available. Treat it and the current
   Termy checkout as authoritative when they differ from this skill.
3. Read [references/api-reference.md](references/api-reference.md) before adding a
   new API surface or diagnosing a type/manifest error.
4. Read [references/performance-security.md](references/performance-security.md)
   before handling user-controlled shell text, files, credentials, network,
   subprocesses, lifecycle events, storage, or native UI.
5. Read [references/examples.md](references/examples.md) before scaffolding. Reuse
   `assets/command-plugin/` or `assets/native-ui-plugin/` when it fits.

Do not invent React, HTML, CSS, GPUI access, package imports, SDK imports, build
hooks, or APIs that are absent from the v1 declarations.

## Choose the shape

| Need | Entrypoint | Capability |
| --- | --- | --- |
| Palette command or lifecycle event | `plugin.ts` | none |
| `context.storage` or managed data/cache paths | `plugin.ts` or `.tsx` | `storage` |
| Native Termy JSX view or `view.open` | `plugin.tsx` | `native-ui` |
| Persistent native view | `plugin.tsx` | `storage`, `native-ui` |

Capabilities gate Termy-owned APIs only. They do not restrict Bun's operating-system
access.

## Build the plugin

### 1. Scaffold

Prefer Termy's scaffold when its CLI is available:

```sh
termy plugin init my-plugin
```

Otherwise create only `plugin.json` and `plugin.ts` or `plugin.tsx`. Add local
relative modules only when they improve the design. Do not add `package.json`,
dependencies, or a bundler for a v1 plugin.

### 2. Write the manifest

Use the public schema, API version 1, a stable lowercase ID, a human-readable name,
an optional display version, an optional relative `main`, and only required
capabilities:

```json
{
  "$schema": "https://termy.sh/schemas/plugin.schema.json",
  "apiVersion": 1,
  "id": "git-tools",
  "name": "Git Tools",
  "version": "1.0.0",
  "capabilities": []
}
```

Keep `main` inside the plugin directory. Never use absolute paths, `..`, or symlinks.

### 3. Implement against ambient types

Export one definition and let Termy's managed declarations provide the API:

```ts
export default definePlugin({
  commands: [],
} satisfies TermyPlugin);
```

Do not import `definePlugin`, `TermyPlugin`, `TermyPluginContext`, `TermyUI`, or an
SDK. Termy supplies them globally.

Apply these rules:

- Give commands stable IDs and clear, searchable titles.
- Treat context as a read-only point-in-time snapshot. Check optional selection,
  directory, command, tab, pane, and event fields before use.
- Use typed settings for configuration. Use `secret` settings for credentials.
- Map fixed choices to fixed `terminal.run` commands. Never concatenate untrusted
  free-form input into a shell command.
- Return typed actions or emit toasts; do not reach into Termy internals.
- Use `commands: []` for event-only plugins. Keep event handlers bounded.
- Use async storage for small JSON and managed paths for larger files.
- Use `.tsx`, the three Termy JSX pragmas, allowlisted `TermyUI` components, unique
  control IDs, named actions, and `onAction` for interactive native views.
- Paginate dynamic native-UI lists and rerender from persisted state.

## Develop and verify

Do not mutate the user's installed plugins merely to inspect code. When the user
only requested source work, perform a non-installing syntax/import check first:

```sh
termy_plugin_check_dir="$(mktemp -d)"
bun build ./my-plugin/plugin.ts --target=bun --outdir "$termy_plugin_check_dir"
```

Use the `.tsx` entrypoint when applicable. This mirrors Termy's Bun target but does
not prove ambient type compatibility or Termy runtime behavior.

When the user asked to build/run the plugin and Termy is installed, use:

```sh
termy plugin dev ./my-plugin
```

This validates the source tree, atomically syncs valid changes, and watches the
development folder. Open the command palette after saving to refresh the current
plugin. Stop the watcher with Ctrl-C; this does not uninstall the managed copy.

Verify the relevant surfaces:

1. Confirm the manifest validates and the Worker loads without a Bun error.
2. Exercise every command, input branch, disabled state, action, and toast.
3. Test missing optional context and both native/tmux behavior when relevant.
4. Confirm settings update on the next invocation and secrets stay out of plain
   plugin JSON.
5. Confirm lifecycle events are idempotent and do not create duplicate work.
6. Confirm native controls emit the intended named action, persist correctly, and
   rerender within document limits.
7. Save a source/import change, reopen the palette, and verify hot refresh.
8. Review the diff and run the repository's nearest checks plus `git diff --check`.

If Termy runtime verification is unavailable, report that gap plainly. Do not call
static TypeScript success proof of palette, Worker, tmux, storage, or native GPUI
behavior.

## Review checklist

- Keep capabilities minimal and manifest/source IDs consistent.
- Keep local imports inside the plugin root; reject dependencies and symlinks.
- Bound selection, input, storage, output, list, network, and subprocess work.
- Clean up child processes explicitly when cancellation matters.
- Avoid repeated filesystem/network work in render and lifecycle hot paths.
- Use cached/persisted data and small documents; never render an unbounded list.
- Preserve the user's source folder when installing, updating, disabling, or
  uninstalling managed copies.

## Hand off

State the files changed, declared capabilities, important security/performance
decisions, commands run, runtime behavior observed, and any unverified Termy surface.
