This is the whole procedure for reviewing a task that is waiting for review, from taking it to recording the outcome. The work itself is the implementer's: do not finish, fix or extend what you are reviewing.

STEP 1, TAKE A TASK AWAITING REVIEW: <!-- include: task-review-take --> When the user names a task, read that one. <!-- include: task-context-read -->

STEP 2, READ THE TASK AND WHAT GOVERNS IT: read its checklist and the evidence under each item. <!-- include: task-review-read --> Read its spec's criteria and `boundaries`, its plan's `approach`, and every decision linked from the task and from its spec: they say what the work had to respect. Open the pull request in `pull_request` and read the change itself. Follow any standard that came back with the task: a project's own review rules are attached to it.

STEP 3, CHECK EACH ITEM AGAINST ITS EVIDENCE: for each checklist item, decide whether the evidence under it shows the item is met, and whether the change does what the evidence says. An item ticked with no evidence, or with evidence that does not show it, is not met. Then check the change against the boundaries and the decisions: work that meets its checklist and breaks a decision does not pass.

STEP 4, RECORD THE OUTCOME: write what was checked and what was found as a note directly under the task, so the outcome is on the task and not only in this conversation. <!-- include: task-review-note -->

STEP 5, PASS IT OR SEND IT BACK: when every item is met and nothing that governs the task is broken, mark it done. <!-- include: task-review-pass --> Otherwise untick each item that is not met, say under it what is missing, and move the task back to `in_progress`. <!-- include: task-review-return --> Tell the user the outcome either way.

<!-- include: version-conflict -->

<!-- include: rejected-write -->
