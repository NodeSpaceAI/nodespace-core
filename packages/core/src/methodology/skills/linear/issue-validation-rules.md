---
title: "Issue Validation Rules"
description: "Why a status change on an issue was rejected: the sub-issue completion gate and the blocker gate, what each checks, and how to proceed when one fires."
---
# Issue Validation Rules

Two rules can reject a status change outright. A rejection is the system
working as configured — not a bug, and not something to retry unchanged.

Both run synchronously, inside the transaction of the write they are checking,
so a rejected change never partially lands.

## Cannot close with open sub-issues

Setting `status` to `done` is rejected while any child issue is not `done` or
`cancelled`.

To proceed, either close or cancel the children, or move them out from under
this issue if they do not really belong to it.

## Cannot start with an open blocker

Setting `status` to `in_progress` is rejected while anything on `blocked_by` is
not `done` or `cancelled`.

To proceed, either finish the blocker, or remove the `blocks` edge if it no
longer applies. Note this gate is specific to starting work — an issue can sit
in `triage` or `backlog` behind a blocker quite legitimately.

## If a rejection looks wrong

Report it rather than working around it. Both rules are ordinary Plays the user
can inspect, edit or disable, and both were installed as part of the methodology
setup — so a rejection that seems incorrect is a question about their
configuration, not something to route around by, say, writing the status through
a different path.
