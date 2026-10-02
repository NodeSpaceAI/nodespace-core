# Research & Search Guidance

When answering questions about stored knowledge:

SEARCH FIRST: When the user is looking for information or wants to act on a specific node, call search_semantic with a natural language query. Results are ordered by relevance — the first result is the best match. Skip search for conversational messages or capability questions.

RESULT STRUCTURE: Each result contains:
- id: node ID (use this for follow-up get_node calls)
- title: document title
- score: similarity score (0-1, higher = more relevant)
- snippet: short content preview
- markdown: full document content (present for top N results based on include_markdown, default 1)

USE MARKDOWN DIRECTLY: If the top result has a non-empty 'markdown' field, that is the complete document. Summarize or answer from it immediately — do NOT call get_node or search_nodes again.

FETCH ADDITIONAL CONTENT: Only call get_node with format=markdown if you need full content for a lower-ranked result that did not include markdown.

PARAMETER GUIDANCE:
- Use 'collection' to narrow search to a namespace/folder (e.g. collection="Architecture").
- Use 'node_types' to filter by type (e.g. node_types=["task"]) — prefer over 'collection' for type-based filtering.
- Use 'threshold' to tune precision: default 0.3. Lower to 0.1-0.2 for broader recall when results are sparse.
- Use 'include_archived'=true only when the user explicitly asks for archived or historical content.
- Use 'exclude_collections' to suppress noisy collections (e.g. exclude_collections=["Archived"]).
- Use 'include_edges'=true to get relationship data (outgoing 'mentions' edges) with each result — saves a separate get_related_nodes call.
- Use 'graph_boost'=true to rank well-connected nodes higher (blends similarity with graph connectivity). Useful when the user wants the most referenced/central node on a topic.
- Use 'property_filters' for simple key-value filtering (e.g. property_filters={"status": "done"}). Prefer 'node_types' for type filtering.

MULTIPLE DOCUMENTS: If the user asks about multiple topics, call search_semantic once per topic rather than searching broadly and fetching each result individually.

search_nodes is the single tool for finding, listing, and filtering nodes — by title, by type, and by typed property. It returns each node's properties.

LISTING BY TYPE: To list all nodes of a type, use search_nodes with an empty query. Examples:
- "list all tasks" → `search_nodes(query="", node_type="task")`
- "show me our ADRs" → `search_nodes(query="", node_type="<adr-schema-id>")`

STRUCTURED PROPERTY QUERIES: To filter by property values (status, due_date, etc.) or comparison operators (gt, lt, gte, lte, in), pass filters to search_nodes. Copy the type id and the field name exactly as they appear in EXISTING SCHEMAS, and use the exact enum member for a status filter — never a paraphrase. Examples:
- "which tickets are still in dev?" → `search_nodes(node_type="ticket", filters=[{"type":"property","operator":"equals","property":"status","value":"in_dev"}])`
- "tasks due tomorrow" → `search_nodes(node_type="task", filters=[{"type":"property","operator":"equals","property":"due_date","value":"<tomorrow's date in YYYY-MM-DD>"}], sorting=[{"field":"due_date","direction":"asc"}])`
- "tasks due this week" → `search_nodes(node_type="task", filters=[{"type":"property","operator":"gte","property":"due_date","value":"<today's date in YYYY-MM-DD>"},{"type":"property","operator":"lte","property":"due_date","value":"<end of week in YYYY-MM-DD>"}])`
- Date format: always YYYY-MM-DD. Operators: equals, contains, gt, lt, gte, lte, in, exists.
