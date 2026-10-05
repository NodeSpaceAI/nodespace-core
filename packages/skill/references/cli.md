# NodeSpace CLI Reference

Complete command reference for the `nodespace` CLI. Loaded on demand — read this
when you need exact flags, argument shapes, or output formats.

The "Complete command surface" section near the end is generated from the CLI's
own definitions and is exhaustive; the worked examples before it carry the
judgment calls that a generator cannot produce.

## CLI Reference

All commands accept `--json` for machine-readable output.

**Node JSON shape.** Every command that emits a node returns objects of this
shape; list-returning commands wrap them as `{"count": N, "nodes": [...]}`.
Every key shown below is present on every node, so parse against these names
and nothing else. `relationship get` may additionally include `title`,
`mentions` and `mentioned_in`.

```json
{
  "id": "550e8400-e29b-41d4-a716-446655440000",
  "node_type": "task",
  "content": "Buy groceries",
  "properties": { "status": "open", "priority": "high" },
  "version": 1,
  "lifecycle_status": "active",
  "created_at": "2026-01-01T00:00:00Z",
  "modified_at": "2026-01-01T00:00:00Z"
}
```

`properties` is a **flat** object keyed by the field names the type's schema
defines — `jq '.properties.status'` reads a value directly, with no intermediate
key and no second parse. These are the same bare names that
`node update --property status=done` and `query --filters '{"property":"status"}'`
accept, so what you parse and what you type always match. Internal bookkeeping
keys are not part of the output. A node with no properties set returns `{}`.
Values of any JSON type round-trip, nested objects included — `--property
address='{"city":"Berlin"}'` reads back as `.properties.address.city`.

**Schema JSON shape.** `schema get` and `schema list` return schemas rather
than nodes; `schema list` wraps them as `{"count": N, "schemas": [...]}`. A
schema is read under the keys `schema create` and `schema update` write it
with: `fields`, `relationships`, `extends`, `abstract`, `children`, `parent`,
`title_template` and `properties_header_summary_template`, next to `id`,
`content` (the type's name), `is_core` and `schema_version`. There is no
`properties` key. `extends`, `abstract`, `children`, `parent` and the two
templates are omitted when the type doesn't declare them. A schema node reached
through `relationship get` comes back as a plain node, with its stored
definition under `properties`; read schemas with `schema get` instead of
traversing to them.

`schema create --json` returns what it created under the same names: `id`,
`is_core`, `schema_version`, `fields` and `relationships`, plus the
`description` it wrote, `extends` when the type has a base, and `warnings` when
there are any. `schema update --json` returns `id` and `success`, a count for
each kind of change it made (`fields_added`, `fields_removed`,
`fields_renamed`, `field_values_added`, `relationships_added`,
`relationships_removed`) and `affected_plays` when a forced update touched
any. Without `--json` both print a short summary instead.

**Two ways to set properties**, on `node create` and `node update` alike:

- `--property key=value`, repeatable, one property each. The value is parsed as
  JSON when it can be, so `estimate=3` is the *number* 3 and `done=true` a
  boolean. A text or enum field whose value looks like a number needs the JSON
  quotes kept through the shell: `--property 'estimate="3"'`. Without them the
  write is rejected as the wrong type.
- `--properties '{"key":"value"}'`, one JSON object carrying several properties.
  Every value keeps the type it is written with (`{"estimate":"3"}` is a
  string), which makes it the simpler form for nested values and for anything
  copied from JSON output.

Both may be given together; `--property` wins for a key set both ways.


**Selecting a database.** A single daemon can serve several local databases. The data commands that read or write a database (`node`, `query`, `search`, `mention`, `schema`, `relationship`, `import`, `diagnostics`) accept a global `--database <name|id>` flag that routes the request to a specific database; the `NODESPACE_DATABASE` environment variable sets the same target when the flag is absent. Without either, requests go to the daemon's default database. Model management (`nodespace model`) is daemon-global — the loaded inference model is shared across all databases, so the flag is accepted but has no effect there. Manage the set of databases with the `nodespace database` subcommands (below).

```bash
nodespace --database work node create --type text --content "work note"
NODESPACE_DATABASE=work nodespace search "meeting notes"
```

### Create a node

```bash
nodespace node create --type text --content "Your content here"
nodespace node create --type task --content "Buy groceries" --parent <parent-id>
nodespace node create --type text --content "Meeting notes" --parent <parent-id>
```

**Options:**
- `--type <type>` — node type: `text`, `task`, `date`, or any schema-defined type
- `--content <text>` — the text content of the node
- `--parent <id>` — optional parent node ID (creates a child node)
- `--property key=value` / `--properties '{json}'` — set properties (see *Two ways to set properties* above)

**Output:** the created node, in the node JSON shape above. It carries no parent id: the node is under the `--parent` you gave, or is a root node when you gave none.

**Creating an instance of a custom type:** read the schema first (`nodespace schema get <type>`) so you know its fields. Use the field name exactly as it appears in the schema's `fields[].name` — do not add namespace prefixes when setting properties on instances (prefixes like `custom:` are part of a *field's name* at schema-authoring time, not something a caller adds — see Schema fields below). If the schema has a `title_template`, `--content` only needs a brief descriptive label — the display title is generated from properties. If there's no `title_template`, set `--content` to the best human-readable name available.

Only include properties the schema actually defines as required, plus any optional ones the user gave a value for. Don't invent fields.

**Describing a collection:** a collection has one optional field, `description`, which says what the collection is for. Agents see it next to the collection's name when they decide where a node belongs. Set it on create, or later with `node update`:

```bash
nodespace node create --type collection --content "Clients" --property description="Accounts we bill, one page per client"
nodespace node update <collection-id> --property description="Accounts we bill, one page per client"
```

A collection is identified by its name (case-insensitive): `node create --type collection` with a name that is taken fails with `Already exists`, naming the collection in the way and its id. Update that collection instead, using the id from the error.

**Success semantics:** once `node create` returns an ID, the node exists — confirm what was created to the user and stop. Don't immediately `node get` the same ID to verify; the create response is the confirmation.

**If it has headings, it is a tree, not a node.** NodeSpace is one-node-per-block, with hierarchy as first-class edges — a document with sections (an ADR, a spec, a plan) decomposes into a root node plus one child per section, not one node whose `content` holds the whole document:

```bash
# Multi-call: root, then one child per section
nodespace node create --type text --content "# ADR-071: Use collections for tagging"
# → returns {"id": "root-id", ...}
nodespace node create --type text --content "## Context\n..." --parent root-id
nodespace node create --type text --content "## Decision\n..." --parent root-id

# Or write it to a file and import it — see `nodespace import` below. This is
# the normal path for any multi-section content, not only bulk migration,
# including a single document you just finished authoring.
nodespace import file ./adr-071.md
```

### Get a node

```bash
nodespace node get <node-id>
```

**Output:** Full node JSON including all properties

### Update a node

```bash
nodespace node update <node-id> --content "Updated content"
nodespace node update <node-id> --property status=in_progress --property priority=high
```

