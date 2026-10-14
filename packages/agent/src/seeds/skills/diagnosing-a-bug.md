This is the procedure for a defect whose cause nobody has found. It ends in evidence, not in a guess: a reproduction, a cause, and a regression test that was seen to fail before the fix. When the report is already understood and only needs writing down as work, use Creating a Task instead. When what is unclear is what the user wants rather than why something fails, use Asking the User.

STEP 1, LOOK FOR WHAT IS ALREADY KNOWN: <!-- include: task-create-find-built --> <!-- include: decision-find-first --> A bug already recorded is worked there and not recorded again, and a decision that says the behaviour is intended is the answer: tell the user what it says and stop unless they want it reopened.

STEP 2, REPRODUCE IT: follow the reporter's steps and say exactly what happened, with the command or action and what it printed. A report you cannot reproduce is reported as not reproducible, with the steps tried and what came back, and the user decides what happens next. Nothing is fixed, and no cause is claimed, for a bug nobody has seen.

STEP 3, MINIMISE IT: remove steps, inputs and settings one at a time until the smallest case that still fails is left. Keep that case: it becomes the regression test.

STEP 4, HYPOTHESISE AND TEST ONE CAUSE AT A TIME: state a cause that would explain the minimal case, and then a check that would show it wrong. Run the check and write what it showed. Change one thing between runs, so that a result has one explanation. A cause is found when changing only it makes the failure go away and restoring it brings the failure back. Do not write down a cause you did not test.

STEP 5, PUT THE USER IN THE LOOP WHEN THE FIX IS NOT YOURS TO CHOOSE: a cause that has two sensible fixes, or whose fix changes behaviour somebody relies on, is a decision. Use Asking the User for it, and Recording a Decision when it will constrain later work.

STEP 6, GUARD IT WITH A TEST SEEN TO FAIL: write the regression test from the minimal case, run it against the code as it is and keep the failing output, then make the fix and run it again and keep the passing output. A test that never failed shows nothing about the bug. A bug with no code to test, such as a record in the wrong state, is guarded by the check that finds it, and the evidence says why no test was written.

STEP 7, WRITE WHAT YOU FOUND, WHERE THE WORK IS: when you were working a task, add a section to that task with the reproduction, the cause and the evidence. <!-- include: section-create --> When you were not, record it as work: <!-- include: task-create-node --> <!-- include: task-create-brief --> The brief is the reproduction, the cause and the evidence, and the checklist is the fix and its regression test, each unchecked. <!-- include: checkbox-item-create --> The bug is not written as fixed here: the work is done by Implementing a Task, which reads this record.

<!-- include: delivery-gates -->

<!-- include: rejected-write -->

WHEN DONE: tell the user the reproduction, the cause, the evidence for it, and where it is written. Say plainly when the cause was not found, and what was ruled out.

ADAPTED FROM: diagnosing-bugs in mattpocock/skills, MIT licence, copyright 2026 Matt Pocock. The rationalizations, red flags and verification of the gates above: addyosmani/agent-skills, MIT licence, copyright 2025 Addy Osmani.
