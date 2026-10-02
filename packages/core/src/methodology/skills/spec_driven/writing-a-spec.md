# Writing a Spec

A spec is the source of truth every plan and task under it is judged against.
A thin spec produces a thin plan and tasks nobody can check. The rules on
approval can only see that `spec_status` is `approved` — they cannot tell
whether the spec says anything. That judgment is yours, before you approve it.

## Check for an existing spec first

Search for nodes of type `spec` before creating one. A second spec for the
same objective splits the lineage instead of extending it. If the existing one
is superseded, the new spec replaces it; say so to the user.

## Fill every field with the user's real answers

All three are fields on the `spec` node:

- `objective` — what is being built, why, and for whom, in the user's own
  terms. Not a restatement of the title.
- `success_criteria` — testable conditions. "`bun run test` passes with no new
  failures" is a criterion; "works correctly" is not.
- `boundaries` — what you may always do without asking, what needs the user's
  sign-off first, and what you must never do.

**Do not invent success criteria.** If the request does not say how "done"
will be judged, ask. A guessed criterion is worse than none: every plan and
task inherits it as false confidence. The same goes for placeholders — "n/a",
"TBD", "see title" defeat the reason the node exists. Each field is a real
question; ask it when you do not know the answer.

## Approval is the user's, not yours

Create the spec with `spec_status: draft`. Set `approved` only after the user
has confirmed the objective and success criteria — approval is what lets a plan
against this spec be approved, so it is not something to set on the turn you
create the spec unless the user has already reviewed the content in this
conversation.

## Replacing a spec

An approved spec that turns out wrong is not edited into a different spec.
Create a new one and set the old one's `spec_status` to `superseded`. Once
superseded, its objective, success criteria and boundaries are locked, and it
cannot be moved back — so plans and tasks that referenced it keep reading what
they were built against.

## When you are done

Tell the user what you captured — the objective and success criteria, verbatim
— and ask them to confirm before you approve it. Do not start a plan in the
same turn.
