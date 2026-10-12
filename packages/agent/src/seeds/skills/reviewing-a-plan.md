A plan says how one approved spec will be met: the approach, tied to the spec's criteria, and the risks of that approach. This is also where the plan is checked, before any work is split from it, for its architecture, its tests and its performance, and for what it leaves out. Review it the way an engineering lead would before committing a team to it: the smallest clear change, explicit over clever, every behaviour tested, every edge case handled, and no abstraction the work does not need.

READ THE SPEC FIRST: a plan written from a spec's title is a guess. <!-- include: plan-read-spec --> Read its `objective`, its `boundaries` and every criterion, then its sections, which hold the decisions made, where the tests go and what is out of scope. <!-- include: section-read --> Read all of it before drafting anything. If the spec is still a draft, say that it needs the user's approval first: the plan can be drafted and reviewed now, and cannot be approved until the spec is.

ONE LIVE PLAN PER SPEC: <!-- include: plan-existing-plans --> A draft plan that is already there is edited, not written again. An approved one is replaced: ask the user whether the old plan is superseded before leaving two live plans against one spec.

DRAFT THE APPROACH: `approach` names the parts of the work, what each depends on and the order they are built in, and says which part meets each of the spec's criteria, quoting the criterion. For each part, look for what already solves it and say whether it is reused or built, and why.

CHALLENGE THE SCOPE: ask what the smallest change is that meets every criterion, and what can wait without blocking it. Count the files the plan touches and the new services or modules it adds, as an estimate, and say it is one. When the plan touches more than eight files or adds two or more new services or modules, stop and put it to the user as a decision: which parts to cut or defer, and whether a smaller arrangement meets the same criteria. Whatever is left out, with one sentence on why, is an item in a Not in scope section of the plan.

REVIEW IT IN FOUR PASSES, in this order, and never skip one. Architecture: boundaries, coupling, data flow, single points of failure, security where data crosses a boundary, and one realistic production failure for each new path. Code quality: structure, duplication (share code only when it improves reliability or removes code, not because two things look alike), gaps in error handling, needless complexity. Tests: for each behaviour the plan adds or changes, the test that will show it, at the boundary the spec chose, and every gap; a test is worth writing only for a regression it would catch. Performance: queries in loops, unbounded reads, missing indexes, work that blocks, each with its scale or marked unknown, never an invented number.

LIST THE FAILURE MODES: for each new path, name one realistic way it fails, whether a test covers it, whether the work handles it, and whether the user would see the failure or it would pass silently. One with no test, no handling and a silent result is a GAP. Close it by adding the test or the handling to the approach before the plan is shown for approval. `risks` is a sentence or two on what could go wrong with this approach in particular, not a list that would fit any plan; the full list is the Failure modes section.

CREATE THE PLAN AND LINK IT TO ITS SPEC: <!-- include: plan-create-and-link --> The link is what makes it this spec's plan; naming the spec in the text does not. A plan has one spec, and a plan with none can never be approved.

WRITE THE REVIEW AS ORDINARY NODES beneath the plan, a header for each section and a text node under it for each item: Not in scope, one deferral to a node, and Failure modes, one to a node. Write each gap as an unticked checkbox in the Failure modes section, and tick it when its test or handling is in the approach; no gap is left unticked when the plan is shown for approval, so the ticked gaps are the record of what the review found and closed. <!-- include: section-create -->

PUT EACH POINT TO THE USER AS A DECISION, AND RECORD IT: every finding that changes the plan is a decision, put to the user one at a time with the skill Asking the User, with your recommendation and its reason. Write the answer down with the skill Recording a Decision and link it to the spec, so that the work split from the plan carries it. A correction of fact that changes nothing is fixed and reported, not asked.

APPROVAL IS THE USER'S: a new plan is a `draft`, and one created already approved is rejected. Show the user the approach, the Not in scope and Failure modes sections and the decisions made, and ask whether to approve. <!-- include: approval-ask-first --> <!-- include: plan-approve --> A plan can be approved only once its spec is approved. A recommendation to approve is never an approval.

REPLACING A PLAN: write the new plan, then set the old one's `plan_status` to `superseded`, in a write that changes nothing else. A superseded plan's fields are locked and it cannot be moved back.

<!-- include: rejected-write -->

WHEN DONE: tell the user which spec the plan is for, which decisions were recorded, any gap still open, and that the plan is a draft waiting on their approval. Do not split it into issues in the same turn: once it is approved, that is the skill Splitting a Plan into Tasks.

ADAPTED FROM: plan-eng-review, in garrytan/gstack, MIT licence, copyright 2026 Garry Tan. Only its procedure and rubric are used.
