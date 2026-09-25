---
title: "Working with Cycles"
description: "How cycles work in a Linear-style workspace: assigning work via the tasks relationship, the derived active/past state, and what the automatic cycle-creation and rollover Plays do."
---
# Working with Cycles

A `cycle` is a time-boxed iteration — Linear's sprint equivalent.

## Assigning work

Work joins a cycle through the `tasks` relationship, not a property. Create the
edge from the cycle to the task or issue; there is no `cycle_id` field to set.

The relationship targets `task`, so plain tasks and issues can both be assigned.
Reading `cycle.tasks` returns both.

## There is no status field

A cycle's state is derived by comparing its dates to today:

- `start_date` in the future → upcoming
- today between the dates → active
- `end_date` in the past → over

Do not look for a `status` property and do not add one. Storing it would mean
two copies of the same truth, one of which would drift.

## What runs automatically

**Cycle creation.** On the day a cycle ends, its successor is created, starting
the next day and running for `duration_days` — read off the ending cycle, so
changing cadence means editing that field, not the Play.

**Create the first cycle yourself.** This triggers on an existing cycle reaching
its end date, so with no cycle in the graph nothing ever fires. Create one with
a `start_date` and `end_date`; the automation takes over from there.

**Rollover.** In the same run, the ending cycle's unfinished tasks move to the
successor — added to the new cycle and removed from the old one, so a task
belongs to exactly one cycle. Tasks whose status is `done` or `cancelled` stay
in the ending cycle as its record of what it accomplished.

Runs daily just after midnight, on whichever devices are online. If several are,
they converge on the same result rather than duplicating it.

## Estimate totals

Not stored. Sum `estimate` across `cycle.tasks` when a total is wanted, rather
than expecting a field.
