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

The one real exception is a **schema** node reached through `relationship get`.
It comes back in the schema's own shape rather than the node shape: camelCase
keys (`isCore`, `schemaVersion`, `description`, `fields`, `relationships`) plus
a `uri`, and no `node_type` or `properties` at all. Read schemas with
`schema get` instead of traversing to them.

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

**Output:** JSON with `id`, `node_type`, `content`, `parent_id`, `created_at`

**Creating an instance of a custom type:** read the schema first (`nodespace schema get <type>`) so you know its fields. Use the field name exactly as it appears in the schema's `fields[].name` — do not add namespace prefixes when setting properties on instances (prefixes like `custom:` are part of a *field's name* at schema-authoring time, not something a caller adds — see Schema fields below). If the schema has a `title_template`, `--content` only needs a brief descriptive label — the display title is generated from properties. If there's no `title_template`, set `--content` to the best human-readable name available.

Only include properties the schema actually defines as required, plus any optional ones the user gave a value for. Don't invent fields.

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

At least one of `--content` or `--property` is required.

**Find then update:** if you don't already have the node's ID, run `nodespace search` or `nodespace node query` first to locate it, then update by ID. If the search comes back with zero matches or several equally plausible matches, ask the user one specific clarifying question rather than retrying — e.g. "I found 3 tickets in review — which one did you mean: the auth one, the CI one, or the audit-log one?"

**Do NOT use `node update --property status=...` for task status changes** — use `nodespace node set-status` instead (below); it validates against the allowed status values before writing.

**Output:** Updated node JSON. Confirm the change to the user from this response — don't re-fetch the node afterward to double-check.

### Set a task's status

```bash
nodespace node set-status <task-id> in_progress
```

Dedicated verb for task status transitions. Status must be one of: `open`, `in_progress`, `done`, `cancelled` — invalid values are rejected before the update is sent.

**Output:** Updated node JSON.

### Delete a node

```bash
nodespace node delete <node-id>
```

**Find then delete:** locate the node via `nodespace search` or `nodespace node query` if you don't have its ID, and confirm the title matches what the user described before deleting. Delete one node per call; confirm each deletion before moving to the next. Don't search again afterward to verify the deletion — the delete response confirms it.

**Output:** Confirmation JSON

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
- `--filters <json>` — array of filter conditions: `{"type":"property"|"content"|"relationship"|"metadata","operator":"equals"|"contains"|"gt"|"lt"|"gte"|"lte"|"in"|"exists","property":"...","value":...}`
- `--sorting <json>` — array of `{"field":"...","direction":"asc"|"desc"}`
- `--limit <n>` — max results (0 = server default of 50; server caps at 500 regardless of the value passed)

See the Tool Decision Guide above for worked examples. This is the CLI counterpart of the property-filtering path of the local agent's `search_nodes` tool.

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
```

**Options:**
- `--type <type>` — filter by node type (repeatable)
- `--collection <path>` / `--collection-id <id>` — narrow to a collection (mutually exclusive)
- `--filters <json>` — array of `{field, operator, value}` filter objects
- `--threshold <0.0-1.0>` — similarity cutoff (0.0 = server default of 0.7); lower it (e.g. 0.1-0.2) for broader recall when results are sparse
- `--limit <n>` — max results (default: 20)

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
# Create a relationship edge (relationship name must exist on the source node's schema)
nodespace relationship create --from <source-id> --type has_task --to <target-id>
nodespace relationship create --from <source-id> --type billed_to --to <target-id> --edge-data '{"note":"..."}'

# Traverse relationships from a node
nodespace relationship get <node-id> --type has_task --direction out
nodespace relationship get <node-id> --type billed_to --direction in

# Traverse the reverse direction by the schema's declared reverseName
# (adr declares: name decided_by, targetType person, reverseName decisions, reverseCardinality many)
nodespace relationship get <person-id> --type decisions
nodespace relationship get <person-id> --type decided_by --direction in   # equivalent
```

**Options (`create`):**
- `--from <id>` — source node ID
- `--type <name>` — relationship name, as defined on the source node's schema (e.g. `has_task`, `billed_to`) — not an arbitrary label
- `--to <id>` — target node ID
- `--edge-data <json>` — optional JSON-encoded edge properties

