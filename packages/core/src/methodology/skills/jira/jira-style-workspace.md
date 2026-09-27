---
title: "Jira-style Workspace"
description: "What workflow this workspace uses: the Jira-style Playbook installed here — its epic, story, bug and sprint types, the Plays that gate sprints, its saved views, and the schema ids they were actually created under."
---
# Jira-style Workspace

This workspace runs the Jira-style Playbook: work is tracked as `task`,
`story` and `bug` nodes, grouped into `epic` nodes and planned into `sprint`
nodes that move from future to active to closed.

## One methodology per workspace

The Playbook is already installed. Do not install it again, and do not install
a second methodology alongside it — a second vocabulary colliding with this one
is not a merge, it is two half-configured setups in one graph. If the user asks
for a different way of tracking work, ask before changing anything.

## Rejections are the system working

Sprint changes are gated. A change the Plays refuse — reopening a closed
sprint, starting one without dates, editing a closed sprint's dates or issues —
is the workflow behaving as installed, not a fault to work around. Explain the
rejection and resolve its cause; never retry the write or strip the Play to get
past it.

## Automation

Closing a sprint records its `completed_date`. Completing every sub-task of a
task, story or bug marks the parent done.

## Where the details are

Narrower guidance covers each part — fetch the one matching the task:

- creating tasks, stories, bugs and epics, and linking work to an epic
- working with sprints: planning, starting and closing
- why a change to a sprint was rejected

Plays and schemas are ordinary nodes the user may have edited since install.
When the exact behavior matters, read the Play or schema itself rather than
trusting this summary.
