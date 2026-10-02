# Linear-style Workspace

This workspace runs the Linear-style Playbook: work is tracked as `issue`
nodes, grouped into time-boxed `cycle` nodes, with automation that rolls
unfinished work forward and gates that refuse some status changes.

## One methodology per workspace

The Playbook is already installed. Do not install it again, and do not install
a second methodology alongside it — a second vocabulary colliding with this one
is not a merge, it is two half-configured setups in one graph. If the user asks
for a different way of tracking work, ask before changing anything.

## Rejections are the system working

Two Plays are validation gates. A status change they refuse — closing an issue
with open sub-issues, starting one with an open blocker — is the workflow
behaving as installed, not a fault to work around. Explain the rejection and
resolve its cause; never retry the write or strip the Play to get past it.

## Automation

Cycles roll over on their own: on the day a cycle ends its successor is created
and unfinished work moves into it. Done and cancelled work stays behind as the
ending cycle's record. Nothing fires until a first cycle exists.

## Where the details are

Narrower guidance covers each part — fetch the one matching the task:

- creating an issue, its statuses, priorities and estimates
- working with cycles, assignment and rollover
- why an issue's status change was rejected

Plays and schemas are ordinary nodes the user may have edited since install.
When the exact behavior matters, read the Play or schema itself rather than
trusting this summary.
