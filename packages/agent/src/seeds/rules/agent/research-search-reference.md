RESULT STRUCTURE: Each result contains:
- id: node ID (use this for follow-up get_node calls)
- title: document title
- score: similarity score (0-1, higher = more relevant)
- snippet: short content preview
- markdown: the document's text, cut off after 2,000 characters (present for the top results, as many as include_markdown asks for; default 1)
- properties: a record's field values (present beside markdown when the result is a record with fields set)

USE MARKDOWN DIRECTLY: A non-empty 'markdown' field is the document's own text, and 'properties' beside it holds a record's field values. A record's text is often just its name: a question about one of its fields is answered from 'properties'. Answer from the two — do NOT call get_node or search_nodes again for that result.

FETCH ADDITIONAL CONTENT: Call get_node with format=markdown when a result you need came back without markdown. It returns the text in 'markdown' and a record's field values in 'properties'.

MULTIPLE DOCUMENTS: If the user asks about multiple topics, call search_semantic once per topic rather than searching broadly and fetching each result individually.

LISTING BY TYPE: To list all nodes of a type, use search_nodes with an empty query. Examples:
- "list all tasks" → `search_nodes(query="", node_type="task")`
- "show me our ADRs" → `search_nodes(query="", node_type="<adr-schema-id>")`

STRUCTURED PROPERTY QUERIES: To filter by property values (status, due_date, etc.) or comparison operators (gt, lt, gte, lte, in), pass filters to search_nodes. Copy the type id and the field name exactly as they appear in EXISTING SCHEMAS, and use the exact enum member for a status filter — never a paraphrase. Examples:
- "which tickets are still in dev?" → `search_nodes(node_type="ticket", filters=[{"type":"property","operator":"equals","property":"status","value":"in_dev"}])`
- "tasks due tomorrow" → `search_nodes(node_type="task", filters=[{"type":"property","operator":"equals","property":"due_date","value":"<tomorrow's date in YYYY-MM-DD>"}], sorting=[{"field":"due_date","direction":"asc"}])`
- "tasks due this week" → `search_nodes(node_type="task", filters=[{"type":"property","operator":"gte","property":"due_date","value":"<today's date in YYYY-MM-DD>"},{"type":"property","operator":"lte","property":"due_date","value":"<end of week in YYYY-MM-DD>"}])`
- Date format: always YYYY-MM-DD. Operators: equals, contains, gt, lt, gte, lte, in, exists.
