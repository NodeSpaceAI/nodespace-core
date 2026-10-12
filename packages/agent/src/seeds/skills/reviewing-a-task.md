This is the whole procedure for reviewing a task that is waiting for review, from taking it to recording the outcome. The work itself is the implementer's: do not finish, fix or extend what you are reviewing. The findings you record are addressed by the session that implemented the task.

STEP 1, TAKE A TASK AWAITING REVIEW: <!-- include: task-review-take --> When the user names a task, read that one. <!-- include: task-context-read --> A plain task that was reviewed at all is reviewed only when the user names it: the queue lists issues.

STEP 2, MAKE SURE YOU ARE NOT ITS IMPLEMENTER: a session knows what it did. If in this conversation you started the task, ticked an item, wrote evidence under it or changed its pull request, you implemented it: stop, and tell the user that another session must review it. You review with nothing but the task, what governs it and the change.

STEP 3, READ THE TASK AND WHAT GOVERNS IT: read its checklist and the evidence under each item. <!-- include: task-review-read --> Read its spec's criteria, `boundaries` and sections, its plan's `approach` and sections, and every decision linked from the task and from its spec: they say what the work had to respect. Read each of those records whole, with its sections. <!-- include: section-read --> When the task is an `issue`, open the pull request in its `pull_request` and read the change itself. A task with no spec is reviewed against its own brief and checklist, and you say so in the note. Follow any standard that came back with the task: a project's own review rules are attached to it.

STEP 4, PASS ONE, AGAINST THE SPEC: this pass reads the change against what was asked and reports what is missing, unrequested or wrong, quoting the spec. Account for every item of the spec, its criteria and the task's checklist, and mark each one done, partial, not done, changed or unverifiable, with the evidence cited: the test, the file and line, or the output. An item ticked with no evidence, or with evidence that does not show it, is not met. Then list every change in behaviour the spec did not ask for. Write the accounting as a section of the task so the outcome is on the task and not only in this conversation. <!-- include: section-create --> Name the section "Review against the spec", with one text node per item, each beginning with its mark.

STEP 5, PASS TWO, AGAINST THE STANDARDS: this is a separate pass and is never merged with the first. Read the change against the standards attached to the project: its naming, its test strategy, its review rules. A change that meets every item of the spec can still break a standard, and a standard is not a reason to call an item of the spec unmet.

STEP 6, A WEAKENED CHECK GOES TO THE USER: a change that removes or loosens a test, a threshold or a lint rule is never decided by you and never by its implementer, whatever the reason given. For each one, put it to the user as restore or keep, with what was weakened, why it was changed, and your recommendation. <!-- include: ask-decision-brief --> Record their answer as a finding: restore is decided fix, and keep is decided reject, with their answer as the reason.

STEP 7, RECORD EVERY FINDING, THEN DECIDE EACH ONE: write each finding as a checkbox directly under the task, with its source (spec or standards) and your confidence in it, before any is decided or fixed. <!-- include: task-review-finding --> Two passes give two sets of findings: keep them apart by their source. A finding two reviewers raised independently counts for more, and several findings of one root cause are one finding, so write it once and say it. Then give each a decision, with the reason: fix, defer or reject. <!-- include: task-review-decide --> A finding that has no decision stops the move out of review.

STEP 8, RECORD THE OUTCOME: write what was checked and what was found as a note directly under the task. <!-- include: task-review-note -->

STEP 9, PASS IT OR SEND IT BACK: when no finding is left to fix, and nothing that governs the task is broken, pass it. <!-- include: task-review-pass --> An issue that passed is ready to merge and not done: done is set when the change lands. Otherwise send it back with its findings. <!-- include: task-review-return --> Tell the user the outcome either way. A task sent back returns to review when its implementer has fixed it. <!-- include: task-review-wait -->

<!-- include: version-conflict -->

<!-- include: rejected-write -->

ADAPTED FROM: code-review in mattpocock/skills, MIT licence, copyright 2026 Matt Pocock; review-triage and review-skill-example in OneRedOak/claude-code-workflows, MIT licence; plan-completion and the review checklist in garrytan/gstack, MIT licence.
