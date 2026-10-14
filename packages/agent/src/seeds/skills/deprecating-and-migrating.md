This is the procedure for retiring something that exists: code, an interface, a data shape or a feature. Whether anything depends on it decides everything after it, so that comes first. The work itself is sliced by Splitting a Plan into Tasks, which this procedure feeds and does not restate. When nothing is being retired, only changed in place with no old form left behind, use Writing a Spec.

STEP 1, DOES IT HAVE USERS: ask this first, with Asking the User for what only the user knows, and look up what you can: the callers in the code, the records that use the shape, the people or systems that rely on it. <!-- include: ask-look-up --> A user is anything that would break or lose data when it goes: another module, a stored record, a person's habit, a system outside the project. Record the answer as a decision before any removal is planned, following Recording a Decision: <!-- include: decision-find-first --> <!-- include: decision-create --> No removal is planned until that decision exists.

STEP 2, SHOULD IT BE RETIRED AT ALL: a thing that works and costs little to keep is cheaper to keep than to move. State what keeping it costs, what retiring it costs, and who bears each. When the answer is not plain, it is the user's: use Asking the User, and record the choice.

STEP 3, FIND THE USAGE, AND SAY HOW: before anything is removed, state how usage is found and run it: a search of the code for callers, a count of the records in the old shape, the logs or measures that show what is still reached. Write what it printed. Usage found only by asking people is a lead and not a result. State, too, what evidence will show it gone, and that is the condition for the removal.

STEP 4, SEQUENCE THE WORK AS EXPAND, MIGRATE, CONTRACT: the new form is added beside the old one (expand), each user is moved to the new form (migrate), and the old form is removed once the evidence from step 3 shows no use (contract). Write that sequence into the plan and give it to Splitting a Plan into Tasks to slice, with the removal as its own task whose checklist holds the evidence that usage is gone.

STEP 5, STATE ROLLBACK FOR EACH STEP, AND WHO IS TOLD: for each of the three, say what undoes it, how long it can be undone, and who needs to know when it is. The removal is the one that cannot always be undone: say what is lost, and what is kept so that it can be rebuilt.

STEP 6, WHEN THE PROJECT HAS NO USERS: a project that has no users, or has declared itself pre-release, may skip the parallel-running steps, and replace the old form directly. That is the project's standards to say, and they are read before it is assumed: a standard attached to the project, as a skill, states it and says when it ends. Without one, step 1's decision is the answer: record that there are none, and why.

STEP 7, SAY WHAT IS DEPRECATED AND UNTIL WHEN: a thing marked deprecated says what replaces it and when it will be removed, in the place its users read. A deprecation with no replacement and no date is not one: it is zombie code, kept by nobody's choice. Look for these too: a thing that nothing uses and nobody owns is removed through this procedure, and a thing marked deprecated that has outlived its date is a task.

<!-- include: rejected-write -->

WHEN DONE: tell the user whether it has users and where that decision is recorded, how usage is found and what it showed, the sequence and the task that holds the removal, and the rollback for each step.

ADAPTED FROM: deprecation-and-migration in addyosmani/agent-skills, MIT licence, copyright 2025 Addy Osmani.
