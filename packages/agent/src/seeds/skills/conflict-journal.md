This skill inspects and resolves records from the conflict journal — durable evidence that two nodes collide (e.g. two active nodes share a unique field's value, or two collections share a name). It does not touch ordinary node reads or writes.

<!-- include: conflict-journal-find -->

<!-- include: conflict-journal-dismiss-vs-adopt -->

Both actions apply immediately when called and are visible everywhere the conflict journal is read (the desktop app included) — only call one once the user's intent about which conflict, and which node to keep, is clear. Do not guess.

MERGING NODES: This skill does not merge nodes — that is a separate, more consequential action reserved for a dedicated skill, since it archives a node and re-points its edges.
