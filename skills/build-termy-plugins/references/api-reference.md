# Termy plugin v1 API reference

Snapshot: official Termy repository commit
`e04bb5ce49177f054f3e1bd63bb308130cf6ffe9`, reviewed 2026-07-25.
Prefer the plugin's current managed `termy.d.ts` and current official documentation
when they differ.

## Contents

- Manifest
- Plugin definition
- Commands and inputs
- Context
- Settings and storage
- Actions
- Lifecycle events
- Native UI
- CLI and managed locations
- Official sources

## Manifest

Required fields:

| Field | Contract |
| --- | --- |
| `apiVersion` | Integer constant `1` |
| `id` | 1–64 chars; `^[a-z0-9][a-z0-9._-]{0,63}$` |
| `name` | Nonblank string, max 200 chars |

Optional fields:

| Field | Contract |
| --- | --- |
| `$schema` | `https://termy.sh/schemas/plugin.schema.json` |
| `version` | Nonblank display metadata, max 100 chars |
| `main` | Relative `.ts` or `.tsx`; default `plugin.ts`; max 1024 chars |
| `capabilities` | Unique subset of `storage`, `native-ui`; max 2 |

The manifest rejects unknown properties. The entrypoint must remain inside the
plugin root and be a regular non-symlink file.

## Plugin definition

```ts
type TermyPlugin<TSettings> = {
  settings?: TSettings;
  commands: TermyPluginCommand<TSettings>[];
  events?: TermyPluginEvents<TSettings>;
  views?: Record<string, TermyPluginView<TSettings>>;
};
```

Export with:

```ts
export default definePlugin({
  commands: [],
} satisfies TermyPlugin);
```

Termy writes ambient declarations to managed `plugins/termy.d.ts`. Do not import an
SDK or repeat manifest identity inside the TypeScript definition.

## Commands and inputs

A command requires `id`, `title`, and `run`. Optional fields are `keywords`,
`status`, `enabled`, `disabledReason`, `icon`, `inputs`, and `timeoutMs`.

Icons:

`command`, `play`, `terminal`, `folder`, `link`, `clipboard`, `settings`, `info`.

Inputs:

| Type | Required | Optional |
| --- | --- | --- |
| `text` | `id`, `type`, `label` | `placeholder`, `defaultValue`, `required`, `maxLength` |
| `select` | `id`, `type`, `label`, `options` | `placeholder`, `defaultValue`, `required` |
| `confirm` | `id`, `type`, `label` | `defaultValue` |

Select options require `value` and `label`; they may add `keywords` and `status`.
Handler values are available under `inputs.<id>` as `string | boolean`.

## Context

Always present:

- `platform`: `macos | linux | windows`
- `appVersion`: string
- `shell`: resolved session launch shell
- `runtime`: `native | tmux`
- `selectedTextTruncated`: boolean
- `settings`, `toasts`

Optional:

- `workingDirectory`
- `activeCommand`
- `selectedText`
- `activeTab`: `{ index, title, paneCount }`
- `activePane`: `{ index, kind: "terminal" }`

Indexes are zero-based. Selection is capped at 64 KiB at a UTF-8 boundary. Use
`selectedTextTruncated` when completeness matters.

Toast methods are `info`, `success`, `warning`, and `error`.

## Settings and storage

Settings live beside `commands`:

| Type | Value | Main fields |
| --- | --- | --- |
| `toggle` | boolean | `title`, optional `description`, `defaultValue` |
| `text` | string | `title`, optional `description`, `placeholder`, `defaultValue`, `maxLength` |
| `select` | string | `title`, `options`, optional `description`, `defaultValue` |
| `secret` | string | `title`, optional `description`, `placeholder`, `maxLength` |

Read values with `context.settings.get("key")`. Secret values use the operating
system credential store. Ordinary overrides use the plugin data directory.

With `storage` capability:

```ts
await context.storage.get<T>("key");
await context.storage.set("key", jsonValue);
await context.storage.delete("key");
await context.storage.clear();
```

