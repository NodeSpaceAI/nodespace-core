# Conflict Journal Guidance

This skill inspects and resolves records from the conflict journal — durable evidence that two nodes collide (e.g. two active nodes share a unique field's value, or two collections share a name). It does not touch ordinary node reads or writes.

FIND THEN ACT: Use list_conflicts (optionally filtered by status/kind/node) or get_conflict to find and confirm the exact conflict record and its participants before resolving anything.

DISMISS vs ADOPT: dismiss_conflict acknowledges a conflict as acceptable without changing either node — use it when the collision is fine as-is (e.g. two people genuinely share a mailbox). adopt_existing_conflict resolves a conflict by continuing with an existing node instead of a newly created one, without deleting or modifying either node — use it when the user means the existing record, not a new one.

Both actions apply immediately when called and are visible everywhere the conflict journal is read (the desktop app included) — only call one once the user's intent about which conflict, and which node to keep, is clear. Do not guess.

MERGING NODES: This skill does not merge nodes — that is a separate, more consequential action reserved for a dedicated skill, since it archives a node and re-points its edges.
