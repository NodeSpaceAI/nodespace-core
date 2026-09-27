---
title: "Creating Implementation Tasks"
description: "Break an approved plan into tasks linked to both the plan and its spec. Use when the user says create tasks for this plan, break this down, let's start implementing, or asks to turn a plan into actionable work."
tools: "create_node, create_relationship, search_nodes, get_node"
---
# Creating Implementation Tasks

Under spec-driven development a task is not a bare task: it traces to the plan
it carries out and the spec that justifies it. Tasks are ordinary `task` nodes
— there is no special type — and the tracing is two relationships.

## What is enforced, and when

A task linked to a plan cannot move to `in_progress` or `done` unless:

- the plan's `plan_status` is `approved`, and
- the task is also linked to the spec that plan implements — any other spec
  does not count.

The check runs when the status changes, not when the task is created. A task
can be created and linked while its plan is still in draft, and it waits in
`open` until the plan is approved. Cancelling is always allowed.

## Before creating

Fetch the plan and check `plan_status`. If it is not `approved`, tell the user:
the tasks can be created now, but none can start until they approve the plan.

Search for an existing task first. If the user asks for "a task for X" and one
already exists under this plan, link or reuse it rather than creating a
duplicate. Likewise, never create a new spec or plan just to have something to
link to — find the real one.

## Link both

After creating the task, create two relationships:

1. From the plan to the task, relationship type `tasks` — the task's `plan`.
2. From the spec to the task, relationship type `tasks` — the task's `spec`.

Link the spec directly even though the plan already points at it, so the
task's spec is one hop away. A task belongs to one plan; linking it to a second
plan moves it.

If there is no plan yet, do not link the task to a spec alone to make it look
traced. Write the plan first (see "Writing a Plan from a Spec").

## When you are done

Tell the user which plan and spec the task traces to, and whether it can start
now or is waiting on the plan's approval.
