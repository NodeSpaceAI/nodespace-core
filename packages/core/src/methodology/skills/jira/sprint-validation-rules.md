---
title: "Sprint Validation Rules"
description: "Why a change to a sprint was rejected in a Jira-style workspace: illegal status moves, starting without dates, and the lock on a closed sprint's dates, completed date and issues."
---
# Sprint Validation Rules

Several rules can reject a change to a sprint outright. A rejection is the
system working as configured — not a bug, and not something to retry
unchanged. Each runs inside the transaction of the write it checks, so a
rejected change never partially lands.

## Illegal status move

`sprint_status` moves only `future` → `active` → `closed`. Anything else is
rejected: reopening a closed sprint, moving an active one back to `future`,
jumping straight from `future` to `closed`, or clearing the field.

To proceed: to close a future sprint, start it first. To continue work from a
closed sprint, plan it into a new sprint.

## Starting without dates

Setting `sprint_status` to `active` is rejected unless `start_date` and
`end_date` are both set. Set them — in the same update is fine — then start it.

## Creating a sprint already started

A sprint must be created as `future`, without `completed_date`. Create it, then
start it.

## A closed sprint is locked

Once `closed`, a sprint rejects:

- changes to `start_date` or `end_date`
- adding work to its `issues`, or removing work from them
- any change to `completed_date`

Its name and `goal` stay editable. To carry work forward, link it to a future
sprint and leave the closed sprint's record alone.

## completed_date is automatic

`completed_date` is set once, automatically, when the sprint closes. Setting it
by hand is rejected, before the close or after.

## If a rejection looks wrong

Report it rather than working around it. The rules are ordinary Plays the user
can inspect, edit or disable, installed with the methodology — a rejection that
seems incorrect is a question about their configuration, not something to route
around.
