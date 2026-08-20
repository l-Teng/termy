const gitViews = [
  {
    value: "status",
    label: "Working tree status",
    status: "git status",
    keywords: ["changes", "branch"],
  },
  {
    value: "branches",
    label: "Local and remote branches",
    status: "git branch",
    keywords: ["refs", "remote"],
  },
  {
    value: "commits",
    label: "Recent commits",
    status: "git log",
    keywords: ["history", "log"],
  },
] satisfies TermyPluginSelectOption[];

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
      keywords: ["git", "status", "branches", "commits"],
      status: "Plugin",
      icon: "terminal",
      when: { hasWorkingDirectory: true },
      inputs: [
        {
          id: "view",
          type: "pick",
          label: "What do you want to inspect?",
          placeholder: "Search Git views",
          required: true,
          loadOptions({ query }) {
            const normalized = query.trim().toLowerCase();
            if (!normalized) return gitViews;
            return gitViews.filter((option) =>
              [option.label, option.status, ...option.keywords].some((value) =>
                value.toLowerCase().includes(normalized),
              ),
            );
          },
        },
        {
          id: "confirmed",
          type: "confirm",
          label: "Open the result in a new tab?",
          defaultValue: true,
        },
      ],
      async run({ inputs, context }) {
        if (inputs.confirmed !== true) {
          context.toasts.info("Git inspection cancelled");
          return;
        }

        const command =
          typeof inputs.view === "string" ? gitCommands[inputs.view] : undefined;

        if (!command) {
          context.toasts.error("Choose a Git view first");
          return;
        }

        context.progress.report({ message: "Opening Git inspection", percentage: 50 });
        if (context.signal.aborted) return;

        return {
          type: "terminal.open",
          location: "tab",
          workingDirectory: context.workingDirectory,
          launch: { type: "shell", command },
          target: "origin",
        };
      },
    },
  ],
} satisfies TermyPlugin);
