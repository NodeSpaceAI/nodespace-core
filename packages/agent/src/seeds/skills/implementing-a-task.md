This is the whole procedure for doing a task, from taking it to handing it over for review and fixing what the review sends back. Reviewing it is separate work with its own procedure: do not review a task you implemented, and do not mark it done.

STEP 1, TAKE A READY TASK: <!-- include: task-implement-take --> A ready task is open, has a checklist, is not blocked, has no plan waiting on approval, and has no spec waiting on approval. A small change needs no spec or plan: a task with a checklist of its own and no spec is taken and worked the same way. When the user names a task, or you are resuming one already in progress, read that task. <!-- include: task-context-read -->

STEP 2, START IT WITH THE VERSION YOU READ: <!-- include: task-implement-start --> Starting is how a task is claimed: of two sessions that read the same task, the second start is refused.

STEP 3, READ WHAT GOVERNS IT: what came back with the task is its context: its spec, its plan, the decisions linked from it and from its spec, and its project. Read the spec's `boundaries` and its sections (implementation decisions, testing decisions and what is out of scope), the plan's sections, the task's own brief (behaviour, interfaces and what is out of scope) and every decision before changing anything: they say what must hold. Read each of those records whole, with its sections. <!-- include: section-read --> Skills came back too: this procedure, and any standard attached to the task's project, such as naming conventions or a test strategy. Follow each of them.

STEP 4, WORK TO THE CHECKLIST: the task's checklist is the checkboxes directly under it, and it defines done. Work through it an item at a time. Never add, reword or remove an item to fit what was built: if an item is wrong or cannot be met, stop and tell the user.

STEP 5, TICK EACH ITEM AS IT IS MET, WITH EVIDENCE UNDER IT: tick an item when it is met and not before, and write what shows it directly beneath the item: the command that was run and what it printed, the test that now passes, the file and line. "Done" is not evidence. <!-- include: task-implement-tick --> Tick as you go and not all at the end, so a task that moved on under you is noticed at the next tick.

STEP 6, RECORD THE PULL REQUEST AND THE COMMITS: when the task is an `issue`, the type for a code change, record its pull request in `pull_request` and its commits in `commits`. <!-- include: task-implement-links --> A plain `task` has neither field and needs neither: skip this step for one.

STEP 7, MOVE IT TO REVIEW: when every item is ticked, move the task to `in_review`. <!-- include: task-implement-review --> Tell the user what was done, item by item, and where the pull request is.

STEP 8, WAIT FOR THE REVIEW AND FIX WHAT IT SENDS BACK: the review is another session's work, and its outcome is the task's status: `done` when it passed, back to `in_progress` with items unticked when something was not met. <!-- include: task-implement-wait --> A task that came back is yours to fix: read what the reviewer wrote under each unticked item, fix it, tick the item again with new evidence under it, and go back to STEP 7. A task that is `done` ends this procedure.

<!-- include: version-conflict -->

<!-- include: rejected-write -->
