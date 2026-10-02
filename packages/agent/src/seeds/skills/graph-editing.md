# Graph Editing Guidance

EXISTING RECORD OR A NEW ONE? This skill also carries create_node, and the two are not interchangeable. If the thing the user describes already exists — it was returned by an earlier tool call, or they are marking, correcting, or setting a value on it — call update_node with its id. If it does not exist yet — they are recording, logging, or adding something for the first time — call create_node instead: update_node needs an id, and inventing one writes to a record that is not theirs or fails outright.

When updating an existing node:

CALL update_node NOW: your next action is the tool call, not planning text.

FIND THEN UPDATE: <!-- include: find-then-act --> Then call update_node with the ID and only the fields that need changing.

ALREADY IN THIS CONVERSATION: an indirect reference like "the auth one", "that one", or "the 2400 one" can still name a record already returned by a prior tool result in this conversation — match the description against those records and use that record's id, matching on what the description says, not on which record was discussed last. Never ask the user to supply an id that's already in the conversation.

INDIRECT AND NOT YET FOUND: If the request identifies the target indirectly and no matching record has appeared in this conversation yet — a bare value without naming its field (an amount, a code), a relative date or status word (a weekday, "overdue", "recent"), or a paraphrased description — call resolve_query(request=<the request verbatim>, node_type) FIRST instead of hand-writing a search_nodes query yourself. resolve_query performs the search itself: if it returns resolved:true, act on the returned id directly (e.g. pass it straight to update_node) — do not call search_nodes afterward. If it returns resolved:false with reason:"no_match", tell the user nothing matched. If it returns reason:"multiple_matches", call route_clarify with one specific question naming the candidates as options — do not ask which one in prose.

AN ID ALONE CHANGES NOTHING: a call carrying only an id is a no-op that reports success — every call must also carry the change itself in `field_values`, using a field the type defines, copied character for character. When a field lists allowed values, use one of those values exactly — never a paraphrase of the user's wording, never a capitalised or spaced form of the value.

<!-- include: ambiguity-clarify -->

<!-- include: task-status-dedicated-verb -->

CONTENT vs FIELD VALUES: Use the content field only when the user is renaming the node. Use field_values for typed fields (status, due_date, etc.).

SUCCESS: <!-- include: success-no-reverify -->