**Options (`get`):**
- `<id>` — node ID to query relationships for
- `--type <name>` — relationship name to traverse: the forward `name` from the source's end, or the declared `reverseName` from the target's end
- `--direction <out|in>` — traversal direction (default: `out`), relative to the name given

**Traversing the reverse direction.** A relationship is declared once, on the source type, but reads from both ends. Given `{"name":"decided_by","targetType":"person","direction":"out","cardinality":"one","reverseName":"decisions","reverseCardinality":"many"}` on `adr`: from the ADR, `nodespace relationship get <adr-id> --type decided_by --direction out`; from the person, use the declared `reverseName` — `nodespace relationship get <person-id> --type decisions` — or the equivalent `--type decided_by --direction in`. Both spellings return the same ADRs, and the output line's arrow shows the direction actually traversed (`<--decided_by--` for an inbound resolution). An empty result means no edges exist, not that reverse traversal is unsupported. A name declared in neither direction is rejected with an error naming the spellings that do work — read it and retry rather than concluding the capability is missing.

Reverse names are for *traversal*, not for `relationship create`: an edge is always created under its forward name, from the source node. They are also not usable in `node query --filters`, whose `relationship` filters cover only the structural graph (`parent`, `children`, `mentions`, `mentioned_by`) — use `relationship get` to traverse a schema-declared name.

Both node IDs must already exist — search for missing IDs first (`nodespace search` / `nodespace node query`). Apart from the built-in names below, the relationship name must be defined on the source node's schema; define it there (`nodespace schema create`/`update`) if it isn't yet. `relationship create` on a node whose schema doesn't define that relationship name fails with an error naming the undefined relationship.

<!-- BEGIN GENERATED: builtin-relationships (see packages/core/src/models/schema.rs (BUILTIN_RELATIONSHIP_NAMES), packages/cli/examples/gen_skill_md.rs) -->
**Built-in relationship names.** Four names are structural and legal between any two nodes without being declared on a schema: `member_of`, `has_child`, `mentions`, `has_role`. They have hardcoded semantics — hierarchy, mentions, collection membership, and roles — and their own UI affordances.

Because they share the one `relationship_type` column with schema-declared relationships, a schema may **not** declare a relationship under one of these names; `schema create`/`schema update` rejects it. Conversely, any *other* name must be declared on the source node's schema before `relationship create` will accept it. When no declared relationship fits, use `mentions`.
<!-- END GENERATED: builtin-relationships -->

**Success semantics:** after `relationship create` returns, confirm the link to the user — don't call `relationship get` afterward just to verify it landed.

**Output:** confirmation of the created edge, or the list of related nodes with `count`/`direction`/`relationship_name`.

### Schema inspection and management

```bash
# List all registered schemas
nodespace schema list

# Get a specific schema definition
nodespace schema get task
nodespace schema get person

# Create a new schema
nodespace schema create --params '{"name":"Ticket","description":"A tracked unit of engineering work","fields":[{"name":"status","type":"enum","required":true,"coreValues":[{"value":"ready_for_dev","label":"Ready for Dev"},{"value":"in_dev","label":"In Dev"},{"value":"done","label":"Done"}]},{"name":"assignee","type":"text"}],"relationships":[{"name":"belongs_to_sprint","targetType":"sprint","direction":"out","cardinality":"one","reverseName":"tickets","reverseCardinality":"many"}]}'

# Create a schema with a unique field — key flagged unique_case_insensitive
nodespace schema create --params '{"name":"ADR","description":"An architecture decision record","fields":[{"name":"key","type":"text","required":true,"unique_case_insensitive":true},{"name":"status","type":"enum","required":true,"coreValues":[{"value":"proposed","label":"Proposed"},{"value":"accepted","label":"Accepted"},{"value":"superseded","label":"Superseded"}]}]}'

# Update an existing schema — add/remove/rename fields, without re-creating it
nodespace schema update --params '{"schema_id":"ticket","add_fields":[{"name":"sprint","type":"text"}]}'

# Delete a schema — clear its relationship declarations first (see below)
nodespace schema update --params '{"schema_id":"adr","remove_relationships":["decided_by","supersedes"]}'
nodespace schema delete adr
```

