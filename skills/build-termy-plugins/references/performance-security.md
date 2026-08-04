# Performance and security

Use this reference to design or review any plugin that handles untrusted input,
shell commands, files, credentials, network requests, subprocesses, frequent
events, persistent data, or native UI.

## Contents

- Trust model
- Imports and installation
- Worker and timeout model
- Exact limits
- Performance design rules
- Shell and process safety
- Storage and secrets
- Native UI safety
- Reload behavior
- Review checklist

## Trust model

Termy plugins are trusted Bun code, not sandboxed browser extensions. They run with
the user's normal file, network, and process authority. Worker isolation contains
many crashes/timeouts from Termy's app, but does not protect the user's account
from malicious code.

`storage` and `native-ui` capabilities gate only Termy-owned APIs. They are not
operating-system permissions and do not restrict `fetch`, `Bun.*`, Node built-ins,
files, subprocesses, or the network.

Consequences:

- Read source before installing.
- Minimize plugin code and capabilities.
- Do not describe a plugin as sandboxed merely because it runs in a Worker.
- Treat third-party plugins like any other executable local tool.

## Imports and installation

V1 allows local relative TypeScript imports that stay inside the plugin directory,
plus Bun and Node built-ins such as `bun` and `node:fs`.

Termy rejects package imports, out-of-root paths, and symlinks. It never installs
packages or runs build hooks.

GitHub installation downloads regular files as data without cloning, evaluating
code, running scripts, or installing dependencies. Execution begins only when the
plugin loads. Interactive CLI installs require confirmation; automation uses
`--yes`.

## Worker and timeout model

- Termy uses one external Bun host. Lifecycle subscribers keep it warm; eventless
  plugins restart it on demand after idle suspension.
- Each plugin runs in its own Worker.
- A crash or timeout is contained to the failed Worker.
- If the host transport exits, Termy rebuilds it and reloads Workers at the next
  refresh.
- Events stay ordered within one plugin; separate plugins may handle them
  concurrently.
- The default handler timeout is 10 seconds.
- A command or view may set `timeoutMs` from 100 through 30,000 milliseconds.
- A child process may outlive its Worker. Stop it explicitly when cancellation
  matters.
- Persist durable state through `context.storage` or managed files; module globals
  may be reset when an eventless host sleeps.

Do not increase a timeout to hide slow architecture. Move expensive work out of hot
render/event paths, cache stable results, bound I/O, and give the user progress or a
clear error.

## Exact limits

| Area | Limit |
| --- | --- |
| Installed plugins | 32 |
| Plugin source tree | 4,096 files, 16 MiB total |
| Definitions per plugin | 512 commands, 64 settings, 32 views |
| Command inputs | 16 per command |
| Select options | 128 per select |
| Returned actions | 32 per invocation |
| Selected terminal text | 64 KiB, truncated on a UTF-8 boundary |
| Small JSON storage | 512 values, 1 MiB total per plugin |
| Native UI document | 256 nodes |
| Native UI nesting | 16 levels |
| Native UI children | 64 per node |
| Value-bearing controls | 64 per document |
| Text input value | 4,096 characters |
| Handler timeout | default 10 s; configurable 100–30,000 ms |

Design well below the hard ceilings. Limits are safety boundaries, not targets.

## Performance design rules

### Commands

- Validate inputs before network, filesystem, or subprocess work.
- Map select choices to constant commands.
- Prefer one typed returned action over multiple cross-boundary calls.
- Bound output and reject work that cannot finish predictably.
- Use async APIs for I/O and preserve useful timeout headroom.

### Lifecycle events

- Keep handlers idempotent and cheap; directory/tab/command events can be frequent.
- Avoid scanning repositories, walking large trees, or calling remote APIs on every
  event.
- Cache by relevant context such as directory or tab, and invalidate deliberately.
- Coalesce redundant work inside the plugin if a burst can occur.
- Do not assume events from different plugins serialize globally.

### Storage

- Use small JSON values for compact state, not large datasets or binary content.
- Use `dataDirectory` for larger persistent files and `cacheDirectory` for
  recomputable data.
- Avoid rewriting a large collection on every keystroke or render.
- Store indexes/summaries separately when they avoid repeated full scans.

### Native UI

- Treat `render` as a hot path because every action rerenders the document.
- Load only state needed for the visible page.
- Paginate or window dynamic collections; official examples use a page size of 24.
- Keep node depth shallow and control IDs stable/unique.
- Avoid network calls, subprocesses, and unbounded file reads directly in render.
- Persist in `onAction`; render the resulting state.

### Reloads and bundling

When the palette opens, Termy fingerprints the manifest and source tree. It bundles
each changed plugin once with `Bun.build({ target: "bun" })` at:

```text
plugins/.termy-cache/bundles/<id>/<content-hash>.mjs
```

Relative imports participate in the content hash and bundle. Unchanged plugins
reuse cached modules. Keep the source tree small and do not add generated outputs,
dependency trees, or unrelated assets.

## Shell and process safety

Never interpolate free-form text directly into `terminal.run`.

Prefer:

```ts
const commands: Record<string, string> = {
  status: "git status --short --branch",
  branches: "git branch --all",
};
const command = typeof inputs.view === "string" ? commands[inputs.view] : undefined;
if (!command) return;
return { type: "terminal.run", command, workingDirectory: context.workingDirectory };
```

If a task truly requires user text:

- Avoid a shell when a Bun/Node API can perform the operation.
- Validate length and syntax against a narrow allowlist.
- Quote for the actual target shell; do not assume POSIX on Windows.
- Keep secrets out of arguments, logs, terminal output, and URLs.
- Track and terminate spawned processes on cancellation/timeout when appropriate.

## Storage and secrets

Small storage is plain local JSON. Never put credentials there. Declare a `secret`
setting so Termy uses the operating-system credential store and masks the value in
Settings.

Treat managed data/cache paths as plugin-scoped organization, not as an operating
system sandbox. Trusted Bun code can still access other user paths.

Disabling preserves managed source and data. Uninstalling removes the managed copy,
storage, data, and cache, while leaving the original development folder untouched.

## Native UI safety

Termy JSX is validated data, not direct renderer access. Only documented components,
semantic props, and string/boolean control values are accepted. Arbitrary HTML,
React, CSS, colors, assets, callbacks, GPUI properties, unknown nodes, and unknown
props are rejected.

- Use named action strings rather than callbacks.
- Require `onAction` for any interactive view.
- Validate `action.id`, `payload`, `value`, and current persisted state.
- Do not trust hidden client state; decide again inside `onAction`.
- Keep destructive actions explicit and preferably confirmed.

## Review checklist

- [ ] Source is small enough to audit and uses no packages/build hooks.
- [ ] Capabilities are minimal and accurately declared.
- [ ] Optional context/event fields are checked.
- [ ] Selection truncation is handled where completeness matters.
- [ ] User-controlled text cannot become unquoted shell syntax.
- [ ] Secrets use `secret` settings.
- [ ] Files/network/processes are bounded and errors are surfaced.
- [ ] Child processes have an ownership and cleanup plan.
- [ ] Lifecycle handlers are cheap, idempotent, and burst-safe.
- [ ] Storage remains below limits and large data uses managed files.
- [ ] Native UI is paginated and remains below node/control/value limits.
- [ ] Runtime tests cover reload, timeout/error, native/tmux, and uninstall/disable
  semantics when relevant.
