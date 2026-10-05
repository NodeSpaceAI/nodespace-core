A plan says how one spec will be met: the approach, tied to the spec's criteria, and the risks of that approach.

READ THE SPEC FIRST: a plan written from a spec's title is a guess. <!-- include: plan-read-spec --> Read its `objective`, its `boundaries` and every criterion before drafting anything.

ONE LIVE PLAN PER SPEC: <!-- include: plan-existing-plans --> A draft plan that is already there is edited, not written again. An approved one is replaced: ask the user whether the old plan is superseded before leaving two live plans against one spec.

FILL THE FIELDS WITH SUBSTANCE: `approach` names the parts of the work, what each depends on and the order they are built in, and says which part meets each of the spec's criteria, quoting the criterion. `risks` says what could go wrong with this approach in particular, not a list that would fit any plan.

CREATE THE PLAN AND LINK IT TO ITS SPEC: <!-- include: plan-create-and-link --> The link is what makes it this spec's plan; naming the spec in the text does not. A plan has one spec, and a plan with none can never be approved.

APPROVAL IS THE USER'S: a new plan is a `draft`, and one created already approved is rejected. Show the user the approach and the risks and ask whether to approve. <!-- include: approval-ask-first --> <!-- include: plan-approve --> A plan can be approved only once its spec is approved. If the spec is still a draft, say that it needs the user's approval first, and leave the plan a draft.

REPLACING A PLAN: write the new plan, then set the old one's `plan_status` to `superseded`, in a write that changes nothing else. A superseded plan's fields are locked and it cannot be moved back.

<!-- include: rejected-write -->

WHEN DONE: tell the user which spec the plan is for and that it is a draft waiting on their approval. Do not write its tasks in the same turn.
