# Spec-driven Workspace

This workspace runs the Spec-driven Playbook: a `spec` captures what is being
built and why, a `plan` captures how, and ordinary `task` nodes carry out the
work, linked back to both. Approval gates stop work from advancing past a stage
that has not been agreed.

## One methodology per workspace

The Playbook is already installed. Do not install it again, and do not install
a second methodology alongside it. If the user asks for a different way of
tracking work, ask before changing anything.

## Where the gates bite

A node and its links are separate writes, so nothing is checked when a spec,
plan or task is created and linked. The checks run when work advances:

- approving a plan requires it to be linked to an approved spec;
- a task linked to a plan cannot move to `in_progress` or `done` until that
  plan is approved and the task is also linked to the plan's spec;
- a task linked to a plan or spec cannot move to `done` without
  `custom:verification_method` recording how it was checked;
- a superseded spec or plan cannot be edited or moved back out of superseded.

Tasks with no `plan` or `spec` link are never checked. A refused change is the
workflow behaving as installed, not a fault to work around: explain it and
resolve its cause, never retry the write or strip the Play to get past it.

## Linking

Links are declared relationships, not mentions, and are created under their
forward names from the spec or plan side: from a plan to its spec as `spec`,
and from a plan or spec to a task as `tasks`.

## Where the details are

Narrower guidance covers each stage — fetch the one matching the task:

- writing a spec
- writing a plan from a spec
- creating implementation tasks
- completing a spec-driven task

Plays and schemas are ordinary nodes the user may have edited since install.
When the exact behavior matters, read the Play or schema itself rather than
trusting this summary.
