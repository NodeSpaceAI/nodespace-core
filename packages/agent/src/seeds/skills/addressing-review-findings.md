This is the whole procedure for dealing with what a review sent back, in the session that implemented the issue and so holds its context. The review is another session's work: do not review the issue again yourself, and do not mark it ready to merge or done.

STEP 1, READ THE FINDINGS: when the user names the issue, read that one. <!-- include: task-context-read --> Otherwise take the next one waiting on its fixes: <!-- include: task-fix-take --> Read the issue with its findings, each with the evidence under it. <!-- include: task-review-read --> The findings are the checkboxes whose text opens with Finding, and each states its source, its confidence, its severity, its decision and its reason. Text nodes that open with Suggestion are optional improvements: take one if it is small and you hold its context, and leave it otherwise. Never record a decision on one.

STEP 2, EVERY FINDING HAS A DECISION BEFORE ANY FIX: a finding with no decision is not yours to guess at. Tell the user, and wait for the reviewer to decide it. A decision is made before any fix, and is fix, defer or reject, and each is answered the same way: a finding decided fix is fixed, and a finding decided defer or reject is left as decided. If you disagree with a decision, say so to the user: do not change it, and do not fix a finding that was decided reject. A finding decided defer must open its reason with "size small", "size medium" or "size large": if it does not, or the note says it is small and its context is loaded, tell the user it should be fixed now and wait for the reviewer.

STEP 3, FIX EACH FINDING DECIDED FIX, TEST-FIRST: for each one, in the order of the reviewer's confidence, write a failing test that shows the finding, see it fail, write only enough code to pass it, and see it pass. A finding that is about a standard and not about behaviour is fixed and shown with what shows it met. Tick the finding, and write directly under it the failing run and the passing run, or what shows it fixed. <!-- include: task-implement-tick -->

STEP 4, CHECK THE WHOLE: run the project's full test suite once. A real failure gets three fix attempts at most and then you stop and tell the user. A flaky failure gets a single rerun. A failure of the environment, or one that was already there, is reported and not fixed here.

STEP 5, RECORD THE COMMITS AND SEND IT BACK TO REVIEW: when every finding decided fix is ticked, record the new commits. <!-- include: task-implement-links --> Then write a re-review note as a text node directly under the issue, opening with "Re-review note (": which findings changed behaviour and which only changed a standard, and the depth you recommend: none when only standards or documentation changed, the delta when behaviour changed within what the findings named, or the whole change when a fix reached beyond them. The reviewer decides the depth. Then move the issue back to `in_review` for the reviewer to confirm. <!-- include: task-fix-resubmit --> The move is refused while a finding decided fix is unchecked, and the message names it. Tell the user what was fixed, finding by finding, and that the issue is back in review.

<!-- include: delivery-gates -->

<!-- include: version-conflict -->

<!-- include: rejected-write -->

ADAPTED FROM: the address-review step of review-triage in OneRedOak/claude-code-workflows, MIT licence, and the per-finding record in plan-eng-review of garrytan/gstack, MIT licence; the re-review recommendation of this repository's own address-review command, as a source of ideas. The rationalizations, red flags and verification of the gates above: addyosmani/agent-skills, MIT licence, copyright 2025 Addy Osmani.
