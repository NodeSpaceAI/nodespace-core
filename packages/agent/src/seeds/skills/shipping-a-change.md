This is the procedure for getting a finished change to the people who use it, safely, and knowing afterwards that it worked. It starts where the review ends. What lands a change, and what runs to release it, is the team's own: this procedure runs no pipeline and names none. The commands and tools are the project's standards, attached to its project as skills. When the change is not finished, use Implementing a Task.

STEP 1, READ WHAT THE CHANGE WAS FOR: read the task, its spec and the spec's criteria, and the project's standards. <!-- include: task-complete-read --> The pre-release checklist is drawn from those criteria and not invented here: each criterion the change was built to meet, and each thing the standards require before a release.

STEP 2, WRITE THE PRE-RELEASE CHECKLIST AS ITEMS ON THE TASK: one condition to an item, each testable: the criteria met, the checks the standards name run on what is being released, the people told, the changes to data that go with it. <!-- include: checkbox-item-create --> Leave each one unchecked. An item that is not met is not skipped: it is put to the user.

STEP 3, STATE THE ROLLOUT: when the project releases in stages, such as a few users first, a percentage or an environment at a time, say the stages, what is watched between them and what moves it on. When it has no stages, say so: one release is the whole rollout. The user's standards say which; do not assume one.

STEP 4, STATE ROLLBACK BEFORE RELEASING: say how the change is undone, how long that takes, what it cannot undo, such as data written in the new shape, and who decides to do it. A change with no rollback is released only when the user has been told that and has agreed. Write it on the task, and write who was told.

STEP 5, RELEASE THROUGH THE PROJECT'S OWN MEANS, AND WRITE WHAT HAPPENED: the release is done by the means the standards name, and what it printed is the evidence under the item. <!-- include: task-implement-tick --> A release step that you cannot do from here is the user's: say so, and wait.

STEP 6, OBSERVE AFTERWARDS, AND RECORD IT: look at what the change was meant to move, using the measures and logs Instrumenting a Change put in place, for as long as the rollout says. Write what was seen, with the figure and when it was taken, as evidence under the task, and say whether it met what the spec asked for. A release with nothing observed afterwards is not known to have worked: say that and no more.

<!-- include: delivery-gates -->

<!-- include: rejected-write -->

WHEN DONE: tell the user what was released and in which stages, the rollback and who was told, and what was observed afterwards and where it is written.

ADAPTED FROM: shipping-and-launch in addyosmani/agent-skills, MIT licence, copyright 2025 Addy Osmani.
