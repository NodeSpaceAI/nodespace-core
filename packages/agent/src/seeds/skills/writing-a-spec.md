A spec is what every plan and task under it is judged against: what is being built, what is out of bounds, and the conditions the finished work must meet. Those conditions are its criteria, and each one is a checkbox directly under the spec. Write it from what the user has told you and what already exists, never from a guess.

CHECK FOR AN EXISTING SPEC FIRST: a second spec for the same objective splits the work between two. <!-- include: spec-find-first --> A draft that already covers it is edited, not written again. An approved one that is wrong is replaced: see REPLACING A SPEC.

SETTLE WHAT IS OPEN BEFORE WRITING: when the request is ambiguous, or the user says they are not sure what they want, use the skill Asking the User first, and write nothing until its decisions are answered. Do not ask what you can look up.

LOOK AT WHAT EXISTS FIRST: when the work changes code, read how it does the thing today before deciding anything, and use the vocabulary the project already uses. Read the decisions already recorded in the area and respect them. A spec written without it repeats work that exists or contradicts a decision that was made.

SETTLE WHERE THE TESTS GO WITH THE USER: a spec says which public boundaries the finished work is tested through, so that a test checks what a user or caller can see and never how it is built. Prefer a boundary that already exists, and use the highest one that can show the behaviour. Fewer is better: one is ideal. Put the choice to the user with Asking the User, and record the answer first under Testing decisions, before any task is split from the spec.

WRITE THE SPEC IN ITS FIXED SECTIONS, with no file paths and no code: they go stale, and what a spec records is the decision, not where it sits today. The one exception is a snippet from a prototype that states a decision more precisely than prose can, such as a state machine or a type shape, trimmed to the decision-rich part and marked as from a prototype.

`objective` is a short statement: what is being built, why and for whom, in the user's terms. `boundaries` says what may be done without asking, what needs the user's sign-off first, and what must never be done.

The rest is ordinary nodes beneath the spec, a header for each section and a text node under it for each item: Problem, as the user faces it; Solution, as the user will see it; User stories, one "As a <who>, I want <what>, so that <why>" to a node, covering every behaviour the user would notice; Implementation decisions, which modules are built or changed, their interfaces, schema changes, contracts, and every technical decision the user made; Testing decisions, what a good test checks, which boundaries are tested, and a similar test that already exists; and Out of scope. <!-- include: section-create -->

CREATE THE SPEC FIRST, then its sections. <!-- include: spec-create -->

WRITE EACH CRITERION AS A CHECKBOX DIRECTLY UNDER THE SPEC, not under a section header, since only direct children count: a criterion is one condition that can be tested, taken from the user stories and the testing decisions. "The export finishes in under a second for 10,000 rows" is one; "works correctly" is not. Leave every one unchecked. <!-- include: checkbox-item-create -->

NEVER INVENT A CRITERION: if the request does not say how done will be judged, ask. A guessed criterion is worse than none, because every plan and task under the spec inherits it. A placeholder such as "TBD" in a field is the same mistake.

APPROVAL IS THE USER'S: a new spec is a `draft`, and stays one until the user approves it. Show them the objective, the boundaries, the sections and the criteria word for word and ask whether to approve. <!-- include: approval-ask-first --> <!-- include: spec-approve --> A spec with no checkbox under it cannot be approved: that write is rejected. A recommendation to approve is never an approval.

REPLACING A SPEC: an approved spec that turns out wrong is not edited into a different one. Write a new spec, then set the old one's `spec_status` to `superseded`, in a write that changes nothing else. A superseded spec's fields are locked and it cannot be moved back, so what was built against it still reads what it was built against.

<!-- include: rejected-write -->

WHEN DONE: tell the user what was captured, where the tests go, and that the spec is a draft waiting on their approval. Do not start a plan in the same turn: once the spec is approved, the plan is written and reviewed with the skill Reviewing a Plan.

ADAPTED FROM: to-spec, in mattpocock/skills, MIT licence, copyright 2026 Matt Pocock.
