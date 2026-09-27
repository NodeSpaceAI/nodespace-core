---
title: "Writing a Plan from a Spec"
description: "Draft the technical plan for an approved spec: approach, components, sequencing and risks, linked back to the spec. Use when the user says plan this out, what's the approach, how should we build this, or asks for a plan for an existing spec."
tools: "create_node, update_node, create_relationship, search_nodes, get_node"
---
# Writing a Plan from a Spec

A plan written without its spec's content behind it is a guess in a plan's
shape. Before drafting anything, fetch the spec and read its `objective` and
`success_criteria` — never plan from the spec's title alone.

## The spec must be approved

A plan cannot be approved unless it is linked to a spec whose `spec_status` is
`approved`; the write is rejected, not warned about. If the spec is still
`draft`, tell the user it needs their approval first. You may still draft the
plan, but it will stay `draft` until the spec is approved.

## Link the plan to its spec

The link is a declared relationship, not a `nodespace://` mention in the text.
Create it with `create_relationship` from the plan to the spec, relationship
type `spec`. A plan without it has no lineage: it can never be approved, and
tasks under it can never start.

A plan has exactly one spec. Linking it to a second one replaces the first.

## Fill the fields with substance

- `approach` — the major components, their dependencies and the order they are
  built in. Refer to the spec's success criteria by what they say, and show
  which part of the approach satisfies each.
- `risks` — what could go wrong with this approach specifically. Not a
  boilerplate list that would fit any plan.

## One active plan per spec

Before creating a plan, check the spec's `plans`. If an approved one already
exists, a new plan usually means the old one is being replaced: ask the user
whether to set the old plan's `plan_status` to `superseded`, rather than
leaving two live plans against one spec. A superseded plan's approach and risks
are locked, and it cannot be moved back.

## Approval is the user's, not yours

Create the plan with `plan_status: draft`. After the plan exists and is linked,
show the user its approach and risks and ask them to approve it. Approving it is
what lets the tasks under it start.
