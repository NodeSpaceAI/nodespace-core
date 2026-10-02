# Completing a Spec-Driven Task

A task linked to a plan or spec cannot move to `done` until its
`custom:verification_method` field records how the work was checked. The status
change is rejected otherwise. Tasks outside the methodology — no plan, no spec —
are not affected.

## Record the verification first

Two separate writes, in this order:

1. Set `custom:verification_method` with `update_node`.
2. Then set the status to `done`.

The other order fails: the status change is checked against the field as it
stands, and it is still empty.

## Write what actually happened

"done" or "tested" is not a verification method; it tells nobody tracing back
through this task anything. Say what was run or checked and what it showed:

- "test: `bun run test` — 0 new failures"
- "manual: followed steps 1–4 from the spec's success criteria; each produced
  the expected output"

Check the spec first. If its `success_criteria` names a specific check, say
whether that exact check was met — not a generic substitute.

If nothing was actually verified, say so plainly, and ask the user whether to
close the task anyway. Never write a check that did not happen.

## A rejection that looks wrong

If marking the task done is rejected for a different reason — the plan is not
approved, or the task is not linked to its spec — see "Creating Implementation
Tasks". These rules are ordinary Plays the user installed and can inspect; a
rejection is not something to route around by writing the status another way.

## When you are done

Tell the user the verification method you recorded.
