This is the procedure for deciding, before a change is built, how someone will tell afterwards whether it worked. What it produces is criteria on the task, each one built and tested like any other, and not a separate system. The tools that collect and show them are the project's standards, attached to its project as skills. When the change is a defect to be explained, use Diagnosing a Bug.

STEP 1, READ WHAT THE CHANGE IS FOR: read the task, its spec and the spec's criteria, and the project's standards, which say what logging and measuring the project already has. <!-- include: task-complete-read --> What is worth measuring is what a criterion says should be true of the running change, and what a failure would look like to someone who cannot see inside.

STEP 2, WHAT IS LOGGED: for each place the change can fail or take a decision someone will later ask about, one log record that says what happened, to what, and why, with the identifiers needed to find the rest. Never what is private to a person, a secret, or the whole of a large input. A log written for every pass of a hot loop is a cost: say which are sampled.

STEP 3, WHAT IS MEASURED: three measures cover most changes: the rate at which it is used, the rate at which it fails, and how long it takes. State each, with its unit and what it is broken down by, and with what level would be a problem. A threshold with no source is the user's to confirm: use Asking the User.

STEP 4, WHERE A TRACE IS NEEDED: a trace is for a request that crosses a boundary, a process or a service, where a log alone cannot say where the time went or where it stopped. Say which boundary, and what is carried across it. A change that stays in one place needs none.

STEP 5, WRITE IT AS CRITERIA ON THE TASK: each log, measure and trace is a checkbox directly under the task, one condition that can be tested, such as "a failed export logs the file and the reason". <!-- include: checkbox-item-create --> Leave each one unchecked: Implementing a Task ticks it with the evidence that it emits, and Shipping a Change reads what it showed.

STEP 6, KEEP WHAT IT COSTS IN VIEW: every measure and log has a cost to write, to store and to read. Leave out what nobody would act on, and say what was left out and why.

<!-- include: rejected-write -->

WHEN DONE: tell the user what is logged, which measures are emitted and the level that is a problem for each, where a trace is needed, and the items written on the task.

ADAPTED FROM: observability-and-instrumentation in addyosmani/agent-skills, MIT licence, copyright 2025 Addy Osmani.
