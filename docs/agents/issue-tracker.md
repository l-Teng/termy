# Issue tracker: GitHub

Issues and specs for this repo live as GitHub issues. Use the `gh` CLI for all operations.

## Conventions

- Create: `gh issue create --title "..." --body "..."`
- Read: `gh issue view <number> --comments`
- List: `gh issue list --state open --json number,title,body,labels,comments`
- Comment: `gh issue comment <number> --body "..."`
- Apply or remove labels: `gh issue edit <number> --add-label "..."` or `--remove-label "..."`
- Close: `gh issue close <number> --comment "..."`

Infer the repository from `git remote -v`. The `gh` CLI does this automatically inside the clone.

## Pull requests as a triage surface

**PRs as a request surface: no.**

When enabled, external pull requests use the same labels and states as issues.

GitHub shares one number space across issues and pull requests. Resolve an ambiguous `#42` with `gh pr view 42`, then fall back to `gh issue view 42`.

## When a skill says "publish to the issue tracker"

Create a GitHub issue.

## When a skill says "fetch the relevant ticket"

Run `gh issue view <number> --comments`.

## Wayfinding operations

- Map: one issue labelled `wayfinder:map`.
- Child ticket: an issue linked to the map as a GitHub sub-issue, with a `wayfinder:<type>` label.
- Blocking: use GitHub issue dependencies. If unavailable, add a `Blocked by: #<n>` line to the child body.
- Frontier: select the first open, unblocked, unassigned child in map order.
- Claim: `gh issue edit <n> --add-assignee @me`.
- Resolve: comment with the answer, close the child, then add its context pointer to the map.
