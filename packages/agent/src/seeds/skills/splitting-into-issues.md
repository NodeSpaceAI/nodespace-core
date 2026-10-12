Splitting turns an approved plan into issues someone can pick up and finish. Each one is a thin vertical slice: a narrow but complete path through every layer the work touches, that can be shown or verified on its own, and small enough to be done in one fresh context window. A slice that cannot be is split again.

A SMALL CHANGE NEEDS NO SPEC AND NO PLAN: an issue or task with a checklist of its own and no spec is ready to work on as it is. Write it and its checklist, and stop there. One that links a spec cannot be started until that spec is approved. Never write a spec or a plan only to have something to link to.

THE KIND OF WORK DECIDES THE TYPE: a change to code is an `issue`. Anything else, such as a document to write, a decision to reach or a discussion to hold, is a plain `task`. Slice both the same way. A plain task has no pull request or commits and needs none.

READ THE PLAN AND ITS SPEC FIRST: <!-- include: task-breakdown-read --> Read the plan's `approach`, the spec's `boundaries` and every criterion, and every decision linked from the spec: what was decided there holds for every slice. If the plan is not approved, say so: the issues can be written now, and none can start until the user approves the plan.

MAKE THE CHANGE EASY FIRST: if the work would be simpler after a preparatory change, such as a refactor that makes room for it, that is the first slice and the others wait on it.

DRAFT THE SLICES, each one a behaviour that exists when it is done and not a layer of the work. Each says which slices must be finished before it can start: those are its blockers. A slice with none can start at once. Look at what the plan already has before adding to it. <!-- include: task-breakdown-existing -->

A WIDE REFACTOR IS THE EXCEPTION: one mechanical change, such as renaming a field or retyping a shared symbol, whose effect fans across the whole codebase, so one edit breaks thousands of call sites and no slice can land green on its own. Do not force it into a vertical slice. Sequence it as expand, migrate, contract: an expand issue that adds the new form beside the old so nothing breaks; then migrate issues that move the callers over in batches sized by their reach, such as a package or a directory, each blocked by the expand issue and leaving everything green because the old form still exists; then a contract issue that removes the old form, blocked by every migrate batch. When even the batches cannot stay green alone, keep the sequence, let them share one integration branch and make them all block a last issue that integrates and verifies, where green is promised.

AGREE THE BREAKDOWN WITH THE USER BEFORE WRITING ANYTHING: show a numbered list, and for each slice its title, what it blocks on and the behaviour it delivers. Ask with the skill Asking the User whether the granularity is right, whether each blocking edge is a real dependency, and whether any slice should be merged or split. Iterate until the user agrees. Write nothing until then.

WRITE THEM IN DEPENDENCY ORDER, blockers first, so that each blocking link can name a real id. For each slice:

CREATE IT: <!-- include: task-breakdown-create --> Its title says what will exist when it is done.

GIVE IT A BRIEF THAT LASTS: the brief is the contract the one who picks it up works from, and it may sit for days while the code changes under it. Describe behaviour, not procedure. Give the current behaviour and the desired behaviour, with the edge cases and the error conditions. Name the types, signatures or shapes to look for or change under key interfaces. State what is out of scope so that nothing adjacent is gold-plated. Never give a file path or a line number, and never assume the present structure will stay. <!-- include: task-breakdown-brief -->

GIVE IT ITS OWN CHECKLIST: a checklist is the checkboxes directly under it. Write it from the spec's criteria: for each criterion the slice helps meet, the condition this slice must satisfy, stated so it can be tested. Every criterion of the spec is covered by at least one slice. Leave every item unchecked. <!-- include: checkbox-item-create --> One with no checklist can never be finished.

LINK IT TO THE PLAN AND TO THE SPEC: both, so the spec is one step away. <!-- include: task-breakdown-link --> It belongs to one plan: linking it to a second moves it. When the work belongs to a project the user named, or the plan's other slices already sit in one, link it to that project too. <!-- include: task-breakdown-project -->

LINK ITS BLOCKERS: <!-- include: task-breakdown-blocks --> Link only real dependencies: a blocked slice is not offered as ready until its blockers are done or cancelled.

READINESS IS NOT YOURS TO SET: an issue is ready when the Ready tasks queue lists it, which needs a checklist, no unfinished blocker, and, if it has a spec or a plan, those approved. Never set a status or a tag to say so, and do not change the spec or the plan.

WHEN DONE: tell the user the issues written, which can start now, and which wait on another issue or on the plan's approval. Do not start any of them.

ADAPTED FROM: to-tickets and the agent brief of triage, in mattpocock/skills, MIT licence, copyright 2026 Matt Pocock.
