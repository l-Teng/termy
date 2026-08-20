/** @jsxRuntime classic */
/** @jsx TermyUI.createElement */
/** @jsxFrag TermyUI.Fragment */

type Todo = { id: string; title: string; done: boolean };
type TodoFilter = "all" | "open" | "done";

const STORAGE_KEY = "todos";
const PAGE_SIZE = 24;

async function loadTodos(context: TermyPluginContext): Promise<Todo[]> {
  return (await context.storage.get<Todo[]>(STORAGE_KEY)) ?? [];
}

function readFilter(params: Readonly<Record<string, TermyPluginJsonValue>>): TodoFilter {
  return params.filter === "open" || params.filter === "done" ? params.filter : "all";
}

export default definePlugin({
  commands: [
    {
      id: "open",
      title: "Todos: Open",
      keywords: ["tasks", "checklist"],
      icon: "info",
      run() {
        return { type: "view.open", view: "todos", params: { filter: "all" } };
      },
    },
  ],
  views: {
    todos: {
      title: "Todos",
      async render({ params, context }) {
        const todos = await loadTodos(context);
        const filter = readFilter(params);
        const visibleTodos = todos
          .filter((todo) =>
            filter === "all" ? true : filter === "done" ? todo.done : !todo.done,
          )
          .slice(0, PAGE_SIZE);
        const completed = todos.filter((todo) => todo.done).length;
        const completion = todos.length === 0 ? 0 : Math.round((completed / todos.length) * 100);

        return (
          <TermyUI.Column gap="medium">
            <TermyUI.Text variant="heading">Todos</TermyUI.Text>
            <TermyUI.Progress label={`${completed} of ${todos.length} complete`} value={completion} />
            <TermyUI.TextArea
              id="title"
              label="New todo"
              placeholder="What needs doing?"
              rows={3}
              maxLength={240}
              submit="add"
            />
            <TermyUI.Row gap="small" align="center">
              <TermyUI.Button id="add-button" action="add" variant="primary">
                Add
              </TermyUI.Button>
              <TermyUI.Select
                id="filter"
                label="Filter"
                value={filter}
                action="filter"
                options={[
                  { value: "all", label: "All" },
                  { value: "open", label: "Open" },
                  { value: "done", label: "Done" },
                ]}
              />
            </TermyUI.Row>
            <TermyUI.Divider />
            {visibleTodos.length === 0 ? (
              <TermyUI.EmptyState
                title="Nothing here"
                description={filter === "all" ? "Add your first todo." : "Try another filter."}
              />
            ) : (
              <TermyUI.List
                id="todos-list"
                action="toggle"
                filtering
                searchPlaceholder="Filter visible todos"
              >
                {visibleTodos.map((todo) => (
                  <TermyUI.ListItem
                    key={todo.id}
                    id={`todo-${todo.id}`}
                    title={todo.title}
                    subtitle={todo.done ? "Completed" : "Open"}
                    status={todo.done ? "Done" : undefined}
                    keywords={[todo.done ? "done" : "open"]}
                    payload={todo.id}
                    action="toggle"
                  />
                ))}
              </TermyUI.List>
            )}
            <TermyUI.Row gap="small" align="center">
              <TermyUI.Button id="clear" action="clear" disabled={completed === 0}>
                Clear completed
              </TermyUI.Button>
              <TermyUI.Button id="close" action="close">Close</TermyUI.Button>
            </TermyUI.Row>
          </TermyUI.Column>
        );
      },
      async onAction({ action, values, params, context }) {
        const todos = await loadTodos(context);
        if (action.id === "add") {
          const title = String(values.title ?? "").trim();
          if (!title) {
            context.toasts.info("Give the todo a title first");
            return;
          }
          await context.storage.set(STORAGE_KEY, [
            ...todos,
            { id: crypto.randomUUID(), title, done: false },
          ]);
          return;
        }
        if (action.id === "filter" && typeof action.value === "string") {
          return {
            type: "view.replace",
            view: "todos",
            params: { ...params, filter: action.value },
          };
        }
        if (action.id === "clear") {
          await context.storage.set(STORAGE_KEY, todos.filter((todo) => !todo.done));
          return;
        }
        if (action.id === "close") return { type: "view.close" };
        if (action.id === "toggle" && action.payload) {
          await context.storage.set(
            STORAGE_KEY,
            todos.map((todo) =>
              todo.id === action.payload ? { ...todo, done: !todo.done } : todo,
            ),
          );
        }
      },
    },
  },
} satisfies TermyPlugin);
