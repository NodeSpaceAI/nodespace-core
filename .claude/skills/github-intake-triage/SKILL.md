---
name: github-intake-triage
description: Handle an issue that someone outside the team filed on GitHub - read it, shape it into NodeSpace work, answer and close it. Use for any GitHub issue; the team's own work is never filed there.
allowed-tools: Bash(bun run gh:*), Bash(gh issue close:*), Bash(gh pr create:*), Read
---

## The rule

GitHub issues are the public intake only, and the team's work is tracked in NodeSpace. Read the decision "GitHub issues are the public intake only, and the team's work is tracked in NodeSpace" in the NodeSpace workspace (`nodespace search "public intake only"`, then `nodespace node get <id>`).

- The team does not create, edit, assign, or set the status of a GitHub issue for its own work. Work the team finds is created in NodeSpace as a task or a spec of the `nodespace-core` project (`54751b77-462a-44e9-af89-409245df92d9`).
- A problem about NodeSpace Sync belongs in nodespace-sync, not here. A generic extension point that the Sync app uses is core work.

## Triage an outside report

1. Read it: `bun run gh:view <number>` (run from the repository root, never from a subdirectory).
2. Shape it into a NodeSpace task, or a spec for something large, following the skills "Filing and Editing an Issue in nodespace-core" and "Working an Issue in nodespace-core". Put the issue's URL in the task's `## Source` section.
3. Reply on the issue with where the work lives, then close it: `bun run gh:comment <number> --body "..."`, then close it: `gh issue close <number> -c "<comment>"` (no `gh:` script closes an issue now).
4. Do not report a duplicate or a report that is not wanted without saying why in the reply.

## Pull requests

A pull request description names its NodeSpace task. Use the closing keyword (`Closes #N`) only for an issue that came from outside, so the reporter is notified when the change lands. From a worktree, whose local branch carries the `worktree-` prefix, create the pull request with `gh pr create --repo NodeSpaceAI/nodespace-core --base main --head issue-<number>-brief-desc --title "..." --body "Closes #N"`; `bun run gh:pr` works only from a branch whose name matches the remote branch, and it writes `Closes #N` as the body, so use it only for such an issue. The merge queue reads the pull request, not an issue number, so a pull request without a closing keyword merges normally.

## Failure issues from checks

A scheduled workflow that fails files its issue through `bun run gh:tracking-issue --title "..." --body-file <path> [--labels "a,b"]`, which comments on the open issue with the same title instead of filing a duplicate. A person reading it shapes the fix into a NodeSpace task under the rule above.

## Commands that remain

`gh:view`, `gh:comment`, `gh:pr`, `gh:tracking-issue`, `gh:help`. The scripts that created, edited, assigned, unassigned, listed or set the board status of an issue were removed.
