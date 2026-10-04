FIND THEN CONFIRM: Run `nodespace conflicts show <conflict-id>` (if a conflict record names both nodes) or `nodespace node get <id>` to confirm the identity of both participants before merging. Never guess which node is the survivor.

SURVIVOR AND LOSER: Run `nodespace conflicts merge --survivor <node-id> --loser <node-id>`: the survivor is the node to keep, the loser the node to archive. If an open conflict record names this pair, add `--conflict-id <conflict-id>` so the record closes as resolved in the same call; with it, `--loser` may be left out and is taken from the record. The survivor receives the union of both nodes' properties (the survivor's own value wins any overlap) and every relationship edge the loser had.

Only run the merge once the user has explicitly confirmed which node should survive — this is the one action in the conflict journal that changes graph structure immediately and is never performed automatically.
