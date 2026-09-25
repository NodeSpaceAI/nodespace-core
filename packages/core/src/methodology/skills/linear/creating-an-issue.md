---
title: "Creating an Issue"
description: "How to create an issue in a Linear-style workspace: when to use issue rather than task, the extended status and priority vocabularies, and point estimates."
---
# Creating an Issue

`issue` extends `task`. Create an `issue` for tracked product or engineering
work; a plain `task` is still right for a one-off to-do that is not part of the
tracked workflow.

An issue carries everything a task does — assignee, due date, blocking edges —
plus the following.

## Status

`status` is `task`'s own field with extra values, not a separate field:

| Value | Means | Reads as, to a task-scoped reader |
|---|---|---|
| `triage` | Not yet assessed | `open` |
| `backlog` | Assessed, not scheduled | `open` |
| `open` | Ready to pick up | `open` |
| `in_progress` | Being worked | `in_progress` |
| `in_review` | Work done, awaiting review | `in_progress` |
| `done` | Complete | `done` |
| `cancelled` | Abandoned | `cancelled` |

The right-hand column matters: a query or Play written against `task` sees the
mapped value, never the raw one. So a `task`-scoped report counting
`in_progress` work includes issues sitting in `in_review`.

## Priority

`urgent` sits above `highest`; `none` means deliberately unprioritized, which
is different from leaving the field unset.

## Estimate

Points on a modified-Fibonacci scale: 1, 2, 3, 5, 8. The gaps are the point —
an 8 says "clearly large" rather than a precise wrong number. Leave it unset
rather than guessing; the field is extensible if a team wants 13 or 21.

## Sub-issues

A sub-issue is just an issue nested under another in the outline. There is no
field for it, and no special call — create the child and put it under the
parent. Note the completion gate: a parent cannot be closed while a child is
still open.

## Labels and teams

Collections, not fields. A label is a collection of issues; a team is a
collection of people. Add membership rather than looking for a `labels` field.
