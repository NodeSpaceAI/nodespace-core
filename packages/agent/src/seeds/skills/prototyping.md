This is the procedure for a design question that running code answers faster than asking does. A prototype is a throwaway built to answer one stated question, and it is discarded when the question is answered. What it leaves behind is a recorded decision, never the prototype. When the question is what the user wants, use Asking the User: no prototype answers a preference. When the answer is a fact in a source, use Researching.

STEP 1, STATE THE ONE QUESTION: write the question the prototype answers, as a sentence that has a result, such as "does the sync of ten thousand rows finish in under a second with the batched call". If you cannot state one question, you do not have a prototype to build yet: use Asking the User. Say what each possible answer would change, and if no answer would change anything, do not build it.

STEP 2, LOOK FOR AN ANSWER BEFORE BUILDING: <!-- include: decision-find-first --> <!-- include: ask-look-up --> A question already decided is not prototyped again.

STEP 3, BUILD THE SMALLEST THING THAT ANSWERS IT: it is throwaway, so it has no tests of its own, no error handling beyond what the question needs, and no place in the project's structure. Build it where it will be thrown away, outside the project's source, and say where. Several candidates are built side by side when the question is a choice, so that they are measured the same way.

STEP 4, RUN IT AND WRITE WHAT IT SHOWED: run it, and keep the command and what it printed. State the answer to the question in one sentence, and say how sure it is: a single run on one machine is a result and not a rule. Do not widen the question afterwards to fit what was built.

STEP 5, DISCARD IT: delete the prototype. A snippet that states a decision more exactly than prose can, such as a state machine or a type shape, is kept in the decision, trimmed and marked as from a prototype. Nothing else is carried into the real work: the real work is written test-first from the decision, not grown from the prototype.

STEP 6, RECORD THE DECISION: follow Recording a Decision, and cite what the prototype showed in its body: the question, the command, what it printed and what was turned down because of it. <!-- include: decision-create --> A decision that settles a spec or a task is linked from it. <!-- include: decision-link --> Its status is the user's to set.

<!-- include: rejected-write -->

WHEN DONE: tell the user the question, the answer and its evidence, that the prototype was discarded, and the decision it was recorded in.

ADAPTED FROM: prototype in mattpocock/skills, MIT licence, copyright 2026 Matt Pocock.
