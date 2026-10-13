This is the whole procedure for doing a task, from the user's choice of it to handing it over for review. Reviewing it is separate work, done by another session with its own procedure: do not review a task you implemented, and do not mark it ready to merge or done. When the review sends findings back, Addressing Review Findings is the procedure.

STEP 1, THE USER CHOOSES THE WORK: you do not take a task on your own. When the user names a task, or you are resuming one already in progress, read that task. <!-- include: task-context-read --> When the user asks what to work on, list what is ready and wait: <!-- include: task-implement-list --> Say what is ready in priority order, name the one you would take and why, and stop until the user chooses. Carrying on with a task this conversation already holds, such as addressing its review findings, is not a new choice. A task the user hands you that has no checklist is not ready: shape it first by following Creating a Task from the step that asks what must be true (then its size, its brief and checklist), tell the user the checklist you wrote, and start only once they agree. A task that is waiting on a question is not ready either: the question belongs to the person it is assigned to, and you do not answer it for them, so follow Answering a Question and put it to the user.

STEP 2, START IT WITH THE VERSION YOU READ: <!-- include: task-implement-start --> Starting is how a task is claimed: of two sessions that read the same task, the second start is refused, and a refused start means another session holds it, so choose again with the user. A task that is blocked, or whose spec or plan is not approved, is refused the same way, and the message says what to do.

STEP 3, READ WHAT GOVERNS IT: what came back with the task is its context: its spec, its plan, the decisions linked from it and from its spec, and its project. Read the spec's `boundaries` and its sections (implementation decisions, testing decisions and what is out of scope), the plan's sections, the task's own brief (behaviour, interfaces and what is out of scope) and every decision before changing anything: they say what must hold, and where the tests go. Read each of those records whole, with its sections. <!-- include: section-read --> Skills came back too: this procedure, and any standard attached to the task's project, such as naming conventions or a test strategy. Follow each of them.

STEP 4, WORK TEST-FIRST, ONE ITEM AT A TIME: the task's checklist is the checkboxes directly under it that are not findings of a review, and it defines done. Never add, reword or remove an item to fit what was built: if an item is wrong or cannot be met, stop and tell the user. For each item, in order:
- write one failing test for that item, at a boundary the spec agreed, so it checks behaviour and not structure;
- run it and see it fail, for the reason the item names, and keep that output;
- write only enough code to make it pass, run it and keep that output;
- tick the item (STEP 5), then go to the next item.
Writing all the tests first and the code afterwards is not this: a test written with no code to fail against proves nothing about the code it was meant to drive, and tests written in bulk pin the behaviour you imagine rather than the behaviour you build. An item with no behaviour to test, such as a rename or a document, is ticked with what shows it met, and says why no test was written.

STEP 5, TICK EACH ITEM AS IT IS MET, WITH EVIDENCE UNDER IT: tick an item when it is met and not before, and write what shows it directly beneath the item: the test and the failing run, then the passing run, with the commands that were run and what they printed, and the file and line. "Done" is not evidence, and a passing run with no failing run before it is not test-first. <!-- include: task-implement-tick --> Tick as you go and not all at the end, so a task that moved on under you is noticed at the next tick.

STEP 6, RUN THE FULL SUITE ONCE: when every item is ticked, run the project's whole test suite once, the command being in the standards attached to the project (ask the user when none says). Do not run it after each item: the focused tests are the loop, and the full run is the check. A real failure gets a bounded number of fix attempts, three at most, and then you stop and tell the user. A failure that is flaky gets a single rerun. A failure of the environment, or one that was already there before your change, is not yours to fix: stop and report it. The gates below apply to a rerun as to the first run.

STEP 7, RECORD THE PULL REQUEST AND THE COMMITS: when the task is an `issue`, the type for a code change, record its pull request in `pull_request` and its commits in `commits`. <!-- include: task-implement-links --> A plain `task` has neither field and needs neither: skip this step for one.

STEP 8, MOVE IT TO REVIEW: when the suite passes, move the task to `in_review`. <!-- include: task-implement-review --> Tell the user what was done, item by item, and where the pull request is.

STEP 9, HAND IT OVER: the review is another session's work and its outcome is the task's status. <!-- include: task-implement-wait --> An issue that passed is `ready_to_merge`, and `done` is set when the change lands, not by you. An issue with findings is `changes_requested`: keep this session, because it holds the context, and follow Addressing Review Findings, which came with the task from the Changes requested queue. A plain task that passed is `done`.

SEVERAL TASKS OF ONE SPEC IN PARALLEL: each is a session of its own. The frontier is the Ready tasks list, and the claim is the start in STEP 2: a session that is refused takes another from the list. Nothing else is tracked.

<!-- include: delivery-gates -->

<!-- include: version-conflict -->

<!-- include: rejected-write -->

ADAPTED FROM: implement, implement-spec and tdd in mattpocock/skills, MIT licence, copyright 2026 Matt Pocock. The rationalizations, red flags and verification of the gates above: addyosmani/agent-skills, MIT licence, copyright 2025 Addy Osmani.
