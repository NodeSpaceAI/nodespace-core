Tasks are how a plan gets done: one task per unit of work someone can pick up and finish, each with a checklist that says when it is finished. A task under a plan is an ordinary task. What ties it to the plan is two links and its checklist.

A SMALL CHANGE NEEDS NO SPEC AND NO PLAN: a task with a checklist of its own and `requires_spec` set to false is ready to work on as it is, and without that field a task cannot be started until it links an approved spec. Write the task and its checklist, set `requires_spec` to false, and stop there. Never write a spec or a plan only to have something to link a task to.

READ THE PLAN AND ITS SPEC FIRST: <!-- include: task-breakdown-read --> Read the plan's `approach` and every criterion of the spec. If the plan is not approved, say so: its tasks can be written now, and none can start until the user approves the plan.

ONE TASK PER UNIT OF WORK: follow the parts the approach names. A task is small enough to finish and review in one piece, and its title says what will exist when it is done. Look at what the plan already has before adding to it. <!-- include: task-breakdown-existing --> <!-- include: task-breakdown-create -->

GIVE EACH TASK ITS OWN CHECKLIST: a task's checklist is the checkboxes directly under it. Write it from the spec's criteria: for each criterion the task helps meet, the condition this task must satisfy, stated so it can be tested. Every criterion of the spec is covered by at least one task. Leave every item unchecked. <!-- include: checkbox-item-create --> A task under a plan that has no checklist can never be finished.

LINK EACH TASK TO THE PLAN AND TO THE SPEC: both, so the task's spec is one step away. <!-- include: task-breakdown-link --> A task belongs to one plan: linking it to a second moves it.

SAY WHERE ORDER MATTERS: where one task cannot start until another is finished, link the two. <!-- include: task-breakdown-blocks --> Link only real dependencies: a blocked task is not offered as ready until its blocker is done or cancelled.

WHEN DONE: tell the user the tasks written, which can start now, and which wait on another task or on the plan's approval. Do not start any of them.