`create`/`update` take a single JSON `--params` blob (or `--params-file <path>` for a file) rather than per-field flags — the params shape mirrors `CreateSchemaParams`/`UpdateSchemaParams` in the daemon.

<!-- BEGIN GENERATED: schema-rules (see packages/agent/src/skill_rules.rs, packages/cli/examples/gen_skill_md.rs) -->
**One schema per request.** Create exactly the type asked for, in a single `schema create` call, then stop and report it. Don't proactively create related types the user didn't ask for (e.g. asked for "ADR" — don't also create "Ticket" or "Sprint"), and don't follow up with `schema update` to wire relationships unless explicitly asked. A relationship's `targetType` must already exist (check `nodespace schema list`) or be the type this call is creating; if it is neither, omit the relationship rather than creating the other type as a side effect.

If `create` reports the schema already exists, stop and tell the user — they can create instances with `node create` against the existing type.

If `create` rejects the schema with a validation error (not "already exists") — for example a `title_template` placeholder missing from `fields`, or an invalid field type — the error names the specific problem. Fix exactly that and retry immediately with the corrected payload; don't ask the user to clarify and don't give up after one rejection.

**Editing:** to add, remove, or rename a field, or change a relationship on an existing schema, use `schema update` with only the fields that need changing (`add_fields`/`remove_fields`/`rename_fields`, or an updated `description`/`title_template`). Don't re-create the whole schema for a small change.

**Rename vs. relabel:** `rename_fields` can rename a field's storage key or relabel its display name only — see the tool schema for the `from`/`to`/`friendlyName` shape of each. A user asking to relabel what a field is called on screen almost always means the display label, not a storage rename.

**Deleting a schema.** A node type can be removed — `nodespace schema delete <schema_id>`. Reach for it whenever the user asks to remove, drop, undo or clean up a type, including a throwaway type created earlier in the session; never report deletion as unsupported, and never propose stripping a schema to an empty shell as a substitute.

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

Two scoping notes. Only declarations *between schemas* block the delete — relationship edges between ordinary nodes are instance data and are not counted, so there is no need to unpick those first. And deleting the type does not delete its instances: they remain as nodes of that type, so remove them with `node delete` separately if the user wants them gone too.

**Schema fields:** define only type-specific fields — don't add a `name` or `title` field; every node already has a built-in content/title field. Exception: if `title_template` uses a `{name}` placeholder, `name` must be defined as a field (any placeholder in `title_template` must have a matching field).

**Field source:** derive every field from what the user's own request describes wanting to track — never from another schema shown in the entity-types context. That listing exists so you don't recreate a type that already exists; it is not a shape to copy fields from for a new, unrelated type.

**Enums:** lowercase values with readable labels — `{"value":"in_progress","label":"In Progress"}`.

**Relationships vs. fields:** use a relationship (not a field) when a value references another node type. `targetType` must be an existing schema ID, or the schema ID of the type being created in the same call. `reverseName` and `reverseCardinality` are **required** on every relationship — a declaration missing either is rejected. One edge is stored and read from both ends, so name it from both: `reverseName` is what the edge is called read from the target (plural where that end may hold many — `invoices`, not `Invoice (Customer)`), and `reverseCardinality` is `one` or `many`, saying how many sources may point at one target. Examples: `{"name":"supersedes","targetType":"adr","direction":"out","cardinality":"one","reverseName":"superseded_by","reverseCardinality":"one"}`, `{"name":"has_task","targetType":"task","direction":"out","cardinality":"many","reverseName":"ticket","reverseCardinality":"one"}`, `{"name":"decided_by","targetType":"person","direction":"out","cardinality":"one","reverseName":"decisions","reverseCardinality":"many"}`.

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

**Title template:** set `title_template` when a node's identity comes from its fields rather than free-form content, using `{field_name}` placeholders — every placeholder must be a defined field. Omit it if the content/title field alone identifies the node.

**Unique fields:** set `"unique": true` on a field when the user's request implies each instance should have a distinct value for it (e.g. "each ticket should have a unique key" → flag `key` unique). Use `"unique_case_insensitive": true` instead when case shouldn't matter — email and username are the common case. This is advisory only: it does not prevent duplicates from being created, it only lets the system surface a likely existing match when a new value collides. Never describe it to the user as blocking or rejecting duplicates — it's a suggestion, not an enforced constraint. Example: `{"name":"key","type":"text","unique_case_insensitive":true}`.
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

**Output:** `list` prints a table (or the full list with `--json`); the other commands print the affected database record (`--json` emits the full `DatabaseInfo`).

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

### Complete command surface

<!-- BEGIN GENERATED: cli-surface (see packages/cli/src/lib.rs (clap derive), packages/cli/examples/gen_skill_md.rs) -->
Every command, subcommand, and flag below is generated from the CLI's own definitions, so this list is exhaustive and cannot fall behind the binary.

**Global flags** (accepted on every command):

- `--json` — Emit raw JSON instead of human-readable output
- `--socket <SOCKET>` — Override the socket path. With no flag and no environment variable, the CLI dials ~/.nodespace/daemon.sock, or auto-discovers a running dev/Pro daemon's socket if that one is absent. Honors the `NODESPACED_SOCKET` environment variable when this flag is absent (env: `NODESPACED_SOCKET`)
- `--database <DATABASE>` — Target a specific local database by name or id (ADR-053). When omitted, requests route to the daemon's default database. Honors the `NODESPACE_DATABASE` environment variable when this flag is absent (env: `NODESPACE_DATABASE`)

### `nodespace node`

Operate on individual nodes (get, create, update, delete, children, query, export, batch-get, batch-update)

**`nodespace node get`** — Retrieve a node by ID

- `<ID>` — Node ID (UUID) (required)

**`nodespace node create`** — Create a new node

- `--type <NODE_TYPE>` — Node type, e.g. `text`, `task`, `date` (required)
- `--content <CONTENT>` — Content (plain text or markdown) (required)
- `--parent <PARENT>` — Parent node ID (omit to create a root node)
- `--collection <PATH>` — Collection path to file the node under, `:`-delimited for hierarchy (e.g. `docs:rust`) — the same syntax `import` and `search` take. Missing segments are created. Repeatable to join several collections in one call. Mutually exclusive with --collection-id
- `--collection-id <ID>` — Collection ID to file the node under (repeatable). Prefer --collection, which takes a readable path and needs no lookup

**`nodespace node update`** — Update an existing node's content and/or properties

- `<ID>` — Node ID to update (required)
- `--content <CONTENT>` — New content. Omit to leave content unchanged (e.g. when only setting properties)
- `--property <PROPERTIES>` — Set one or more properties: `--property key=value` (repeatable). Values are parsed as JSON when possible (numbers, booleans, `null`, arrays, objects), otherwise treated as a plain string. Deep-merged into the node's existing properties (unspecified keys are left untouched). Do NOT use this to change a task's status; use `node set-status` instead
- `--collection <PATH>` — Collection path to add the node to, `:`-delimited for hierarchy (e.g. `docs:rust`). Missing segments are created. Repeatable. Mutually exclusive with --collection-id
- `--collection-id <ID>` — Collection ID to add the node to (repeatable). Prefer --collection
- `--remove-collection-id <ID>` — Collection ID to remove the node from (repeatable)

**`nodespace node set-status`** — Set a task node's status (dedicated verb — do not use `update` for this)

- `<ID>` — Task node ID (required)
- `<STATUS>` — New status. Must be one of: open, in_progress, done, cancelled (required)

**`nodespace node delete`** — Delete a node

- `<ID>` — Node ID to delete (required)

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

**`nodespace model load`** — Load a model (downloading first if needed); streams progress to stdout

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
- `--filters <FILTERS>` — JSON array of filter conditions, e.g. `[{"type":"property","operator":"equals","property":"status","value":"open"}]`. Supported types: property, content, relationship, metadata. Supported operators: equals, contains, gt, lt, gte, lte, in, exists
- `--sorting <SORTING>` — JSON array of sort configs, e.g. `[{"field":"due_date","direction":"desc"}]`
- `--limit <LIMIT>` — Max results to return (0 = server default of 50)

### `nodespace diagnostics`

Developer diagnostics: database path, size, node counts, schema count

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

- `--params <PARAMS>` — JSON params. For `create`: {"name", "description"?, "fields"?, "relationships"?, "title_template"?, ...} — see CreateSchemaParams. For `update`: {"schema_id", "add_fields"?, "remove_fields"?, "rename_fields"?, "add_relationships"?, "remove_relationships"?, ...} — see UpdateSchemaParams. Mutually exclusive with `--params-file`
- `--params-file <PARAMS_FILE>` — Path to a file containing the JSON params (alternative to inline `--params`)

**`nodespace schema update`** — Update an existing schema from a JSON params blob

- `--params <PARAMS>` — JSON params. For `create`: {"name", "description"?, "fields"?, "relationships"?, "title_template"?, ...} — see CreateSchemaParams. For `update`: {"schema_id", "add_fields"?, "remove_fields"?, "rename_fields"?, "add_relationships"?, "remove_relationships"?, ...} — see UpdateSchemaParams. Mutually exclusive with `--params-file`
- `--params-file <PARAMS_FILE>` — Path to a file containing the JSON params (alternative to inline `--params`)

**`nodespace schema delete`** — Delete a schema definition by ID

- `<ID>` — Schema ID to delete (node type identifier, e.g. `adr`, `person`) (required)

### `nodespace relationship`

Manage typed relationship edges between nodes (distinct from mentions)

**`nodespace relationship create`** — Create a typed relationship edge from one node to another

- `--from <FROM>` — Source node ID (required)
- `--type <RELATIONSHIP_NAME>` — Relationship name (as defined on the source node's schema) (required)
- `--to <TO>` — Target node ID (required)
- `--edge-data <EDGE_DATA>` — Optional JSON-encoded edge properties

**`nodespace relationship get`** — List nodes related to a given node via a named relationship

- `<ID>` — Node ID to query relationships for (required)
- `--type <RELATIONSHIP_NAME>` — Relationship name (as defined on the node's schema) (required)
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

Install, remove, or check the NodeSpace skill for detected AI-agent harnesses (Claude Code, Codex, Gemini CLI, OpenCode) -- the CLI-only equivalent of the desktop app's first-launch skill installer

**`nodespace skill install`** — Detect AI-agent harnesses and install the NodeSpace skill into them. Safe to re-run: already-installed harnesses are left alone, and a harness installed since the last run is picked up

- `--yes` — Install without prompting for confirmation. Implied automatically when stdin/stdout isn't a terminal (CI, a script, an agent's non-interactive shell) — mirrors install.sh's `--gui`/`--no-gui` no-TTY default: never hang waiting on a prompt that can't be answered

**`nodespace skill uninstall`** — Remove the NodeSpace skill from detected (or specified) harnesses

**`nodespace skill status`** — Report which harnesses currently have the skill installed

### `nodespace mcp`

Host a stdio MCP server exposing one passthrough tool, for bash-less MCP surfaces (e.g. Claude Desktop's Chat tab) that cannot shell this CLI directly — see `commands::mcp` for the architecture and its ADR-038 trust-boundary controls. Disabled until `nodespace mcp install` explicitly turns it on. With no subcommand, hosts the stdio server itself — what a client config launches, not something a person types directly

**`nodespace mcp install`** — Configure a detected bash-less MCP client (currently Claude Desktop) to launch `nodespace mcp`, and enable the passthrough tool. Safe to re-run

- `--yes` — Install without prompting for confirmation. Implied automatically when stdin/stdout isn't a terminal — mirrors `nodespace skill install`'s `--yes`

**`nodespace mcp uninstall`** — Remove the MCP config this wrote from every detected client and disable the passthrough tool again

**`nodespace mcp status`** — Report whether the passthrough tool is enabled and which clients currently have a config pointing at it

<!-- END GENERATED: cli-surface -->
