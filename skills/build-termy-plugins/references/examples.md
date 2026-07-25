# Examples and reusable patterns

Use the copyable templates under `assets/` for full files. These smaller patterns
show the intended v1 shape.

## Contents

- Minimal command
- Command inputs and fixed shell mapping
- Event-only plugin
- Settings and secret
- Storage
- Native view
- Keybinding
- Template map

## Minimal command

```ts
export default definePlugin({
  commands: [
    {
      id: "greet",
      title: "Hello: Greet me",
      keywords: ["hello", "example"],
      icon: "info",
      run({ context }) {
        context.toasts.success(`Welcome to Termy ${context.appVersion}`);
      },
    },
  ],
} satisfies TermyPlugin);
```

## Command inputs and fixed shell mapping

```ts
const gitCommands: Record<string, string> = {
  status: "git status --short --branch",
  branches: "git branch --all",
  commits: "git log --oneline -n 12",
};

export default definePlugin({
  commands: [
    {
      id: "inspect",
      title: "Git: Inspect repository",
      icon: "terminal",
      inputs: [
        {
          id: "view",
          type: "select",
          label: "What do you want to inspect?",
          required: true,
          options: [
            { value: "status", label: "Working tree status" },
            { value: "branches", label: "Local and remote branches" },
            { value: "commits", label: "Recent commits" },
          ],
        },
      ],
      run({ inputs, context }) {
        const command =
          typeof inputs.view === "string" ? gitCommands[inputs.view] : undefined;
        if (!command) {
          context.toasts.error("Choose a Git view first");
          return;
        }
        return {
          type: "terminal.run",
          command,
          workingDirectory: context.workingDirectory,
        };
      },
    },
  ],
} satisfies TermyPlugin);
```

Do not replace the fixed mapping with string interpolation.

## Event-only plugin

```ts
export default definePlugin({
  commands: [],
  events: {
    "terminal.ready"({ context }) {
      context.toasts.info("Terminal ready");
    },
    "workingDirectory.changed"({ event }) {
      console.log(event.previousWorkingDirectory, event.workingDirectory);
    },
    "command.finished"({ event }) {
      console.log(event.command, event.exitCode, event.durationMs);
    },
  },
} satisfies TermyPlugin);
```

Treat completion fields as optional, particularly under tmux.

## Settings and secret

```ts
export default definePlugin({
  settings: {
    greeting: {
      type: "text",
      title: "Greeting",
      defaultValue: "Hello from Termy",
    },
    token: {
      type: "secret",
      title: "API token",
    },
  },
  commands: [
    {
      id: "greet",
      title: "Hello: Greet me",
      run({ context }) {
        const greeting = context.settings.get("greeting") ?? "Hello from Termy";
        context.toasts.success(greeting);
      },
    },
  ],
} satisfies TermyPlugin);
```

Add no capability for settings. Never log or persist the secret value.

## Storage

Declare `"capabilities": ["storage"]`, then:

```ts
type Entry = { id: string; title: string };

const entries = (await context.storage.get<Entry[]>("entries")) ?? [];
await context.storage.set("entries", [
  ...entries,
  { id: crypto.randomUUID(), title: "New entry" },
]);
await context.storage.delete("obsolete-key");
```

Use JSON storage for small values only. Put larger persistent content under
`context.paths.dataDirectory`.

## Native view

Use `plugin.tsx`, declare `native-ui` (and `storage` here), and include:

```tsx
/** @jsxRuntime classic */
/** @jsx TermyUI.createElement */
/** @jsxFrag TermyUI.Fragment */

export default definePlugin({
  commands: [
    {
      id: "open",
      title: "Notes: Open",
      run() {
        return { type: "view.open", view: "notes" };
      },
    },
  ],
  views: {
    notes: {
      title: "Notes",
      async render({ context }) {
        const note = (await context.storage.get<string>("note")) ?? "";
        return (
          <TermyUI.Column gap="medium">
            <TermyUI.Text variant="heading">Quick note</TermyUI.Text>
            <TermyUI.TextInput
              id="note"
              value={note}
              maxLength={4096}
              submit="save"
            />
            <TermyUI.Button id="save" action="save" variant="primary">
              Save
            </TermyUI.Button>
          </TermyUI.Column>
        );
      },
      async onAction({ action, values, context }) {
        if (action.id !== "save") return;
        await context.storage.set("note", String(values.note ?? ""));
        context.toasts.success("Saved");
      },
    },
  },
} satisfies TermyPlugin);
```

Termy rerenders after `onAction`. Keep render cheap and all control IDs unique.

## Keybinding

Bind with manifest ID and command ID:

```txt
keybind = secondary-g=plugin:git-tools/inspect
```

Later keybinding lines win. `unbind` removes a shortcut. Task keybindings take
priority on conflicts.

## Template map

- `assets/command-plugin/`: official Git inspection example with safe fixed command
  mapping and native inputs.
- `assets/native-ui-plugin/`: official paginated todos example using storage,
  named native actions, and bounded rendering.

Copy a template into the user's requested destination, then change identity, command
IDs, titles, types, and behavior. Do not blindly retain capabilities or example
logic.
