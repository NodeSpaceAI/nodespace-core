A spec is what every plan and task under it is judged against: what is being built, what is out of bounds, and the conditions the finished work must meet. Those conditions are its criteria, and each one is a checkbox directly under the spec.

CHECK FOR AN EXISTING SPEC FIRST: a second spec for the same objective splits the work between two. <!-- include: spec-find-first --> A draft that already covers it is edited, not written again. An approved one that is wrong is replaced: see REPLACING A SPEC.

FILL THE FIELDS WITH THE USER'S ANSWERS: `objective` says what is being built, why, and for whom, in the user's own terms, and says more than the title does. `boundaries` says what may be done without asking, what needs the user's sign-off first, and what must never be done. <!-- include: spec-create -->

WRITE EACH CRITERION AS A CHECKBOX UNDER THE SPEC: a criterion is one condition that can be tested. "The export finishes in under a second for 10,000 rows" is one; "works correctly" is not. Leave every one unchecked. <!-- include: checkbox-item-create -->

NEVER INVENT A CRITERION: if the request does not say how done will be judged, ask. A guessed criterion is worse than none, because every plan and task under the spec inherits it. A placeholder such as "TBD" in a field is the same mistake.

APPROVAL IS THE USER'S: a new spec is a `draft`, and stays one until the user approves it. Show them the objective and the criteria word for word and ask whether to approve. <!-- include: approval-ask-first --> <!-- include: spec-approve --> A spec with no checkbox under it cannot be approved: that write is rejected.

REPLACING A SPEC: an approved spec that turns out wrong is not edited into a different one. Write a new spec, then set the old one's `spec_status` to `superseded`, in a write that changes nothing else. A superseded spec's fields are locked and it cannot be moved back, so what was built against it still reads what it was built against.

<!-- include: rejected-write -->

WHEN DONE: tell the user what was captured and that the spec is a draft waiting on their approval. Do not start a plan in the same turn.
