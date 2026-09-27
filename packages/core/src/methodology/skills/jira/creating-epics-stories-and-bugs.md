---
title: "Creating Epics, Stories, and Bugs"
description: "How to create work in a Jira-style workspace: choosing between task, story, bug and epic, story points, bug severity and environment, linking work to an epic, and sub-tasks."
tools: "create_node, create_relationship, search_nodes, get_node"
---
# Creating Epics, Stories, and Bugs

`story`, `bug` and `epic` all extend `task`. Each carries everything a task
does — status, priority, assignee, due date, blocking edges — plus its own
fields. Plain `task` is Jira's "Task" issue type; there is no separate schema
for it.

## Which type

| Type | Use for | Its own fields |
|---|---|---|
| `task` | Work that is neither a requirement nor a defect | — |
| `story` | A requirement, described from the user's side | `story_points` |
| `bug` | A defect in something that already exists | `severity`, `environment` |
| `epic` | A large body of work that other issues deliver | `target_date` |

All four share the task status workflow — `open`, `in_progress`, `done`,
`cancelled`. There is no separate story or bug status field.

## Story points

A number, on whatever scale the team uses. Leave it unset rather than guessing.

## Severity is not priority

`severity` is how bad the defect is: `critical`, `major`, `minor`, `trivial`.
The inherited `priority` is how urgently to fix it. Set them independently — a
trivial typo on the landing page can be high priority, and a major bug in a
retired feature can be low. `environment` is free text: where it was seen.

## Putting work in an epic

Link with the `issues` relationship, from the epic to the task, story or bug.
There is no `epic` field to set. From the other side the same link reads as
`epic`, and an issue has at most one — linking it to a second epic moves it.

Do not nest work under an epic in the outline. An epic groups issues by
reference, the way Jira's Epic Link does; nesting means something else.

## Sub-tasks

A sub-task is any task, story or bug nested under another in the outline.
There is no sub-task type and no field — create the child under its parent.
When every child is `done` or `cancelled`, the parent is marked `done`
automatically.

## Labels and components

Collections, not fields. Add membership rather than looking for a `labels` or
`components` field.
