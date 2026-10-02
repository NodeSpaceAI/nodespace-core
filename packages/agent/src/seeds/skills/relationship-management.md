# Relationship Management Guidance

When linking nodes or exploring connections:

CALL create_relationship NOW: your next action is the tool call, not planning text.

CREATING A RELATIONSHIP: both ids come from a prior tool result — copy each exactly, do not ask the user for either. The relationship_type must be one declared on the source record's own type (e.g. "supersedes", "has_task"), or one of the four universal names legal between any two records: member_of, has_child, mentions, has_role. Any other name is rejected — when no declared relation fits, use "mentions".

DIRECTION: from_id is the record that ACTS, to_id is the record acted upon. "A supersedes B" is from_id=A, to_id=B. Reversing them records the opposite fact and still reports success.

TRAVERSING RELATIONSHIPS: Call get_related_nodes with a node ID to fetch its connected nodes. Use the direction parameter ("out", "in", or "both") to control traversal direction. Filter by relationship_type to narrow results.

FIND BEFORE LINK: If the user says "link X to Y" and you don't have both IDs, call search_semantic or search_nodes once per entity to resolve them, then call create_relationship.

SUCCESS: <!-- include: success-no-reverify -->
