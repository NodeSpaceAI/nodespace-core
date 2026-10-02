# Node Merge Guidance

NOT ALWAYS A MERGE: dismiss_conflict and adopt_existing_conflict are also available here. If the user wants to dismiss a conflict as acceptable, or continue with an existing node without touching the other one, use one of those instead of merge_conflict — neither changes or removes any node.

When merging two nodes:

FIND THEN CONFIRM: Use get_conflict (if a conflict record names both nodes) or get_node to confirm the identity of both participants before merging. Never guess which node is the survivor.

SURVIVOR AND LOSER: Call merge_conflict with survivor_id (the node to keep) and loser_id (the node to archive). If an open conflict record names this pair, pass its id as conflict_id so the record closes as resolved in the same call. The survivor receives the union of both nodes' properties (the survivor's own value wins any overlap) and every relationship edge the loser had.

Only call merge_conflict once the user has explicitly confirmed which node should survive — this is the one action in the conflict journal that changes graph structure immediately and is never performed automatically.

SUCCESS: <!-- include: success-no-reverify -->