**Options:**
- `--content <text>` — replaces the node's content/title. Omit to leave content unchanged.
- `--property key=value` — repeatable; sets one property, deep-merged into existing properties (properties you don't mention are left untouched). Values are parsed as JSON when possible (numbers, booleans, arrays, objects), otherwise treated as a plain string.
- `--properties '{json}'` — several properties as one JSON object, deep-merged the same way; each value keeps the type it is written with.
- `--version <n>` — the node's `version` as you read it. The update is written only if the node is still at that version. Omit it to update whatever is current.

At least one of `--content`, `--property`, `--properties` or a collection flag is required.

**Writing at the version you read:** every node has a `version`, printed by `node get` and by every write. Pass it back with `--version` when the change depends on what you read: starting a task, ticking a checklist item, editing text you just fetched. If someone else changed the node in between, nothing is written and the command exits non-zero:

```
Node <node-id> has changed since it was read: version 3 was given and it is now at version 4. Nothing was written. Read the node again before deciding what to do.
```

With `--json` the same is printed as `{"error": "version_conflict", "node_id": …, "given_version": 3, "current_version": 4, "message": …}`.

**After a conflict, read the node again (`nodespace node get <node-id>`) before you do anything else.** Do not retry with the new version number: the node now holds a change you have not seen, and your write may no longer be right. A task you meant to start may already be in progress under another session, in which case you leave it and pick other work.

Joining or leaving a collection does not change a node's version. `--version` on an update that only changes collections is still checked against the node, but it does not stop a second session making the same change: claim work with `set-status`, not with a collection.

**A derived attribute cannot be written.** A checkbox's `checked` is computed from its content (`- [ ] ` / `- [x] `) and is never a property: tick or untick one with `--content`, e.g. `nodespace node update <checkbox-id> --content "- [x] Tests pass"`. `--property checked=true` is refused.

**Find then update:** if you don't already have the node's ID, locate it first — by name with `nodespace node query --title-contains "<name>"` (an exact match; `nodespace search` also finds names but mixes in documents that are only similar in meaning), or by topic with `nodespace search` — then update by ID. If the lookup comes back with zero matches or several equally plausible matches, ask the user one specific clarifying question rather than retrying — e.g. "I found 3 tickets in review — which one did you mean: the auth one, the CI one, or the audit-log one?"

**Do NOT use `node update --property status=...` for task status changes** — use `nodespace node set-status` instead (below); it validates against the allowed status values before writing.

**Output:** Updated node JSON. Confirm the change to the user from this response — don't re-fetch the node afterward to double-check.

### Set a task's status

```bash
nodespace node set-status <task-id> in_progress
nodespace node set-status <task-id> in_progress --version <n>   # only if the task is still at the version you read
```

Dedicated verb for task status transitions. Status must be one of the values the `task` schema's `status` field declares — the four built-ins (`open`, `in_progress`, `done`, `cancelled`) plus any added via `schema update`'s `add_field_values` (see "Adding a value to an existing enum" under Schema inspection and management). Validated against that live vocabulary; an invalid value is rejected with the current list.

`--version <n>` works as it does for `node update` (see "Writing at the version you read" above). Start a task with the version you read it at: of two sessions that both try, one is refused, and it reads the task again and moves on.

**Output:** Updated node JSON.

### Move a node

```bash
nodespace node move <node-id> --parent <parent-id>                     # under another parent, placed last
nodespace node move <node-id> --parent <parent-id> --first             # placed first
nodespace node move <node-id> --parent <parent-id> --after <sibling-id>
nodespace node move <node-id> --root                                   # no parent
nodespace node move <node-id> --first                                  # same parent, new position
nodespace node move <node-id> --after <sibling-id>
```

The node keeps its ID and everything nested under it; never recreate a node and delete the original to relocate it.

**Options:**
- `--parent <id>` or `--root` — where the node goes. Give neither to keep the current parent and change only the position. `--root` takes no position: root nodes have no order.
- `--first` or `--after <sibling-id>` — the position among the siblings. `--after` names a child of the parent the node ends up under; any other node is refused and nothing is written. To place a node before a sibling, name the sibling ahead of that one with `--after`, or use `--first` when there is none (`nodespace node children <parent-id>` lists them in order). With a new parent and no position, the node is placed last.
- `--version <n>` — works as it does for `node update` (see "Writing at the version you read" above).

At least one of `--parent`, `--root`, `--first` or `--after` is required. A root node has no siblings to be ordered among, so a position without `--parent` is refused for one.

**This is the only way to change a node's parent.** A node has one parent, so `relationship create --type has_child` to a node that already has one is refused, and the message gives the `node move` command to run instead.

**Output:** The moved node's JSON, at its new version.

### Delete a node

```bash
nodespace node delete <node-id>                                  # step 1: preview, deletes nothing
nodespace node delete <node-id> --version <v> --descendants <n>  # step 2: delete exactly what was previewed
```

**Two steps, always:** the bare form only previews — it names the node (title, type, version) and how many nested nodes go with it, and prints the step-2 command as `confirm_command`. Show the user the preview, and run `confirm_command` unchanged only after they say yes. Step 2 is refused, deleting nothing, if the node was edited or anything was added or removed beneath it since the preview; preview again and re-confirm rather than adjusting the numbers yourself.

**Find then delete:** locate the node via `nodespace node query --title-contains` (or `nodespace search` for notes and documents) if you don't have its ID. Delete one node per call; confirm each deletion before moving to the next. Don't search again afterward to verify the deletion — the delete response confirms it.

**Output:** Step 1 — `{"node_id", "existed", "deleted": false, "title", "node_type", "version", "descendant_count", "confirm_command"}`. Step 2 — `{"node_id", "existed", "deleted_count"}`, where `deleted_count` includes the node itself.

### List children

```bash
nodespace node children <parent-id>
```

**Output:** JSON array of child nodes

### Query nodes (exact-match filter)

```bash
nodespace node query --type task
nodespace node query --content-contains "authentication" --limit 10
nodespace node query --title-contains "Ticket" --type text
nodespace node query --mentioned-by <node-id>
```

**Options:**
- `--type <type>` — filter by node type
- `--content-contains <text>` — substring match in content
- `--title-contains <text>` — substring match in title
- `--mentioned-by <id>` — nodes mentioned by the given node
- `--limit <n>` — max results
- `--offset <n>` — pagination offset

**Note:** for property-level filtering (status, due_date, priority, etc.) or comparison operators, use `nodespace query` (below) — this command only does exact substring/type matching.

**Output:** JSON array of matching nodes

### Structured property query

```bash
nodespace query --type task --filters '[{"type":"property","operator":"equals","property":"status","value":"open"}]'
nodespace query --type task --filters '[{"type":"property","operator":"gte","property":"due_date","value":"2026-07-11"}]' --sorting '[{"field":"due_date","direction":"asc"}]' --limit 20
```

**Options:**
- `--type <type>` — target node type, or `*` for all types
- `--filters <json>` — array of filter conditions: `{"type":"property"|"content"|"metadata"|"relationship"|"related","operator":"equals"|"contains"|"gt"|"lt"|"gte"|"lte"|"in"|"exists","property":"...","value":...}`
  - A `relationship` filter selects the nodes connected to one node: `{"type":"relationship","operator":"equals","path":["child_of"],"node_id":"<id>"}` is the children of `<id>` (each matching node reaches `<id>` by following `child_of`).
  - A `related` filter selects by a condition on the connected nodes: `{"type":"related","operator":"equals","path":["project"],"filter":{"type":"property","operator":"equals","property":"status","value":"active"}}` is the tasks whose project is active.
  - `path` lists the relationship names to follow from each candidate node, in order: built-in names (`has_child`, `member_of`, `mentions`), schema-declared names, or the reverse name of either (`child_of`, `mentioned_by`, a declared `reverseName`). `{"name":"child_of","open_ended":true}` in place of a name follows it to every depth (all ancestors). A name the type does not declare is an error naming the ones it does. With `--type '*'` only built-in names resolve.
  - Any filter takes `"negate": true` to keep the nodes it does **not** hold for: `{"type":"property","operator":"equals","property":"status","value":"done","negate":true}` is every task whose status is not `done`, a task with no status included, and a negated `exists` is "has no value". A negated `related` filter is "the path reaches no node matching the nested filter", which a node the path leads nowhere from satisfies; the nested `filter` can be negated too, and the two together say "every node the path reaches matches". Filters are ANDed; there is no OR.
  - A `property` filter may name a field inside an object field's value with a dotted path: `"property":"repository.url"` is the `url` inside `repository`. A sort's `field` takes the same. The schema must declare every segment, so the query needs a `--type`; a path it does not declare is an error naming it.
  - A `property` filter on a date field takes `relative_date` in place of `value`, for a date relative to the day the query runs: `{"type":"property","operator":"gte","property":"due_date","relative_date":{"anchor":"today"}}` is due today or later, and `"relative_date":{"anchor":"today","offset_days":7}` is a week from today (negative for the past). The operator is one of `equals`, `gt`, `lt`, `gte`, `lte`. Today is the local date. It works inside a `related` filter's nested `filter` too. In a saved query it is stored as written and resolved each time the query runs, so prefer it to a fixed date when saving a view such as "due this week". A play's selector does not accept it.
- `--sorting <json>` — array of `{"field":"...","direction":"asc"|"desc"}`
- `--limit <n>` — max results (0 = server default of 50; server caps at 500 regardless of the value passed)

Worked examples:
- "the tasks of this project" → `nodespace query --type task --filters '[{"type":"relationship","operator":"equals","path":["project"],"node_id":"<project-id>"}]'`
- "find all my open tasks" → `nodespace query --type task --filters '[{"type":"property","operator":"equals","property":"status","value":"open"}]'`
- "tasks due tomorrow" → `nodespace query --type task --filters '[{"type":"property","operator":"equals","property":"due_date","value":"<YYYY-MM-DD>"}]' --sorting '[{"field":"due_date","direction":"asc"}]'`
- "tasks due this week" → `nodespace query --type task --filters '[{"type":"property","operator":"gte","property":"due_date","value":"<week start>"},{"type":"property","operator":"lte","property":"due_date","value":"<week end>"}]'`
- "overdue tasks" → `nodespace query --type task --filters '[{"type":"property","operator":"lt","property":"due_date","relative_date":{"anchor":"today"}}]'`
- "issues in the current cycle" → `nodespace query --type issue --filters '[{"type":"related","operator":"exists","path":["cycle"],"filter":{"type":"property","operator":"lte","property":"start_date","relative_date":{"anchor":"today"}}},{"type":"related","operator":"exists","path":["cycle"],"filter":{"type":"property","operator":"gte","property":"end_date","relative_date":{"anchor":"today"}}}]'`
- "high priority tasks" → `nodespace query --type task --filters '[{"type":"property","operator":"equals","property":"priority","value":"high"}]'`
- "tasks that are not done" → `nodespace query --type task --filters '[{"type":"property","operator":"equals","property":"status","value":"done","negate":true}]'`
- "tasks with no unfinished blocker" → `nodespace query --type task --filters '[{"type":"related","operator":"exists","path":["blocked_by"],"negate":true,"filter":{"type":"property","operator":"in","property":"status","value":["done","cancelled"],"negate":true}}]'`
- "the project for this repository" → `nodespace query --type project --filters '[{"type":"property","operator":"equals","property":"repository.url","value":"<remote url>"}]'` (when the `project` schema declares a `repository` object field with a `url` inside it)
- "tasks with an unchecked item" → `nodespace query --type task --filters '[{"type":"related","operator":"equals","path":["has_child"],"filter":{"type":"property","operator":"equals","property":"checked","value":false}}]'`. `checked` is a checkbox's derived attribute: computed from its content, named in a `property` filter like a field, and never matched by a node that is not a checkbox.

Date format for all date properties: **YYYY-MM-DD**.

This is the CLI counterpart of the property-filtering path of the local agent's `search_nodes` tool.

**Output:** JSON array of matching nodes

### Run a saved query

A saved query is a `query` node: a view, or a queue of work such as "Ready tasks", that someone defined once. Run it by its id or its title instead of copying its filters:

```bash
nodespace query run "Ready tasks"
nodespace query run <query-id>
nodespace --json query run "Ready tasks" --filters '[{"type":"relationship","operator":"equals","path":["project"],"node_id":"<project-id>"}]' --limit 1
```

It returns the nodes the query matches now, with its stored filters, sorting, limit and relative dates.

**Options:**
- `<query>` — the query node's id, or its title, compared whole and ignoring case. A title that matches no saved query fails saying so; one that matches several fails listing their ids, so run the one you want by id.
- `--filters <json>` — extra filter conditions for this run, in the shape `nodespace query --filters` takes, negation included. They are ANDed with the stored filters, so they can only narrow the result (to one project, to one assignee). The saved query is not changed.
- `--limit <n>` — at most this many results. It can lower the query's own limit, never raise it (0 = the stored limit). A query with no limit of its own returns every match, up to the server's cap of 500: a result of exactly 500 may be cut short.

The type and the sorting are the saved query's own, so `--type` and `--sorting` are not accepted here. To find the saved queries: `nodespace query --type query`.

This is the CLI counterpart of the local agent's `run_query` tool.

**Output:** JSON array of matching nodes

### Export node as markdown

```bash
nodespace node export <node-id>
nodespace node export <node-id> --children false   # root node only
nodespace node export <node-id> --max-depth 3
nodespace node export <node-id> --node-ids false   # clean markdown without OCC comments
```

**Options:**
- `--children` — include children recursively (default: true)
- `--max-depth <n>` — maximum recursion depth (default: 20)
- `--node-ids` — embed `<!-- id v<version> -->` OCC comments (default: true)

**Output:** Markdown string (human mode) or `{"markdown":"…","node_count":N}` (JSON mode)

### Batch fetch nodes

```bash
nodespace node batch-get --id <id1> --id <id2> --id <id3>
```

**Output:** `{"count":N,"nodes":[…],"not_found":["id-that-was-missing"]}`

### Batch update nodes (OCC-aware)

```bash
nodespace node batch-update --updates '[{"node_id":"abc","content":"new text","version":3}]'
```

Each object in the array: `node_id` (required), `version` (optional — omit to auto-fetch, note this bypasses OCC), `content`, `node_type`, `properties`.

**Output:** `{"count":N,"updated":["id1",…],"failed":[{"node_id":"…","error":"…"}]}`

### Semantic search

```bash
nodespace search "meeting notes from last week"
nodespace search "rust async" --type text --limit 10
nodespace search "" --type task    # list all nodes of a type (empty query)
nodespace search "" --collection "docs:rust"   # list a collection's members
```

An empty query (or `"*"`) is a listing, not a search: the most recently modified nodes first, narrowed by `--type`, `--collection` and `--filters`. It leaves out the body paragraphs of skills, agent guidance and other system nodes, so `--type text` lists the user's own text.

**Options:**
- `--type <type>` — filter by node type (repeatable)
- `--collection <path>` / `--collection-id <id>` — narrow to a collection (mutually exclusive)
- `--filters <json>` — array of `{field, operator, value}` filter objects
- `--threshold <0.0-1.0>` — similarity cutoff (0.0 = server default of 0.7); lower it (e.g. 0.1-0.2) for broader recall when results are sparse
- `--limit <n>` — max results (default: 20)

Matches on meaning and on title keywords, so tasks, date pages and typed records are found by name. Results are whole documents and records, never a line from inside one.

A search that names no `--type` leaves out system types such as `skill`. `--type skill` returns them, but to find the skill for a task use `nodespace skill guidance "<task>"`: it ranks every skill, with no similarity cutoff, and returns each one's instructions.

**Answering a question about what's stored** (how something works, what it is, why it was decided): search before you answer or ask the user for context, with `--include-content` — a heading is not the document. Answer from every hit that bears on the question, not the first alone; when two disagree, or one is marked superseded or archived, say so and prefer the current one. Reading is part of answering — do it rather than offering to.

**Output:** JSON array of matching nodes

### Import markdown files

```bash
nodespace import file ./notes.md --collection "docs:rust"
nodespace import dir ./docs
nodespace import dir ./docs ./adr ./specs --auto-collection-routing   # several directories, one call
nodespace import dir ./docs --replace                                # re-import in place
```

Import is a repeatable sync, not a one-off bulk load: re-running `import dir`/`import file` without `--replace` skips a document already imported (no duplicates), and `--replace` refreshes its child subtree from the fresh parse while keeping the root node — so anything that already links to it stays valid. A first import and a refresh are the same command, just with or without `--replace`.

`import dir` accepts more than one directory in a single call — one process, one summary, instead of a shell loop calling it once per directory. `--auto-collection-routing` still routes each file relative to *its own* directory's root, never a synthesised common ancestor across the directories passed, so two unrelated trees can't produce surprising collection paths. Per-directory failures (a bad path, an unreadable folder) are reported in the combined results rather than aborting the rest. Passing exactly one directory behaves exactly as `import dir <dir>` always has.

`--auto-collection-routing` turns directory structure into collection membership for free — the folder layout becomes the collection hierarchy, no separate tagging step. Combined with `--replace`, markdown-on-disk becomes a viable source of truth: import once to build the tree, re-import to keep it current.

**Options (`import dir`):** `--collection <path>`, `--use-filename-as-title`, `--auto-collection-routing`, `--exclude <pattern>` (repeatable), `--include-agent-files`, `--include-hidden`, `--no-recursive`, `--replace` — see the flag list below for exact semantics.

**Output:** streamed progress events (human mode: `[step/9] name: message` on stderr, plus a pass/fail line per file); JSON mode prints one combined array of `{file_path, root_id, nodes_created, success, error, collection, archived}` once the call completes.

### Mention relationships

```bash
# Create a mention link
nodespace mention create --from <source-id> --to <target-id>

# Delete a mention link
nodespace mention delete --from <source-id> --to <target-id>

# List nodes this node mentions
nodespace mention outgoing <node-id>

# List nodes that mention this node
nodespace mention incoming <node-id>
```

Mentions are inline references captured from markdown content — distinct from schema-defined relationships (below), which are named, typed edges.

### Typed relationships

```bash
# Create a relationship edge (a name declared on the source node's schema, or a built-in one)
nodespace relationship create --from <source-id> --type has_task --to <target-id>
nodespace relationship create --from <source-id> --type billed_to --to <target-id> --edge-data '{"note":"..."}'

# Traverse relationships from a node
nodespace relationship get <node-id> --type has_task --direction out
nodespace relationship get <node-id> --type billed_to --direction in

# Traverse the reverse direction by the schema's declared reverseName
# (adr declares: name decided_by, targetType person, reverseName decisions, reverseCardinality many)
nodespace relationship get <person-id> --type decisions
nodespace relationship get <person-id> --type decided_by --direction in   # equivalent
nodespace relationship get <person-id> --type decisions --direction in    # also equivalent — see below
```

**Options (`create`):**
- `--from <id>` — source node ID
- `--type <name>` — relationship name: one declared on the source node's schema (e.g. `has_task`, `billed_to`), or one of the built-in names below — not an arbitrary label
- `--to <id>` — target node ID
- `--edge-data <json>` — optional JSON-encoded edge properties

**Options (`get`):**
- `<id>` — node ID to query relationships for
- `--type <name>` — relationship name to traverse: the forward `name` from the source's end, or the declared `reverseName` from the target's end
- `--direction <out|in>` — traversal direction (default: `out`), relative to the name given. **Ignored when `--type` is a `reverseName`** (or a built-in's fixed inverse, e.g. `child_of`) — see below

<!-- BEGIN GENERATED: relationship-rules (see packages/agent/src/seeds/rules/skill-md/relationship-direction.md, packages/agent/src/seeds/rules/skill-md/relationship-reverse-traversal.md) -->
**Direction.** `--from` is the record that ACTS, `--to` is the record acted upon. "A supersedes B" is `--from <A> --to <B>`. Reversing them records the opposite fact and still reports success.

**Traversing the reverse direction.** A relationship is declared once, on the source type, but reads from both ends. Given `{"name":"decided_by","targetType":"person","direction":"out","cardinality":"one","reverseName":"decisions","reverseCardinality":"many"}` on `adr`: from the ADR, `nodespace relationship get <adr-id> --type decided_by --direction out`; from the person, use the declared `reverseName` — `nodespace relationship get <person-id> --type decisions` — or the equivalent `--type decided_by --direction in`. Both spellings return the same ADRs, and the output line's arrow shows the direction actually traversed (`<--decided_by--` for an inbound resolution). An empty result means no edges exist, not that reverse traversal is unsupported. A name declared in neither direction is rejected with an error naming the spellings that do work — read it and retry rather than concluding the capability is missing.

A `reverseName` (or a built-in's fixed inverse, like `child_of`) names exactly one traversal — the forward relationship, read from the target end — so `--direction` has nothing to select once `--type` already resolved to one: `--type decisions --direction in` runs the identical query as `--type decisions` with no flag at all, not a second, further-reversed one. Pairing `--direction` with the forward name is where direction still does something (`--type decided_by --direction in` vs. `--direction out`, from the person and the ADR respectively).
<!-- END GENERATED: relationship-rules -->

Reverse names are for *traversal*, not for `relationship create`: an edge is always created under its forward name, from the source node. A `relationship` or `related` filter's `path` in `query --filters` takes them like any other relationship name.

Both node IDs must already exist — look up missing IDs first (`nodespace node query --title-contains` by name; `nodespace search` for notes and documents). Apart from the built-in names below, the relationship name must be defined on the source node's schema; define it there (`nodespace schema create`/`update`) if it isn't yet. `relationship create` on a node whose schema doesn't define that relationship name fails with an error naming the undefined relationship.

### Play automation rule-sets

A Play (`trigger → conditions → actions`) is a `node_type: "play"` node:

```bash
nodespace playbook list                     # every Play, its state (on, off or suspended) and its lifecycle
nodespace playbook disable <play-id>        # switch a Play off: it stops running and stays in the list
nodespace playbook enable <play-id>         # switch it on, and clear a suspension
```

A Play's switch is its `enabled` field (default `true`). `playbook enable` and `playbook disable` write it, and so does `nodespace node update <play-id> --property enabled=false`. Only you or the user change it: the engine never does. Switching a Play off always works, even when its rules no longer validate.

A Play runs when it is `enabled`, not suspended, not archived, and its rules validate. `playbook list` reports each Play's `state`:

- **on** — it runs.
- **off** — `enabled` is `false`.
- **suspended** — the engine took the Play out of service on this device: its rules failed validation (`validation_failed`), an action failed (`action_failed`), a chain of rules hit the cycle limit (`cycle_limit`), or a schema change broke its rules (`schema_drift`). The Play's `suspended_reason`, `suspended_message` and `suspended_at` say why and when. The engine writes these three fields; a write that changes one is refused, and a new Play can't be created with one set. `enabled` stays as it was.

To bring a suspended Play back, fix the cause, then run `playbook enable <play-id>` (it clears the suspension even when the Play is already enabled) or save the corrected `rules`. The engine checks the Play again and suspends it again if the problem remains.

<!-- BEGIN GENERATED: play-rules (see packages/agent/src/seeds/rules/skill-md/, packages/agent/src/seeds/skill-md/play-rules.md) -->
**Play rule descriptions:** a Play's rule, each of its conditions and each of its actions carry a required `description`: one plain sentence saying what that part means, written in the same write as the part. A condition is an object with `expr` and `description`, never a bare expression. An action carries `description` beside `action_type`, `params` and `for_each`. The trigger takes no description. A missing or blank description is rejected. Write a Play's rules whole, with `nodespace node update <play-id> --property 'rules=[...]'`; `nodespace node get <play-id>` and `playbook list --json` print them in the same shape:

```json
{"name": "complete parent", "description": "Mark a task done once all its sub-tasks are done", "trigger": {"type": "graph_event", "on": "property_changed", "select": {"target_type": "task"}, "property_key": "task.status"}, "conditions": [{"expr": "node.child_of.has_child.all(c, c.status == 'done')", "description": "Every sub-task of the parent is done"}], "actions": [{"action_type": "update_node", "description": "Mark the parent task done", "params": {"node_id": "{trigger.node.child_of.id}", "properties": {"status": "done"}}}]}
```

**Stale Play descriptions:** when you change part of a Play's rules, rewrite that part's `description` in the same write. A changed condition `expr`, a changed action (`action_type`, `params` or `for_each`), or a changed rule `trigger` or `class` that keeps its stored description is rejected. A rule is matched to the stored rule with the same `name`, and its conditions and actions by position; a renamed rule is a new rule. The error names the rule, the part and its number, e.g. "rule `complete parent`, condition 2: its expression changed and its description didn't". Rewrite that description and run the update again with the corrected payload. A write that leaves `rules` alone is not checked, so `playbook disable` always works.
<!-- END GENERATED: play-rules -->

Archiving is not a Play's switch. An archived node takes part in nothing, so an archived Play doesn't run whatever its `state` says, no rule fires on an archived node and no action touches one; `playbook list --include-archived` shows archived Plays too, with their lifecycle. A Play's conditions and actions can't read or set whether a node is archived.

A Play's execution errors are **not** in the graph: the graph holds only the suspension above. Engine diagnostics (a failed
action, a cycle-limit breach, a rule that would not compile) are operational
telemetry rather than knowledge, so they go to the daemon log rather than
becoming nodes — there is nothing to query for them. Read them with
`nodespace logs`, which resolves the log path for you (it differs between a
desktop-app install and a Homebrew service):

```bash
nodespace logs --filter <play-id>
nodespace logs --filter <play-id> --lines 200
nodespace logs --path-only                    # just print where the log lives
```

With `NODESPACE_HOME` set, the log is the one under that home (`$NODESPACE_HOME/.nodespace/logs/nodespaced.log`) and no other install's log is consulted.

`get-workflow-state` is the one purpose-built verb — it runs the engine's condition evaluation out of band from a live trigger, which a generic verb cannot do:

```bash
nodespace playbook get-workflow-state <node-id>
```

Evaluates every active Play rule whose trigger could apply to the node's type against its current state, and reports each condition's state:
- **satisfied** — the condition evaluated true right now.
- **not_yet_met** — the condition references a real, schema-declared field or relationship that simply doesn't have a value yet. Normal; the Play stays active waiting for it.
- **unresolvable** — the condition references something that is neither a declared field nor a declared relationship on the node's schema at all. Almost certainly a typo in how the Play was authored — no future graph state will make it resolve, so report it plainly rather than telling the user to wait.
- **unknown** — a lookup the condition depends on failed while it was evaluated (its `reason` says which), so whether it holds is not known. Not a typo and not a wait: re-run, and report it as unevaluated if it persists.

A non-empty `degraded_reasons` in the output means a schema or graph lookup failed while the result was built: it may be missing rules, may report a real field or relationship as **unresolvable**, and names each condition it reported as **unknown**. Re-run once; if `degraded_reasons` is still non-empty, report the result as incomplete rather than calling any condition a typo.

Scoped to this device only: whether a rule has already fired is not tracked anywhere in the system, so this reports live condition state, never an execution history.

<!-- BEGIN GENERATED: builtin-relationships (see packages/core/src/models/schema.rs (BUILTIN_RELATIONSHIP_NAMES), packages/cli/examples/gen_skill_md.rs) -->
**Built-in relationship names.** Four names are structural and legal between any two nodes without being declared on a schema: `member_of`, `has_child`, `mentions`, `has_role`. They have hardcoded semantics — hierarchy, mentions, collection membership, and roles — and their own UI affordances.

Because they share the one `relationship_type` column with schema-declared relationships, a schema may **not** declare a relationship under one of these names; `schema create`/`schema update` rejects it. Conversely, any *other* name must be declared on the source node's schema before `relationship create` will accept it. When no declared relationship fits, use `mentions`.
<!-- END GENERATED: builtin-relationships -->

**Success semantics:** after `relationship create` returns, confirm the link to the user — don't call `relationship get` afterward just to verify it landed.

**Output:** confirmation of the created edge, or the list of related nodes with `count`/`direction`/`relationship_name`.

### Authoring a skill

A `skill` node is guidance an agent finds by search: its name and `description` are what a request is matched against, and its markdown children are the procedure to follow. Create the root, then add the guidance beneath it as markdown children:

```bash
nodespace node create --type skill --content 'Booking a Venue' \
  --properties '{"description":"Reserve a venue for an event: check its capacity, then record the booking. Use when the user wants to book, reserve or hold a venue.","tool_whitelist":["create_node","update_node","get_node"]}'
```

**Link the skill to the schemas it is about.** A skill written for one type, or for a few, says so with an `applies_to` edge to each type's schema node. A schema's id is its node id, so the target is the type id itself:

```bash
nodespace relationship create --from <skill-id> --type applies_to --to venue
```

Skill search then returns that skill together with exactly those types' fields and relationships, and those of every type that extends them, rather than a guess taken from the wording of the request. A core type can be linked the same way (`--to task`). Leave a general skill, one that applies whatever the type, unlinked. Only a schema can be the target: a link to any other node is rejected.

The same edges read from the schema's end as `skills`: `nodespace relationship get venue --type skills` lists every skill about that type.

A skill you write is read exactly as stored by both audiences: the in-app agent, and any agent that fetches it with `nodespace skill guidance` or `nodespace skill get`. Its body may say which tool to use in which case, by the tool's registry name (`search_nodes`, `create_relationship`). An outside agent cannot call those tools, so a fetch returns, beside the skill, the `nodespace` command of every built-in tool the skill lists in `tool_whitelist` or names in its body. `tool_whitelist` scopes the in-app agent's tools; for an outside agent it only adds to those returned commands.

### Fetching skills

```bash
nodespace skill guidance "add an issue to the current cycle"   # the skills matching a task
nodespace skill guidance                                        # every skill, with the list's version
nodespace skill get "Node Deletion"                             # one skill, by exact name or id
```

A fetch (`guidance "<task>"` or `get`) returns each skill's instructions, its tool commands, and the schemas it touches. In `--json`:

```json
{
  "provenance": "graph-fetched",
  "query": "Recording a Decision",
  "count": 1,
  "guidance": [{
    "node_id": "…", "title": "Recording a Decision", "description": "…",
    "content": "Use `search_nodes` when you know the decision's name. Link it with create_relationship.",
    "tool_commands": [
      { "tool": "create_relationship", "command": "nodespace relationship create" },
      { "tool": "search_nodes", "command": "nodespace query" }
    ]
  }],
  "schemas": []
}
```

- `tool_commands` — each built-in tool the skill lists or names, with the command and subcommand that does the same thing. Where a step names a tool, run its command; the arguments are in this reference. A tool with no `nodespace` equivalent is left out. The body is not rewritten.
- In human output the same list is printed under the skill's instructions, inside its banner, as `- <tool> -> <command>`.
- `skill get` takes the exact name the list shows, or the node id. A name no skill has is an error (exit 1) naming it; a name two skills share is an error listing their ids. `confidence` is `null`: nothing was ranked.
- `confidence` is between 0.0 and 1.0. Skills arrive best match first, so read the order to rank them: several strong matches can all show 1.0.

A listing (`guidance` with no task) returns names and descriptions only, plus the list's `version` (top level in `--json`, in the first line of human output). The version changes when a skill is added, removed or archived, and when a skill's name, description, tool list or any part of its body changes. Two listings with no such change between them print the same version, so comparing it is enough to know whether a list read earlier is still current.

### Schema inspection and management

```bash
# List all registered schemas
nodespace schema list

# Get a specific schema definition
nodespace schema get task
nodespace schema get person

# Create a new schema
nodespace schema create --params '{"name":"Ticket","description":"A tracked unit of engineering work","fields":[{"name":"status","type":"enum","required":true,"coreValues":[{"value":"ready_for_dev","label":"Ready for Dev"},{"value":"in_dev","label":"In Dev"},{"value":"done","label":"Done"}]},{"name":"assignee","type":"text"}],"relationships":[{"name":"belongs_to_sprint","targetType":"sprint","direction":"out","cardinality":"one","reverseName":"tickets","reverseCardinality":"many"}]}'

# Create a schema with a unique field — key flagged uniqueCaseInsensitive
nodespace schema create --params '{"name":"ADR","description":"An architecture decision record","fields":[{"name":"key","type":"text","required":true,"uniqueCaseInsensitive":true},{"name":"status","type":"enum","required":true,"coreValues":[{"value":"proposed","label":"Proposed"},{"value":"accepted","label":"Accepted"},{"value":"superseded","label":"Superseded"}]}]}'

# Update an existing schema — add/remove/rename fields, without re-creating it
nodespace schema update --params '{"schema_id":"ticket","add_fields":[{"name":"sprint","type":"text"}]}'

# Delete a schema — clear its relationship declarations first (see below)
nodespace schema update --params '{"schema_id":"adr","remove_relationships":["decided_by","supersedes"]}'
nodespace schema delete adr
```

`create`/`update` take a single JSON `--params` blob (or `--params-file <path>` for a file) rather than per-field flags — the params shape mirrors `CreateSchemaParams`/`UpdateSchemaParams` in the daemon.

<!-- BEGIN GENERATED: schema-rules (see packages/agent/src/seeds/rules/skill-md/, packages/agent/src/seeds/skill-md/schema-rules.md) -->
**Only the types asked for.** Create exactly the types asked for — no more — then stop and report them. Don't proactively create related types the user didn't ask for (e.g. asked for "ADR" — don't also create "Ticket" or "Sprint"), and don't follow up with `schema update` to wire relationships unless explicitly asked. This is a rule about restraint, not about call count: when the user does ask for several types, create all of them — see *Creating two linked types* for the order.

**Creating two linked types.** When the user asks for a pair (e.g. "Customer and Invoice, linked"), that is two `schema create` calls, not one. A relationship's `targetType` must already exist, or be the type the same call is creating — pointing at a type you only intend to create next is rejected. So create the target type first, then the referencing type, declaring the relationship on the *referencing* side: create `Customer`, then create `Invoice` with `{"name":"billed_to","targetType":"customer","direction":"out","cardinality":"one","reverseName":"invoices","reverseCardinality":"many"}`. The required `reverseName` gives the Customer end its `invoices` accessor for free — one stored edge, readable from both ends, no `schema update` follow-up. Declaring `invoices → invoice` on `Customer` first is rejected: the target doesn't exist yet. Don't omit the relationship here — the user asked for the types to be linked, and omitting it silently delivers two unlinked types.

If `create` reports the schema already exists, stop and tell the user — they can create instances with `node create` against the existing type.

If `create` rejects the schema with a validation error (not "already exists") — for example a `title_template` placeholder missing from `fields`, or an invalid field type — the error names the specific problem. Fix exactly that and retry immediately with the corrected payload; don't ask the user to clarify and don't give up after one rejection.

**Editing:** to add, remove, or rename a field, add a value to an existing enum field, or change a relationship on an existing schema, use `schema update` with only the fields that need changing (`add_fields`/`remove_fields`/`rename_fields`/`add_field_values`, or an updated `description`/`title_template`). Don't re-create the whole schema for a small change.

**Adding a value to an existing enum.** To give a field that already exists a new choice — a `backlog` status on `task`, another priority level — use `add_field_values`, not `add_fields`:

```bash
nodespace schema update --params '{"schema_id":"task","add_field_values":[{"field":"status","values":[{"value":"backlog","label":"Backlog"}]}]}'
```

`add_fields` is the wrong tool here: it declares a *new* field and leaves the original one's vocabulary untouched. Redeclaring the existing field with a fuller `coreValues` list is rejected outright, so extending in place is the only route.

Only a field declared `extensible: true` **and** typed `enum` can be extended — `nodespace schema get <schema_id>` shows both, so check before calling rather than discovering it through a rejection. Added values land in `user_values`; `core_values` is never written.

The operation is all-or-nothing: it is rejected if the field doesn't exist, isn't extensible, isn't an enum, or if any value string already exists on `core_values` or `user_values` — nothing is merged or overwritten. Collision is checked on the `value` string and never on `label` (two values may legitimately share a label), so a rejection naming a colliding value means pick a different `value`, not a different `label`.

**Rename vs. relabel:** `rename_fields` can rename a field's storage key or relabel its display name only — see the tool schema for the `from`/`to`/`friendlyName` shape of each. A user asking to relabel what a field is called on screen almost always means the display label, not a storage rename.

**Deleting a schema.** A node type can be removed — `nodespace schema delete <schema_id>`. Reach for it whenever the user asks to remove, drop, undo or clean up a type, including a throwaway type created earlier in the session; never report deletion as unsupported, and never propose stripping a schema to an empty shell as a substitute. Core types (`task`, `text`, `date`, `person`, …) are the exception: they cannot be deleted, and the attempt is rejected with `schema_is_core`.

Relationship declarations are the one prerequisite: a schema that still declares relationships, or is still targeted by another type's declaration, is rejected with `schema_has_declarations` and the remaining count. Clear them with `schema update` first, then delete:

```bash
# 1. Drop the relationships this type declares
nodespace schema update --params '{"schema_id":"adr","remove_relationships":["decided_by","supersedes"]}'
# 2. Drop declarations on OTHER types that target it (`schema list --json` shows them)
nodespace schema update --params '{"schema_id":"ticket","remove_relationships":["related_adr"]}'
# 3. Delete the schema
nodespace schema delete adr
```

This is the mirror of the `targetType` rule above: a relationship's target must **exist** before the relationship can be declared, and must be **absent** before the type it points at can be deleted.

`schema get` on a type that `extends` another lists the relationships it inherits alongside its own. `remove_relationships` only removes the type's **own** declarations: naming an inherited one is rejected with the ancestor that declares it, and naming one the type doesn't have is rejected with the list of names it does declare. An inherited relationship doesn't block deleting the child, so step 1 for a child type covers only the names it declares itself.

Two scoping notes. Only declarations *between schemas* block the delete — relationship edges between ordinary nodes are instance data and are not counted, so there is no need to unpick those first. And deleting the type does not delete its instances: they remain as nodes of that type, so remove them with `node delete` separately if the user wants them gone too.

**Exception for `extends`:** it is never cleared through `remove_relationships` — that call is rejected outright, since the only way to change an `extends` edge is the dedicated `extends` field on `schema update` (re-targeting it, never clearing it). So the sequence above does not apply to `extends` itself: a schema that extends a parent needs no prerequisite step — deleting it deletes its own `extends` declaration right along with it. A schema OTHER schemas still extend stays blocked with the same `schema_has_declarations` rejection until those children are deleted, or re-targeted onto a different parent: `nodespace schema update --params '{"schema_id":"<child>","extends":"<new-parent>"}'`.

**Specializing an existing type: `extends`.** When a new type IS a more specific version of one that already exists ("an Issue type that's a Task with a severity field"), reach for `extends` rather than hand-copying the base type's fields into a new, unrelated schema — a hand-copied list loses real subtype identity, automatic inheritance of the base type's future changes, and compatibility with Plays/queries already written against the base type.

`extends` is a **first-class key in the schema definition**, taking the parent's schema id — never a hand-written entry in `relationships`:

```json
{"name": "Issue", "extends": "task", "fields": [{"name": "severity", "type": "enum", "coreValues": [{"value": "low", "label": "Low"}, {"value": "high", "label": "High"}]}]}
```

This is the single most important thing to get right: every other relationship is declared with `direction`/`cardinality`/`reverseName` inside `relationships`, so it's tempting to infer `extends` follows the same shape — `{"relationships": [{"name": "extends", "targetType": "task", ...}]}` is exactly that inference, and `create`/`update` reject it outright. `schema update` takes the same top-level `extends` key to set or re-point a parent after creation; there is no way to clear one once set, only re-target it.

Composition is **additive only**: the extending schema cannot redeclare a field its parent already has, even with a different enum vocabulary — that's a hard rejection, not a merge. And **single parent only** — a schema extends at most one other schema.

An instance of the extending schema gets that schema's own id as its real `node_type` — creating an `issue` produces `node_type: "issue"`, never `"task"`. This is the mechanism's whole point: the base type does not persist as the created node's type.

**Querying is scope-projected, not flat.** A query for `node_type: "task"` returns `task` rows *and* every extending instance, but each result is projected to `task`'s own field set — an `issue` in those results carries `status` but not `severity`. To see a subtype's own fields, query that subtype directly (`node_type: "issue"`). Querying the base type and then looking for an extension field on the result finds nothing; it isn't a bug, it's the wrong scope.

**Giving an inherited enum field a richer vocabulary** uses `add_field_values` exactly as usual, with one addition: every newly appended value must carry `mapsTo`, naming which pre-existing value it collapses to at the parent's scope.

```bash
nodespace schema update --params '{"schema_id":"issue","add_field_values":[{"field":"status","values":[{"value":"backlog","label":"Backlog","mapsTo":"todo"}]}]}'
```

Never declare a new, differently-named field (`issue_status`) for this — that isn't an extension of `status` at all, and it's exactly what `mapsTo` exists to make unnecessary: a base-scoped Play or query watching `task.status` keeps matching an `issue` node's `backlog` value as `todo`, unmodified.

**Namespace exception:** fields declared directly on the extending schema's own `fields` list are stored bare — no `custom:`/`org:`/`plugin:` prefix required, unlike the usual rule for extending a type you don't own. They live in their own bucket and never collide with the parent's fields.

**Schema fields:** define only type-specific fields — don't add a `name` or `title` field; every node already has a built-in content/title field. Exception: if `title_template` uses a `{name}` placeholder, `name` must be defined as a field (any placeholder in `title_template` must have a matching field).

**Field source:** derive every field from what the user's own request describes wanting to track — never from another schema shown in the entity-types context. That listing exists so you don't recreate a type that already exists; it is not a shape to copy fields from for a new, unrelated type.

**Enums:** lowercase values with readable labels — `{"value":"in_progress","label":"In Progress"}`.

**Relationships vs. fields:** use a relationship (not a field) when a value references another node type. `targetType` must be an existing schema ID, or the schema ID of the type being created in the same call. If it doesn't exist yet and the user asked for both types, create the target type first and declare the relationship on the type created second (see *Creating two linked types*); omit the relationship only when the target is a type the user never asked for. `reverseName` and `reverseCardinality` are **required** on every relationship — a declaration missing either is rejected. One edge is stored and read from both ends, so name it from both: `reverseName` is what the edge is called read from the target (plural where that end may hold many — `invoices`, not `Invoice (Customer)`), and `reverseCardinality` is `one` or `many`, saying how many sources may point at one target. Examples: `{"name":"supersedes","targetType":"adr","direction":"out","cardinality":"one","reverseName":"superseded_by","reverseCardinality":"one"}`, `{"name":"has_task","targetType":"task","direction":"out","cardinality":"many","reverseName":"ticket","reverseCardinality":"one"}`, `{"name":"decided_by","targetType":"person","direction":"out","cardinality":"one","reverseName":"decisions","reverseCardinality":"many"}`.

**Self-referential relationships:** a type may point at itself in the same `schema create` call — give its own schema ID (the snake_case form of the name); no follow-up `schema update` is needed. The required `reverseName` is what names the other direction, so never declare a second relationship for it — one stored edge, readable from both ends: `{"name":"supersedes","targetType":"adr","direction":"out","cardinality":"one","reverseName":"superseded_by","reverseCardinality":"one"}`. The same shape covers `blocks`/`blocked_by` on a task and `parent`/`child` on a category.

**Grouping is collections, not an array field.** Don't declare a `tags`, `categories`, `topics`, `labels`, `areas` or `groups` field — collections already are the tagging and grouping mechanism, with a flat label and a nested path (`docs:rust`) as the same mechanism at two depths. They also cost the same to write: `node create --collection docs:rust` is one argument, repeatable, with missing path segments created for you and no lookup first — exactly the cost of setting one array element. What differs is what you get. An array value renders in no UI, has to be edited on every member to rename, cannot nest, and is invisible to collection queries; a collection does all four, and `member_of` is structural so joining one needs no schema change. If the user explicitly asks for a plain tags field, give them one without arguing.

**Edge fields.** A relationship can carry attributes on the edge itself via `edgeFields` — facts about the *connection*, not about either node (an access level on a membership, a billing date on an invoice link). Give an edge field a fixed vocabulary by declaring it as an enum with `coreValues`, the same shape a node field uses:

```json
{"name": "access", "type": "enum",
 "coreValues": [{"value": "owner", "label": "Owner"},
                {"value": "editor", "label": "Editor"},
                {"value": "viewer", "label": "Viewer"}]}
```

`coreValues` is required on an enum edge field and rejected on any other type; a `default` must be one of the declared values; values must be unique. Edge enums are closed — no `userValues`/`extensible` half. Creating or editing an edge validates the value against the declared set (including via `--edge-data`), and the relationships UI renders a picker instead of a free-text box.

Two limits worth knowing. Only relationships you declare can carry `edgeFields`: the built-in structural names (`member_of`, `has_child`, `mentions`, `has_role`) are reserved and rejected as declarations, so an edge field cannot be attached to them. And `required`/`default` on an edge field are recorded but not enforced at write time — an omitted enum key is stored absent rather than filled in from `default`, so don't rely on a default to supply a value.

**Title template:** `content` is a node's name for entity types (`Customer`, `Person`, `Invoice`) — for a node created without a parent, NodeSpace surfaces it as the title automatically. Only markdown primitives (`text`, `header`, `quote-block`, `code-block`, etc.) use `content` as a prose body instead of a name. Three cases:
- **Single-field identity** — e.g. `Customer`: one field's value is the whole title. Put it directly in `content`; don't set `title_template`, and don't add a separate field (e.g. `company_name`) that duplicates it.
- **Composed identity** — e.g. `Person` (`first_name` + `last_name`): no single field holds the full title, so assemble one with `title_template: "{first_name} {last_name}"`, using `{field_name}` placeholders — every placeholder must be a defined field.
- **Markdown primitive** — `text`, `header`, etc.: `content` is prose, not a name; `title_template` doesn't apply.

Use `title_template` only to assemble a title from two or more fields. If one field already holds the whole identity, that value belongs in `content` alone.

**Unique fields:** set `"unique": true` on a field when the user's request implies each instance should have a distinct value for it (e.g. "each ticket should have a unique key" → flag `key` unique). Use `"uniqueCaseInsensitive": true` instead when case shouldn't matter — email and username are the common case. This is advisory only: it does not prevent duplicates from being created, it only lets the system surface a likely existing match when a new value collides. Never describe it to the user as blocking or rejecting duplicates — it's a suggestion, not an enforced constraint. Example: `{"name":"key","type":"text","uniqueCaseInsensitive":true}`.
<!-- END GENERATED: schema-rules -->

A `description` field is fine when it adds value beyond the title. Field names are alphanumeric-and-underscore only — the CLAUDE.md-documented `custom:` namespace prefix convention applies to natural-language schema authoring in the local agent, not to explicit `fields` arrays passed here; don't prefix field names when calling `schema create`/`update` directly.

**Recognizing a relationship field.** The "relationships vs. fields" rule above presumes you've already noticed a field is a reference — that recognition step is the hard part. A field naming a **person, team, project, or any other entity** is a reference, even when it reads naturally as text: a plain string has no integrity (`"M. Alibio"` and `"m alibio"` are different values to a query engine), no reverse lookup, and no rename path — renaming a person means rewriting every node that names them. Worked examples, from how a request is phrased:

- "who signed off" → a relationship (`decided_by`, `targetType: person`), not a `deciders: array` field
- "who it's assigned to" → `assignee`, `targetType: person`, not an `assignee: text` field
- "which project it affects" → `affects_project`, `targetType: project`

False friends — field names that read as plain attributes but are usually references: `deciders`, `assignee`, `owner`, `author`, `reviewer`, `reported_by`, `members`. Before defaulting one of these to a text field, check whether the target type already exists (`nodespace schema list`). Escape hatch: free text is legitimate for a one-off external party who will never be a node in this graph — use a relationship when the party is, or could become, a first-class entity here.

**Output:** Schema nodes as JSON (same shape as regular nodes; `node_type="schema"`)

### Manage local databases

One daemon serves a registry of local databases. These subcommands operate on that registry globally — they are never affected by the `--database` flag.

```bash
# List every registered database (the default is marked with *)
nodespace database list

# Create a brand-new database and register it
nodespace database create work
nodespace database create work --path /path/to/work.db   # explicit file location

# Register an existing database file already on disk
nodespace database register /path/to/existing.db

# Rename a database's label (by name or id)
nodespace database rename work "Work Projects"

# Set the daemon-wide default database (used by requests without --database)
nodespace database use work

# Unregister a database (never deletes the underlying file)
nodespace database remove work
```

**Options:**
- `create <name> [--path <path>]` — omit `--path` to let the daemon place the file under its managed database directory
- `rename <name|id> <new-name>` — relabels the entry without moving the file
- `use <name|id>` — sets the registry's default; all clients that don't pass `--database`/`NODESPACE_DATABASE` route here afterwards
- `remove <name|id>` — detaches the registry entry only; the database file is left on disk

A database is addressed by **name or id**. When a name is ambiguous (shared by more than one database), select by id instead — `database list --json` shows each id.

**Output:** `list` prints a table (or the full list with `--json`); the other commands print the affected database record (`--json` emits the full `DatabaseInfo`). A database's status is `open`, `closed`, `missing` (its file is gone) or `requires_extension`.

**Refused databases.** A database can list an extension this NodeSpace build doesn't support. The daemon then refuses to open it and changes nothing in its file. `database list` shows its status as `requires_extension` and appends what it needs after the path; with `--json` the entry carries `unsupported_extensions` and a `refusal` message (`null` for any other database). `nodespace diagnostics` lists every database the same way, in both forms. `database use` accepts such a database and prints a `Warning:` line on stderr: once it is the default, every command without `--database` is refused. Any command routed to such a database, without `--database` when it is the default or with `--database` naming it, exits non-zero with that refusal message and a `Download …` line. Relay the message to the user verbatim, with the download line; the `refusal` field is the message alone, so when relaying from a listing, say the download link comes with the error of any command run against that database. Do not retry, and never move, rename, copy or edit the file to get around it. To keep working, target another database with `--database`. Ask the user before `database use`: it changes the default for every client, the desktop app included.

### Conflicts

Reads and resolves records from the conflict journal — durable evidence that two nodes collide (e.g. two active `person` nodes share a unique-flagged field's value, or two collections share a name). This is a second client onto the same journal the desktop app's Conflicts view reads and writes, so a dismiss/adopt/merge made here is immediately visible there and vice versa.

```bash
# List conflicts (optionally filtered)
nodespace conflicts list
nodespace conflicts list --status open
nodespace conflicts list --kind unique_field_collision
nodespace conflicts list --node <node-id>   # conflicts naming this node as a participant

# Show one conflict record in full, including detail and any prior resolution
nodespace conflicts show <conflict-id>

# Dismiss a conflict as acceptable, without changing either node
nodespace conflicts dismiss <conflict-id>

# Resolve by continuing with an existing node instead of a new one (non-destructive)
nodespace conflicts adopt <conflict-id> --keep <node-id>

# Merge a losing node into a surviving node: unions properties, re-points edges, archives the loser
nodespace conflicts merge --survivor <node-id> --loser <node-id>
nodespace conflicts merge --survivor <node-id> --conflict-id <conflict-id>   # loser inferred from the record
```

**Options:**
- `list [--status open|resolved|dismissed] [--kind unique_field_collision|collection_name_collision] [--node <id>] [--limit <n>]` — `--node` lists only conflicts naming that node as a participant and, when set, ignores `--status`/`--kind`/`--limit`
- `show <conflict-id>` — full record, including `detail` (kind-specific evidence) and `resolution` (once resolved or dismissed)
- `dismiss <conflict-id>` — acknowledges the conflict; re-detection will not reopen it
- `adopt <conflict-id> --keep <node-id>` — resolves without deleting or modifying either node
- `merge --survivor <node-id> [--loser <node-id>] [--conflict-id <conflict-id>]` — `--loser` is required unless `--conflict-id` names an open record with exactly one other participant besides `--survivor`, in which case the loser is inferred

**Which action for which kind:** `dismiss` and `adopt` apply to any kind. `merge` makes sense for `unique_field_collision` (two node records for the same real thing) but not `collection_name_collision` (renaming one collection, via `nodespace node update`, is the fix there — then dismiss or let the record self-resolve).

**Merge is the one irreversible-feeling action here** — it archives the loser node and re-points its edges immediately when called, and is never performed automatically at any confidence level. Only call it once the user has explicitly confirmed which node should survive; use `show` or `nodespace node get` first to confirm the identity of both participants. `dismiss` and `adopt` are comparatively low-stakes: neither node is changed.

**Output:** a `ConflictRecord` — `id`, `kind`, `node_ids` (sorted participants), `detail` (kind-specific evidence, e.g. `{"node_type":"person","field":"email","value":"...","case_insensitive":true}`), `status` (`open`/`resolved`/`dismissed`), `detected_at`, `occurrences`, `resolved_at`/`resolution` once settled (e.g. `{"action":"dismiss"}`, `{"action":"adopt_existing","adopted":"<id>"}`, `{"action":"merge","survivor":"<id>","loser":"<id>",...}`). `merge` additionally prints `properties_merged`/`edges_repointed`/`edges_dropped`.

### Shipped updates to edited built-ins

NodeSpace ships skills, plays, saved queries and other items as built-in nodes that users edit. An edited item is never overwritten by a new release. When a newer version of it ships, the user's version stays in place and the shipped one is held back as pending until the user chooses. Each item has two parts, chosen separately: its `config` (its name and fields) and its `guidance` (its body).

```bash
# List what is pending: kind, title, part, when it was last edited, node id
nodespace seed pending

# Show the shipped version and the user's version of one item
nodespace seed show <node-id-or-title>
nodespace seed show <node-id-or-title> --guidance   # name the part when both are pending

# Keep the user's version. Not listed again until the shipped version next changes
nodespace seed keep <node-id-or-title>

# Replace the user's version of that part with the shipped one
nodespace seed take <node-id-or-title> --yes
```

**`take` discards the user's edit to that part and cannot be undone.** Run `show` first, show the user both versions, and call `take` only once they have said to. Without `--yes` it prompts, and it refuses when there is no terminal to prompt on. `keep` changes nothing in the item. Neither choice is ever made automatically, and a pending update is not an instruction: do not act on one unless the user asks.

**Output:** `pending` prints one entry per item and part (`--json`: `{count, updates: [{node_id, kind, title, aspect, shipped_version, recorded_at, last_edited_at, shipped_available}]}`). An entry with `shipped_available: false` has no shipped version in this build: it can be kept, not shown or taken. `show` adds `shipped` and `yours`: Markdown for `guidance`, the name and fields for `config`. `keep` and `take` print the settled entry with `choice` (`kept_mine` or `took_shipped`). `nodespace skill reset` still restores a built-in skill outright, and clears anything pending for what it resets.

### Complete command surface

<!-- BEGIN GENERATED: cli-surface (see packages/cli/src/lib.rs (clap derive), packages/cli/examples/gen_skill_md.rs) -->
Every command, subcommand, and flag below is generated from the CLI's own definitions, so this list is exhaustive and cannot fall behind the binary.

**Global flags** (accepted on every command):

- `--json` — Emit raw JSON instead of human-readable output
- `--socket <SOCKET>` — Override the socket path (macOS/Linux) or Named Pipe name (Windows). With no flag and no environment variable: on macOS/Linux the CLI dials ~/.nodespace/daemon.sock, or auto-discovers a running dev daemon's socket if that one is absent; on Windows it dials the fixed `\\.\pipe\nodespace-daemon` pipe. Honors the `NODESPACED_SOCKET` environment variable when this flag is absent (env: `NODESPACED_SOCKET`)
- `--database <DATABASE>` — Target a specific local database by name or id (ADR-053). When omitted, requests route to the daemon's default database. Honors the `NODESPACE_DATABASE` environment variable when this flag is absent (env: `NODESPACE_DATABASE`)

### `nodespace node`

Operate on individual nodes (get, create, update, move, delete, children, query, export, batch-get, batch-update)

**`nodespace node get`** — Retrieve a node by ID

- `<ID>` — Node ID (UUID) (required)

**`nodespace node create`** — Create a new node

- `--type <NODE_TYPE>` — Node type, e.g. `text`, `task`, `date` (required)
- `--content <CONTENT>` — Content (plain text or markdown). Omit for a type with a title template (e.g. `person`): its name comes from the template's fields, set with `--property`, and content is rejected
- `--parent <PARENT>` — Parent node ID (omit to create a root node)
- `--property <PROPERTIES>` — Set one or more properties: `--property key=value` (repeatable). Values are parsed as JSON when possible (numbers, booleans, `null`, arrays, objects), otherwise treated as a plain string. Required this way for any schema field that is `required` with no default — validation runs at create time, so there is no way to supply it afterward via `update`
- `--properties <JSON>` — Set several properties at once from one JSON object: `--properties '{"key":"value"}'`. Each value keeps the JSON type it is written with, so this is the form for nested values and for a string that reads as a number (`{"estimate":"3"}`). May be combined with `--property`, which wins for a key given both ways
- `--collection <PATH>` — Collection path to file the node under, `:`-delimited for hierarchy (e.g. `docs:rust`) — the same syntax `import` and `search` take. Missing segments are created. Repeatable to join several collections in one call. Mutually exclusive with --collection-id
- `--collection-id <ID>` — Collection ID to file the node under (repeatable). Prefer --collection, which takes a readable path and needs no lookup

**`nodespace node update`** — Update an existing node's content and/or properties

- `<ID>` — Node ID to update (required)
- `--content <CONTENT>` — New content. Omit to leave content unchanged (e.g. when only setting properties)
- `--property <PROPERTIES>` — Set one or more properties: `--property key=value` (repeatable). Values are parsed as JSON when possible (numbers, booleans, `null`, arrays, objects), otherwise treated as a plain string. Deep-merged into the node's existing properties (unspecified keys are left untouched). Do NOT use this to change a task's status; use `node set-status` instead
- `--properties <JSON>` — Set several properties at once from one JSON object: `--properties '{"key":"value"}'`, deep-merged like `--property`. Each value keeps the JSON type it is written with. May be combined with `--property`, which wins for a key given both ways
- `--collection <PATH>` — Collection path to add the node to, `:`-delimited for hierarchy (e.g. `docs:rust`). Missing segments are created. Repeatable. Mutually exclusive with --collection-id
- `--collection-id <ID>` — Collection ID to add the node to (repeatable). Prefer --collection
- `--remove-collection-id <ID>` — Collection ID to remove the node from (repeatable)
- `--version <VERSION>` — The node version you read. Updates only if the node is still at it; otherwise nothing is written and the current version is reported. Omit to update whatever is current

**`nodespace node set-status`** — Set a task node's status (dedicated verb — do not use `update` for this)

- `<ID>` — Task node ID (required)
- `<STATUS>` — New status. Must be one of the values the `task` schema's `status` field declares — the four built-ins (open, in_progress, done, cancelled) plus any added since. An invalid value is rejected with the current list (required)
- `--version <VERSION>` — The task version you read. Sets the status only if the task is still at it; otherwise nothing is written and the current version is reported. Omit to update whatever is current

**`nodespace node move`** — Move a node under another parent (or to the root), or change its position among its siblings. The node keeps its ID and everything nested under it

- `<ID>` — Node ID to move (required)
- `--parent <PARENT>` — New parent node ID. Omit (with `--root` also omitted) to keep the current parent and only change the position among its siblings
- `--root` — Make the node a root node (no parent). Root nodes have no order, so this takes no position
- `--first` — Place the node first among its siblings. With neither `--first` nor `--after`, a node given a new parent is placed last
- `--after <SIBLING_ID>` — Place the node directly after this sibling, which must be a child of the parent the node ends up under
- `--version <VERSION>` — The node version you read. Moves only if the node is still at it; otherwise nothing is written and the current version is reported. Omit to move whatever is current

**`nodespace node delete`** — Delete a node and everything nested under it, in two steps: without `--version`/`--descendants` it only previews what would be removed and prints the exact command that deletes it

- `<ID>` — Node ID to delete (required)
- `--version <VERSION>` — The node version its preview showed. Deletes only if it still matches
- `--descendants <DESCENDANTS>` — The nested-node count its preview showed. Deletes only if it still matches

**`nodespace node children`** — List the direct children of a node

- `<ID>` — Parent node ID (required)

**`nodespace node query`** — Query nodes with structured filters

- `--id <ID>` — Filter by exact node ID
- `--mentioned-by <MENTIONED_BY>` — Filter nodes that mention this node ID
- `--content-contains <CONTENT_CONTAINS>` — Filter by substring in content
- `--title-contains <TITLE_CONTAINS>` — Filter by substring in title
- `--type <NODE_TYPE>` — Filter by node type (e.g. `text`, `task`)
- `--limit <LIMIT>` — Maximum number of results (0 = server default)
- `--offset <OFFSET>` — Result offset for pagination

**`nodespace node export`** — Export a node and its subtree as markdown

- `<ID>` — Node ID to export (required)
- `--children <CHILDREN>` — Include children recursively (default: true)
- `--max-depth <MAX_DEPTH>` — Maximum recursion depth (0 = server default of 20)
- `--node-ids <NODE_IDS>` — Embed HTML comments with node IDs for OCC (default: true)

**`nodespace node batch-get`** — Fetch multiple nodes in one request

- `--id <IDS>` — Node IDs to fetch (repeatable: --id <id1> --id <id2>) (required)

**`nodespace node batch-update`** — Update multiple nodes in one request (OCC-aware)

- `--updates <UPDATES>` — JSON-encoded array of update objects: [{"node_id":"…","content":"…","version":N}]. Each item may have: node_id (required), version (optional), content, node_type, properties (required)

### `nodespace model`

Manage the local inference model (list, load, recommended)

**`nodespace model list`** — List models in the catalog and their download/load status

**`nodespace model load`** — Load a model (downloading first if needed); streams progress to stdout (with `--json`, prints a single document once the model is ready)

- `<MODEL_ID>` — Model id to load, e.g. `gemma-4-e4b-q4km`. Omit to use the recommended model

**`nodespace model recommended`** — Print the recommended model id for this machine's RAM

**`nodespace model status`** — Print the loaded model, the context window granted to it, and host RAM

### `nodespace search`

Semantic search across the knowledge graph

- `<QUERY>` — Free-text query. Pass an empty string or "*" when using --type for type-only listing — both enumerate every node of the type rather than being treated as a literal search term
- `--type <TYPE>` — Filter results to one or more node types (e.g. `--type task --type text`)
- `--collection <COLLECTION>` — Filter to a collection by path (mutually exclusive with --collection-id)
- `--collection-id <COLLECTION_ID>` — Filter to a collection by ID (mutually exclusive with --collection)
- `--filters <FILTERS>` — JSON-encoded array of {field, operator, value} filter objects
- `--threshold <THRESHOLD>` — Semantic similarity threshold, 0.0-1.0 (0.0 = server default of 0.7)
- `--limit <LIMIT>` — Maximum number of results to return (0 = server default, currently 20)
- `--include-content` — Attach each top result's aggregated subtree markdown to the response, so a hit can be answered from directly instead of needing a follow-up `node get`/`node export` per result. Bounded server-side to the top 5 results regardless of `--limit`; off by default so a plain search stays cheap

### `nodespace query`

Structured property query with comparison operators (equals/contains/gt/lt/gte/lte/in/exists)

- `--type <TARGET_TYPE>` — Target node type ("task", "text", etc.) or "*" for all types (required)
- `--filters <FILTERS>` — JSON array of filter conditions, e.g. `[{"type":"property","operator":"equals","property":"status","value":"open"}]`. Supported types: property, content, metadata, relationship, related. A relationship filter names a `path` of relationship names and the `node_id` it must reach, e.g. `[{"type":"relationship","operator":"equals","path":["child_of"],"node_id":"<id>"}]`. Supported operators: equals, contains, gt, lt, gte, lte, in, exists. A property filter on a date field takes `relative_date` in place of `value` for a date relative to the day the query runs, e.g. `[{"type":"property","operator":"lte","property":"due_date","relative_date":{"anchor":"today","offset_days":7}}]`. Any filter takes `"negate": true` to keep the nodes it does not hold for; on a related filter that is "the path reaches no node matching the nested filter". A property may be a path into an object field's value, e.g. `"property":"repository.url"`
- `--sorting <SORTING>` — JSON array of sort configs, e.g. `[{"field":"due_date","direction":"desc"}]`
- `--limit <LIMIT>` — Max results to return (0 = server default of 50)

**`nodespace query run`** — Run a saved query node by its id or title, with its stored filters, sorting and limit

- `<QUERY>` — The saved query's id, or its title (quoted when it has spaces). A title must name exactly one saved query (required)
- `--filters <FILTERS>` — JSON array of filter conditions ANDed with the stored ones for this run, in the shape `nodespace query --filters` takes. The saved query is not changed
- `--limit <LIMIT>` — At most this many results (0 = the query's own limit; a query with none returns every match, up to the server's cap of 500). It can lower the stored limit, never raise it

### `nodespace diagnostics`

Developer diagnostics: database path, size, node counts, schema count, daemon process memory

### `nodespace logs`

Read the daemon's log — where Play execution errors go

- `--filter <FILTER>` — Show only lines containing this text — a play id, a rule name, an error type. Matched literally, not as a regex
- `--lines <LINES>` — How many matching lines to show, most recent last
- `--path-only` — Print the resolved log file path and exit without reading it

### `nodespace import`

Import markdown files into NodeSpace

**`nodespace import file`** — Import a single markdown file

- `<FILE>` — Path to the markdown file (required)
- `--collection <COLLECTION>` — Collection path to assign the document to (e.g. "docs:rust")
- `--use-filename-as-title` — Use the filename stem as the document title
- `--auto-collection-routing` — Route files to collections based on directory structure
- `--replace` — Refresh an already-imported document in place: replace its child subtree from the fresh parse, keeping the root node so inbound links survive. Without this, an already-imported document is left untouched

**`nodespace import dir`** — Import all markdown files from a directory (recurses into sub-folders by default; see --no-recursive)

- `<DIRECTORIES>` — Path(s) to the directory containing markdown files. Pass more than one to import several directories in a single call: each directory is walked and routed relative to its own root, and results are combined into one summary rather than reported per directory (required)
- `--collection <COLLECTION>` — Collection path to assign all documents to
- `--use-filename-as-title` — Use filename stems as document titles
- `--auto-collection-routing` — Route files to collections based on directory structure
- `--exclude <EXCLUDE_PATTERNS>` — Directory names to exclude (repeatable, e.g. --exclude node_modules)
- `--include-agent-files` — Include CLAUDE.md / AGENTS.md / DESIGN.md files (default: excluded). Matched by basename, case-insensitive, at any depth
- `--include-hidden` — Include hidden files and folders — any path component starting with '.', e.g. .git/, .claude/, dotfiles (default: skipped)
- `--no-recursive` — Import only the top-level directory; do not descend into sub-folders (default: recurses into sub-folders)
- `--replace` — Refresh already-imported documents in place: replace each existing document's child subtree from the fresh parse, keeping its root node so inbound links survive. Without this, already-imported documents are skipped (a plain re-import never duplicates)

### `nodespace mention`

Manage mention relationships between nodes

**`nodespace mention create`** — Create a mention relationship from one node to another

- `--from <FROM>` — The node that contains the mention (source) (required)
- `--to <TO>` — The node being mentioned (target) (required)

**`nodespace mention delete`** — Delete a mention relationship

- `--from <FROM>` — The node that contains the mention (source) (required)
- `--to <TO>` — The node being mentioned (target) (required)

**`nodespace mention outgoing`** — List nodes that a given node mentions (outgoing)

- `<ID>` — Node ID to query mentions for (required)

**`nodespace mention incoming`** — List nodes that mention a given node (incoming)

- `<ID>` — Node ID to query mentions for (required)

### `nodespace schema`

Inspect and manage node type schema definitions

**`nodespace schema list`** — List all schema definitions

**`nodespace schema get`** — Get a single schema definition by ID

- `<ID>` — Schema ID (node type identifier, e.g. `task`, `person`) (required)

**`nodespace schema create`** — Create a new schema from a JSON params blob

- `--params <PARAMS>` — JSON params. For `create`: {"name", "description"?, "fields"?, "relationships"?, "title_template"?, ...} — see CreateSchemaParams. For `update`: {"schema_id", "add_fields"?, "remove_fields"?, "rename_fields"?, "add_field_values"?, "add_relationships"?, "remove_relationships"?, ...} — see UpdateSchemaParams. Mutually exclusive with `--params-file`
- `--params-file <PARAMS_FILE>` — Path to a file containing the JSON params (alternative to inline `--params`)

**`nodespace schema update`** — Update an existing schema from a JSON params blob

- `--params <PARAMS>` — JSON params. For `create`: {"name", "description"?, "fields"?, "relationships"?, "title_template"?, ...} — see CreateSchemaParams. For `update`: {"schema_id", "add_fields"?, "remove_fields"?, "rename_fields"?, "add_field_values"?, "add_relationships"?, "remove_relationships"?, ...} — see UpdateSchemaParams. Mutually exclusive with `--params-file`
- `--params-file <PARAMS_FILE>` — Path to a file containing the JSON params (alternative to inline `--params`)

**`nodespace schema delete`** — Delete a schema definition by ID

- `<ID>` — Schema ID to delete (node type identifier, e.g. `adr`, `person`) (required)

### `nodespace playbook`

Inspect and control Play automation rule-sets (list, enable, disable, get-workflow-state)

**`nodespace playbook list`** — List the installed Plays with each one's state: on, off (disabled), or suspended by the engine on this device, with the reason and time

- `--include-archived` — Also list archived Plays. An archived Play runs nowhere, whatever its switch says

**`nodespace playbook enable`** — Switch a Play on. Also clears a suspension: the engine checks the Play again and suspends it again if the problem remains

- `<PLAY_ID>` — Play ID (node ID of the `play` node) (required)

**`nodespace playbook disable`** — Switch a Play off: it stops running and stays in the list

- `<PLAY_ID>` — Play ID (node ID of the `play` node) (required)

**`nodespace playbook get-workflow-state`** — Evaluate a node against every active Play rule that could apply to its type, and report which conditions are satisfied, not yet met, or unresolvable (a likely typo in a condition's path)

- `<NODE_ID>` — ID of the node to evaluate active Play rules against (required)

### `nodespace relationship`

Manage typed relationship edges between nodes (distinct from mentions)

**`nodespace relationship create`** — Create a typed relationship edge from one node to another

- `--from <FROM>` — Source node ID: the record that acts ("A supersedes B" is `--from A --to B`) (required)
- `--type <RELATIONSHIP_NAME>` — Relationship name: one declared on the source node's schema, or a built-in one (`member_of`, `has_child`, `mentions`, `has_role`) (required)
- `--to <TO>` — Target node ID: the record acted upon (required)
- `--edge-data <EDGE_DATA>` — Optional JSON-encoded edge properties

**`nodespace relationship get`** — List nodes related to a given node via a named relationship

- `<ID>` — Node ID to query relationships for (required)
- `--type <RELATIONSHIP_NAME>` — Relationship name: one declared on the node's schema, its declared reverse name, or a built-in one (required)
- `--direction <DIRECTION>` — Direction to traverse

### `nodespace conflicts`

Inspect and resolve the local conflict journal (list, show, dismiss, adopt, merge)

**`nodespace conflicts list`** — List conflict records, optionally filtered by status, kind, or participant node

- `--status <STATUS>` — Filter by status: open | resolved | dismissed. Omit for every status
- `--kind <KIND>` — Filter by kind: unique_field_collision | collection_name_collision
- `--node <NODE>` — List only conflicts naming this node id as a participant
- `--limit <LIMIT>` — Cap the number of records returned. Ignored when `--node` is set

**`nodespace conflicts show`** — Show a single conflict record by its own id, including detail and any prior resolution

- `<CONFLICT_ID>` — Conflict record id (required)

**`nodespace conflicts dismiss`** — Dismiss a conflict as acceptable — acknowledges it without changing any node

- `<CONFLICT_ID>` — Conflict record id (required)

**`nodespace conflicts adopt`** — Resolve a conflict by continuing with an existing node instead of the new one (non-destructive)

- `<CONFLICT_ID>` — Conflict record id (required)
- `--keep <KEEP>` — The node id to keep — the counterparty is resolved without navigating to it (required)

**`nodespace conflicts merge`** — Merge a losing node into a surviving node: unions properties, re-points edges, archives the loser

- `--survivor <SURVIVOR>` — Surviving node id — receives the union of properties and every re-pointed edge (required)
- `--loser <LOSER>` — Losing node id, archived after the merge. Required unless `--conflict-id` names a two-participant record, in which case the other participant is used
- `--conflict-id <CONFLICT_ID>` — The open conflict record this merge resolves, closed as resolved in the same transaction

### `nodespace seed`

Review shipped changes to built-in items you have edited (pending, show, take, keep)

**`nodespace seed pending`** — List the built-in items you have edited that have a newer shipped version: kind, title, which part (config or guidance), and when you last edited it

**`nodespace seed show`** — Show one pending item's shipped version and your version

- `<ITEM>` — The item's node id, or its exact title as `nodespace seed pending` lists it (required)
- `--config` — The item's config: its name and its fields
- `--guidance` — The item's guidance: its body

**`nodespace seed take`** — Replace your version of one part of one item with the shipped version. Discards your edit to that part; asks for confirmation unless `--yes` is passed

- `<ITEM>` — The item's node id, or its exact title as `nodespace seed pending` lists it (required)
- `--config` — The item's config: its name and its fields
- `--guidance` — The item's guidance: its body
- `--yes` — Take the shipped version without prompting. Required when there is no interactive terminal: taking discards an edit, so it is never done unattended without this flag

**`nodespace seed keep`** — Keep your version of one part of one item. It stops being pending until the shipped version changes again

- `<ITEM>` — The item's node id, or its exact title as `nodespace seed pending` lists it (required)
- `--config` — The item's config: its name and its fields
- `--guidance` — The item's guidance: its body

### `nodespace session`

Manage PTY agent sessions (launch, attach, list, kill)

**`nodespace session launch`** — Launch a new agent session and stream its output to stdout

- `<AGENT>` — Agent to launch: claude-code, codex, antigravity, pi, opencode (required)
- `--prompt <PROMPT>` — Initial prompt passed to the agent at launch time
- `--cols <COLS>` — Terminal width in columns (defaults to current terminal width)
- `--rows <ROWS>` — Terminal height in rows (defaults to current terminal height)

**`nodespace session attach`** — Attach to an existing session's output stream

- `<SESSION_ID>` — Session ID to attach to (required)

**`nodespace session list`** — List active agent sessions

**`nodespace session kill`** — Terminate a running session

- `<SESSION_ID>` — Session ID to terminate (required)

### `nodespace database`

Manage the daemon's registry of local databases (list, create, register, remove, rename, use)

**`nodespace database list`** — List every registered database with its status and the default marker

**`nodespace database create`** — Create a brand-new database and register it

- `<NAME>` — Human-facing label for the new database (required)
- `--path <PATH>` — Explicit path for the new database file. When omitted the daemon places it under its managed database directory

**`nodespace database register`** — Register an existing database file already present on disk

- `<PATH>` — Absolute path to an existing database file to register (required)

**`nodespace database remove`** — Unregister a database (never deletes the underlying file)

- `<DATABASE>` — Database to unregister, by name or id (required)

**`nodespace database rename`** — Rename a registered database's human-facing label

- `<DATABASE>` — Database to rename, by name or id (required)
- `<NEW_NAME>` — New human-facing label (required)

**`nodespace database use`** — Set the daemon-wide default database (used when no database is selected)

- `<DATABASE>` — Database to make the default, by name or id (required)

### `nodespace uninstall`

Uninstall NodeSpace: stop daemon, remove binaries and service registration

### `nodespace skill`

Install, remove, or check the NodeSpace skill for detected AI-agent harnesses (Claude Code, Codex, Antigravity CLI, OpenCode, Pi) -- the CLI-only equivalent of the desktop app's first-launch skill installer -- and fetch the graph's own skills for a task (`guidance`)

**`nodespace skill install`** — Detect AI-agent harnesses and install the NodeSpace skill into them. Safe to re-run: a harness whose skill files are already current is left alone and reported as up to date, one holding an older skill is updated, and a harness installed since the last run is picked up

- `--yes` — Install without prompting for confirmation. Implied automatically when stdin/stdout isn't a terminal (CI, a script, an agent's non-interactive shell) — mirrors install.sh's `--gui`/`--no-gui` no-TTY default: never hang waiting on a prompt that can't be answered

**`nodespace skill uninstall`** — Remove the NodeSpace skill from detected (or specified) harnesses

**`nodespace skill status`** — Report which harnesses currently have the skill installed, and which are present on this machine without it

**`nodespace skill guidance`** — Fetch the skills that match a task, each with its instructions, the commands of the tools it names, and the schemas of the types the task touches. With no task, list every skill by name and description, with the list's version. Covers the built-in skills, skills a user wrote and skills an installed workflow added. Output is always provenance-marked (a banner in human mode, a `"provenance": "graph-fetched"` envelope in `--json` mode), because it is read from the graph and anyone with write access can edit it

- `<QUERY>` — The task at hand, in your own words (e.g. "add an issue to the current cycle", "define a new type with an enum field"). Matched by meaning against every skill's name and description, ranked the way the in-app agent ranks skills. Omit it, or pass an empty string, to list every skill by name and description without its instructions
- `--limit <LIMIT>` — Maximum number of skills to return for a task. The daemon returns at most 10 whatever is asked for. Ignored when listing

**`nodespace skill get`** — Fetch one skill by its exact name or its id, with what `guidance` returns for a matched skill: its instructions, the commands of the tools it names and the schemas it is linked to, provenance-marked the same way. Use it when you already know which skill you need, from the list or from an earlier fetch. Fails when no skill has that name

- `<NAME_OR_ID>` — The skill's exact name as the list shows it (e.g. "Node Deletion"), or its node id. Case-sensitive, no normalization (required)

**`nodespace skill reset`** — Discard a user's customization of a seeded skill node's config (description/exclusion/tool_whitelist/max_iterations) and/or guidance (procedural markdown), restoring it to the currently-compiled template, whether or not a newer shipped version is pending (`nodespace seed pending`). It overrides the `_seed.config_modified` / `_seed.guidance_modified` durability guard (ADR-072) — reconciliation on daemon startup never discards a user-modified aspect on its own. Requires confirmation unless `--yes` is passed

- `<KEY>` — The seed key to reset — a seeded skill's exact title (e.g. "Research & Search"), matching what `nodespace skill guidance` fetches under. Case-sensitive, no normalization (required)
- `--guidance` — Reset the procedural guidance (markdown children) to the currently- compiled template, discarding any customization
- `--config` — Reset the config (description/exclusion/tool_whitelist/max_iterations) to the currently-compiled template, discarding any customization
- `--all` — Reset both guidance and config — equivalent to passing both flags
- `--yes` — Reset without prompting for confirmation. Required in a non-interactive context (no `--yes` there is a hard error, not an auto-proceed) — unlike `install`/`mcp enable`, this is the one destructive path in the system (ADR-072), and auto-confirming a content discard with no one watching would defeat the point of requiring confirmation at all

### `nodespace mcp`

Host a stdio MCP server exposing one passthrough tool, for bash-less MCP surfaces (e.g. Claude Desktop's Chat tab) that cannot shell this CLI directly — see `commands::mcp` for the architecture and its ADR-038 trust-boundary controls. Disabled until `nodespace mcp install` explicitly turns it on. With no subcommand, hosts the stdio server itself — what a client config launches, not something a person types directly

**`nodespace mcp install`** — Configure a detected bash-less MCP client (currently Claude Desktop) to launch `nodespace mcp`, and enable the passthrough tool. Safe to re-run

- `--yes` — Install without prompting for confirmation. Implied automatically when stdin/stdout isn't a terminal — mirrors `nodespace skill install`'s `--yes`

**`nodespace mcp uninstall`** — Remove the MCP config this wrote from every detected client and disable the passthrough tool again

**`nodespace mcp status`** — Report whether the passthrough tool is enabled and which clients currently have a config pointing at it

<!-- END GENERATED: cli-surface -->