`context.paths.dataDirectory` is persistent; `cacheDirectory` is disposable. Both
are plugin-specific and survive reload/update.

## Actions

Handlers may return nothing, one action, an action array, or `{ actions: [...] }`.

| Action | Shape |
| --- | --- |
| Run shell | `{ type: "terminal.run", command, workingDirectory? }` |
| Invoke Termy | `{ type: "termy.command", command }` |
| Copy | `{ type: "clipboard.write", text }` |
| Open URL | `{ type: "url.open", url }` with `http` or `https` |
| Open view | `{ type: "view.open", view }`; requires `native-ui` |
| Toast | `{ type: "toast", level, message }` |

## Lifecycle events

Supported event keys:

- `terminal.ready`: runs once after the plugin catalog is ready.
- `tab.activated`: optional `previousTabIndex`.
- `workingDirectory.changed`: optional previous/current directories.
- `command.finished`: optional `command`, `exitCode`, and `durationMs`.

All handlers receive the same context snapshot as commands. Native shell integration
provides command exit/duration data; tmux completion is inferred, so missing fields
remain omitted. Events remain ordered inside one plugin; separate plugins may run
concurrently.

## Native UI

Use a `.tsx` entrypoint and:

```tsx
/** @jsxRuntime classic */
/** @jsx TermyUI.createElement */
/** @jsxFrag TermyUI.Fragment */
```

Supported components:

| Component | Props |
| --- | --- |
| `Column`, `Row` | `gap`, `align`, children |
| `Text` | `variant`, `tone`, text children |
| `TextInput` | `id`, `label`, `placeholder`, `value`, `maxLength`, `submit`, `disabled` |
| `Button` | `id`, `action`, `payload`, `variant`, `disabled`, text children |
| `Checkbox` | `id`, `action`, `payload`, `checked`, `disabled`, text children |
| `Divider` | none |
| `Spacer` | `size` |

Semantic values:

- gap/size: `none`, `small`, `medium`, `large`
- align: `start`, `center`, `end`, `stretch`
- text variant: `heading`, `body`, `caption`, `code`
- tone: `default`, `muted`, `success`, `danger`
- button variant: `secondary`, `primary`, `danger`

Interactive controls use named strings, not callbacks. `Button` and `Checkbox`
require `id` and `action`; `TextInput` requires `id` and may submit a named action.
All control IDs in one document must be unique.

`onAction` receives:

```ts
{
  action: { id, controlId, payload?, value? },
  values: Readonly<Record<string, string | boolean>>,
  context
}
```

Termy reruns `render` after `onAction` completes.

## CLI and managed locations

```sh
termy plugin init my-plugin
termy plugin dev ./my-plugin
termy plugin add ./my-plugin
termy plugin add https://github.com/example/repo --path my-plugin
termy plugin status my-plugin
termy plugin disable my-plugin
termy plugin enable my-plugin
termy plugin update my-plugin
termy plugin uninstall my-plugin
```

`add` aliases `install`; `remove` aliases `uninstall`.

Managed plugin directory:

- `$XDG_CONFIG_HOME/termy/plugins` when set
- `~/.config/termy/plugins` otherwise on macOS/Linux
- `%APPDATA%\termy\plugins` on Windows

## Official sources

- [Plugin overview](https://termy.sh/docs/using-termy/plugins)
- [Getting started](https://termy.sh/docs/using-termy/plugins/getting-started)
- [Commands and context](https://termy.sh/docs/using-termy/plugins/commands-context)
- [Native UI](https://termy.sh/docs/using-termy/plugins/native-ui)
- [Lifecycle and storage](https://termy.sh/docs/using-termy/plugins/lifecycle-storage)
- [Security and limits](https://termy.sh/docs/using-termy/plugins/security-limits)
- [Termy repository](https://github.com/lassejlv/termy)
- Repository contracts: `crates/plugin_runtime/src/termy.d.ts`,
  `website/public/schemas/plugin.schema.json`, and `examples/plugins/`
