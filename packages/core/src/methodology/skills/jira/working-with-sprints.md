# Working with Sprints

A `sprint` is a time-boxed iteration with its own lifecycle, stored in
`sprint_status`.

## The lifecycle

| `sprint_status` | Means | Moves to |
|---|---|---|
| `future` | Being planned | `active` |
| `active` | Running | `closed` |
| `closed` | Finished — a record | nothing |

Create a sprint as `future`. Its dates may be unset while it is being planned.

**Start it** by setting `sprint_status` to `active`. This needs both
`start_date` and `end_date`; set them in the same update if they are not set
yet.

**Close it** by setting `sprint_status` to `closed`. `completed_date` is then
recorded automatically. Do not set it yourself.

Moves are one way. A closed sprint cannot be reopened and an active one cannot
go back to future.

## Planning work into a sprint

Link with the `issues` relationship, from the sprint to the task, story or bug.
From the other side the same link reads as `sprint`. There is no `sprint`
field to set.

An issue can be linked to several sprints over its life: work unfinished when
a sprint closes stays on that sprint's record and is linked to the next one
too. Keep it in at most one sprint that is not closed.

## After a sprint closes

Its name (content) and `goal` can still be edited. Its dates, its
`completed_date` and the set of issues it holds cannot. To carry unfinished
work forward, link it to a future sprint — do not remove it from the closed
one.

## Dates

`start_date` and `end_date` are the plan. `completed_date` is when it actually
closed, which can differ from `end_date`.
