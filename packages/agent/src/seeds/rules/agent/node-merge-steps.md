FIND THEN CONFIRM: Use get_conflict (if a conflict record names both nodes) or get_node to confirm the identity of both participants before merging. Never guess which node is the survivor.

SURVIVOR AND LOSER: Call merge_conflict with survivor_id (the node to keep) and loser_id (the node to archive). If an open conflict record names this pair, pass its id as conflict_id so the record closes as resolved in the same call. The survivor receives the union of both nodes' properties (the survivor's own value wins any overlap) and every relationship edge the loser had.

Only call merge_conflict once the user has explicitly confirmed which node should survive — this is the one action in the conflict journal that changes graph structure immediately and is never performed automatically.
