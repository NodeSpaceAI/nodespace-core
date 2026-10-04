WHICH COMMAND:
- A record you know by name (a task, a company, any typed record): `nodespace node query --title-contains "<name>"`. It matches the title exactly. `nodespace search` also returns near-misses by meaning, so it cannot tell you a record is absent.
- Documents and records by topic: `nodespace search "<query>"`. It matches on meaning and on title keywords, and returns whole documents and records, never a line from inside one.
- Every node of a type: `nodespace search "" --type <type>`.
- By property value or comparison (status, due_date, priority): `nodespace query --type <type> --filters '<json>'`.
- One node by id: `nodespace node get <id>`. A document's whole subtree as markdown: `nodespace node export <id>`.

NARROWING A SEARCH:
- `--type <type>` filters by node type, and is repeatable.
- `--collection <path>` narrows to a collection (e.g. `--collection Architecture`).
- `--limit <n>` caps the results (default 20).
- `--threshold <0.0-1.0>` tunes precision. Lower it to 0.1-0.2 for broader recall when results are sparse.

MULTIPLE TOPICS: If the user asks about several topics, run `nodespace search` once per topic rather than one broad search plus a fetch per result.

STRUCTURED PROPERTY QUERIES: Copy the type id and the field names exactly as `nodespace schema get <type>` shows them, and use the exact enum value for a status filter — never a paraphrase. Examples:
- "which tickets are still in dev?" → `nodespace query --type ticket --filters '[{"type":"property","operator":"equals","property":"status","value":"in_dev"}]'`
- "tasks due tomorrow" → `nodespace query --type task --filters '[{"type":"property","operator":"equals","property":"due_date","value":"<tomorrow's date in YYYY-MM-DD>"}]' --sorting '[{"field":"due_date","direction":"asc"}]'`
- "tasks due this week" → `nodespace query --type task --filters '[{"type":"property","operator":"gte","property":"due_date","value":"<today's date in YYYY-MM-DD>"},{"type":"property","operator":"lte","property":"due_date","value":"<end of week in YYYY-MM-DD>"}]'`
- Date format: always YYYY-MM-DD. Operators: equals, contains, gt, lt, gte, lte, in, exists.
