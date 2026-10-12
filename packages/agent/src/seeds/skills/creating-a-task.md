Work that did not come out of a spec still has to be shaped before anyone can take it: a bug, a chore, something found in the middle of other work, a finding that was deferred. It is shaped when it is created, by whoever creates it, so that it is ready the moment it exists. A task made this way is one unit of work with a brief and a checklist, worked on one branch and delivered by one pull request.

This procedure is adapted from the triage procedure of `mattpocock/skills` (MIT licensed), with its labels and state machine left out: whether a task is ready is the Ready tasks queue, and its category is whatever its brief says.

STEP 1, IS IT ALREADY BUILT: look before creating anything. <!-- include: task-create-find-built --> Check the code as well when the work is a change to code. Tell the user where you looked and what you found. A request that is already built is not created: say where it lives and stop.

STEP 2, WAS IT DECLINED BEFORE: <!-- include: decision-find-first --> A decision that records this as something that will not be done is the answer: tell the user what it says and why, and stop unless they want it reopened.

STEP 3, REPRODUCE A BUG BEFORE SHAPING IT: for a bug, follow the reporter's steps and say what happened. A bug that reproduces is shaped from what you saw. One that does not is reported as not reproducible, with the steps tried and what came back, and the user decides whether it is shaped at all. Never write down a bug nobody has seen.

STEP 4, ASK WHEN THE REQUEST IS UNCLEAR: what must be true, for whom, and what is out of bounds. Use the skill Asking the User, one decision at a time, and wait for each answer. Never fill a gap with a guess.

STEP 5, DECIDE WHAT IT IS AND HOW BIG IT IS, WITH THE USER: a change to code is an `issue` in its software project, and anything else is a plain `task`. Small and clear goes straight to a brief and a checklist, with no spec. Big or risky gets a spec first, and its issues or tasks come from splitting its plan: follow Writing a Spec and do not create the work yet. When it is not plain which of the two this is, ask the user whether it needs a spec, with Asking the User.

STEP 6, WRITE THE BRIEF: the task's title says what will exist when it is done. Its brief is the text under it: durable, so it stays true as the code changes, and behavioural, so it says what the result does and not how it is built. Name no file paths or line numbers. Say what is out of scope. <!-- include: task-create-node --> <!-- include: task-create-brief -->

STEP 7, WRITE THE CHECKLIST: the checkboxes directly under the task define done. Each is one condition that can be tested, stated as behaviour. Leave every one unchecked. <!-- include: checkbox-item-create --> A task with no checklist is not ready and appears in Needs shaping.

STEP 8, LINK ITS PROJECT: <!-- include: task-create-project-link -->

WHEN A REQUEST IS DECLINED: when the user decides this will not be done, leave no open task. Record a decision that says what will not be done and why, so STEP 2 finds it next time, and mark it accepted once they agree. <!-- include: decision-create --> <!-- include: decision-accept --> If a task was already created for it, cancel the task. <!-- include: task-status-dedicated-verb -->

WHEN YOU CANNOT GO ON WITHOUT AN ANSWER: when the user is the one who owes it, ask them directly and wait. When it must come from someone else, or cannot come now, do not leave the task looking ready: create a question task that holds the question, specific enough to answer without a conversation, assigned to the person who owes the answer (the task's creator when nobody is named), in the same project, and blocking the stopped task. <!-- include: task-question-create --> The stopped task keeps its status and its assignee. The person sees the question in Waiting on you, and the stopped task is ready again when the question is finished.

<!-- include: rejected-write -->

WHEN DONE: tell the user what was created, its project, its checklist, and whether it is ready now or waiting on a question or a spec. Do not start the work in the same turn.
