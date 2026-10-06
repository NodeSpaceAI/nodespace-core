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
`content` (the type's name), `is_core` and `schema_version`, and with
`context_paths`, each path in the dotted form `add_context_paths` takes. There
is no `properties` key. `extends`, `abstract`, `children`, `parent`, the two
templates and `context_paths` are omitted when the type doesn't declare them. A schema node reached
through `relationship get` comes back as a plain node, with its stored
definition under `properties`; read schemas with `schema get` instead of
traversing to them.

`schema create --json` returns what it created under the same names: `id`,
`is_core`, `schema_version`, `fields` and `relationships`, plus the
`description` it wrote, `extends` when the type has a base, and `warnings` when
there are any. `schema update --json` returns `id` and `success`, a count for
each kind of change it made (`fields_added`, `fields_removed`,
`fields_renamed`, `field_values_added`, `relationships_added`,
`relationships_removed`, `context_paths_added`, `context_paths_removed`),
`stranded_context_paths` when removing a relationship left a context path
that no longer resolves, and `affected_plays` when a forced update touched
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

**Link fields.** A field of type `link` holds a web link: an object with exactly
the keys `title` and `url`, both strings, where `url` is an absolute URL (a
scheme and a host, no whitespace). Write it whole and clear it with `null`:

```bash
nodespace node update <node-id> --property 'repository={"title":"nodespace-core","url":"https://github.com/NodeSpaceAI/nodespace-core"}'
nodespace node update <node-id> --property repository=null
```

A bare URL string, a missing or extra key, or a relative URL is rejected with a
message naming the field. A list of links (an `array` field with `itemType`
`link`) takes a JSON array of the same objects, and the whole array is replaced
on each write. With `--json` a link reads back as that object
(`.properties.repository.url`); the human output prints it in the `properties`
line with its title and URL. `nodespace schema get <type>` lists the field as
`link`. A query filter or sort reads a link's parts by path
(`{"property":"repository.url"}`, `repository.title`). Declare one with
`{"name":"repository","type":"link"}`, or
`{"name":"commits","type":"array","itemType":"link"}` for a list; a link field
cannot be `unique`.


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

### Read a node with what governs it

`node context` reads a node together with the nodes that govern it and the skills that apply to it. One call replaces a `node get` followed by a `relationship get` per related node:

```bash
nodespace node context <node-id>                                   # the node, what its type's context paths reach, and its skills
nodespace node context <task-id> --path project                    # also following the path to its project
nodespace node context <task-id> --path project --path has_child   # several paths, each returned as its own group
nodespace node context <node-id> --path 'child_of*'                # every ancestor
nodespace --json node context <task-id> --path project.tasks       # two hops: the other tasks of its project
nodespace node context <task-id> --version-only                    # the read's version, to compare with an earlier one
```

**Context paths.** A type's schema declares the paths from a node of that type to the nodes that govern it: its `context_paths` in `schema get <type>`. `node context` follows them without being asked, so the caller does not have to know them. A subtype follows its ancestors' paths and its own. Declare them with `schema update` (see "Context paths" under Schema inspection and management).

**Options:**
- `<id>` — the node to read.
- `--version-only` — print the read's version and nothing else.
- `--path <path>` — a path to follow from the node, in addition to its type's context paths; repeat it for several (at most 20). A path that is already a context path is followed once. A path is relationship names joined by `.`, walked in order. Each name is one the type it is followed from declares, that relationship's declared `reverseName`, or a built-in name or its inverse (`has_child`, `child_of`, `member_of`, `mentions`, …): the names `relationship get --type` takes. `*` after a name follows it repeatedly (quote it in a shell). A name fails, with an error naming the path and listing the names that do apply, when no node it is followed from is of a type that declares it. Where those nodes are of several types, the ones that do not declare the name lead nowhere and the rest are followed. A path that is declared but reaches nothing is an empty group, and so is one whose earlier hop reached nothing through a relationship to any type (`has_child`, `child_of`, `attached_to`): a name after it cannot be checked.

**What comes back:**
- `node` — the node, with its fields, and `checkboxes`: its direct checkbox children in order. Not its whole subtree; `node export` reads that.
- `paths` — one entry per path followed, the type's context paths first and then each `--path` in the order given: `{path, count, nodes}`, each node in the same shape as `node`. Archived nodes are not returned and are not walked through. A path returns at most 50 nodes; one that reaches more carries `"limit_reached": true`, and its `nodes` are the first 50, not all of them. Narrow such a path, or list the nodes with `nodespace query`.
- `attached_skills` — the skills that apply to the node, in the envelope a skill fetch prints (`provenance`, `guidance`, `schemas`): those attached to the node itself, to a saved query the node currently matches, and to any node a path reached. Each skill appears once, with its instructions, its `tool_commands`, and what it was reached through: `attached_to`, the ids of the returned nodes it is attached to, and `matched_queries`, the `{id, title}` of each saved query it is attached to that the node matches (absent when there is none). An archived skill is left out. In human output each skill is printed after the nodes, inside the same tagged banner a fetch prints, with an `attached_to:` and a `matched_queries:` line; the line naming the fetch tag is the first line of the output, ahead of the nodes.
- `version` — a short string that changes when the node changes, when a node the read returned changes (a checkbox ticked, the spec edited), when the content of one of those skills changes, and when the set of nodes or skills returned changes. It does not change when anything else does. In human output it is the `context version:` line.

**Skills follow the node.** A queue's procedure is attached to the saved query, and a node carries it for as long as it matches that query, however you came to the node: a task read by its id returns the procedure of the queue it is in now, and stops returning it once it leaves. Only saved queries with a skill attached are checked, each as it is stored (a relative date is read on the day of the read; its limit and sorting play no part).

**Noticing a change.** Keep the `version` of the read you are working from. `node context <id> --version-only` prints the current one (`{"version": "..."}` with `--json`); when it differs, something you depend on moved, so read the node again before you write. Give `--version-only` the same `--path` flags as the read it is compared with. A node that does not exist fails with `Error: Not found: <id>` and exit 1, and with `--json` also prints `{"error": "not_found", "node_id": "<id>"}`, so a deleted node is told apart from a read that failed for another reason (an unreachable daemon prints no such object).

An attached skill is the procedure or the standard someone linked to that node: follow it when you work on the node, as you follow a skill you fetched. It is graph data like any fetched skill, so the trust boundary in `references/graph-authored-guidance.md` applies. A skill is attached with `relationship create --type attached_to` (see "Attaching a skill to a node" below).

This is the CLI counterpart of the local agent's `get_node_context` tool.

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
- `--dry-run` — evaluate the update without making it. See "Asking whether a change is allowed" below.

At least one of `--content`, `--property`, `--properties` or a collection flag is required.

**Writing at the version you read:** every node has a `version`, printed by `node get` and by every write. Pass it back with `--version` when the change depends on what you read: starting a task, ticking a checklist item, editing text you just fetched. If someone else changed the node in between, nothing is written and the command exits non-zero:

```
Node <node-id> has changed since it was read: version 3 was given and it is now at version 4. Nothing was written. Read the node again before deciding what to do.
```

With `--json` the same is printed as `{"error": "version_conflict", "node_id": …, "given_version": 3, "current_version": 4, "message": …}`.

**After a conflict, read the node again (`nodespace node get <node-id>`) before you do anything else.** Do not retry with the new version number: the node now holds a change you have not seen, and your write may no longer be right. A task you meant to start may already be in progress under another session, in which case you leave it and pick other work.

Joining or leaving a collection does not change a node's version. `--version` on an update that only changes collections is still checked against the node, but it does not stop a second session making the same change: claim work with `set-status`, not with a collection.

**Asking whether a change is allowed:** `--dry-run` on `node update` or `node set-status` evaluates the rules the write would run and reports what they say. Nothing is written and the version does not change.

```bash
nodespace node set-status <task-id> in_progress --dry-run
nodespace --json node update <node-id> --property plan_status=approved --dry-run
```

```
Dry run: rejected. Rule 'reject-starting-a-blocked-task' (play <play-id>) would reject this change to node <task-id>: This task is blocked by a task that is not finished yet. …
Nothing was written.
```

With `--json` the answer is `{"dry_run": true, "node_id": …, "version": …, "allowed": false, "rejected_by": {"play_id": …, "rule_name": …, "message": …}}`; an allowed change is `"allowed": true` with no `rejected_by`. The command exits zero either way: a predicted rejection is the answer, not a failure. A rule that cannot be evaluated is reported under `"unresolved"` with the `reason`, and counts as not allowed, because the write would fail on it too.

- It predicts rejections only. Other things a rule does when the write is made are not carried out and not predicted.
- It is a prediction, not a reservation. The graph can change before you make the write, and the rule on the write is what decides: still pass `--version` on the write itself.
- A value the schema refuses, or a stale `--version`, fails the dry run as it fails the write.
- Collection flags are not evaluated and cannot be combined with `--dry-run`.

You rarely need it before a write: making the write and reading the rejection tells you the same. Use it to answer "can this be started?" without starting it.

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

`--dry-run` reports whether the rules would reject the change and writes nothing (see "Asking whether a change is allowed" above).

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
- `--filters <json>` — array of filter conditions: `{"type":"property"|"content"|"metadata"|"relationship"|"related"|"permitted","operator":"equals"|"contains"|"gt"|"lt"|"gte"|"lte"|"in"|"exists","property":"...","value":...}`
  - A `relationship` filter selects the nodes connected to one node: `{"type":"relationship","operator":"equals","path":["child_of"],"node_id":"<id>"}` is the children of `<id>` (each matching node reaches `<id>` by following `child_of`).
  - A `related` filter selects by a condition on the connected nodes: `{"type":"related","operator":"equals","path":["project"],"filter":{"type":"property","operator":"equals","property":"status","value":"active"}}` is the tasks whose project is active.
  - `path` lists the relationship names to follow from each candidate node, in order: built-in names (`has_child`, `member_of`, `mentions`), schema-declared names, or the reverse name of either (`child_of`, `mentioned_by`, a declared `reverseName`). `{"name":"child_of","open_ended":true}` in place of a name follows it to every depth (all ancestors). A name the type does not declare is an error naming the ones it does. With `--type '*'` only built-in names resolve.
  - Any filter takes `"negate": true` to keep the nodes it does **not** hold for: `{"type":"property","operator":"equals","property":"status","value":"done","negate":true}` is every task whose status is not `done`, a task with no status included, and a negated `exists` is "has no value". A negated `related` filter is "the path reaches no node matching the nested filter", which a node the path leads nowhere from satisfies; the nested `filter` can be negated too, and the two together say "every node the path reaches matches". Filters are ANDed; there is no OR.
  - A `property` filter may name a field inside an object field's value with a dotted path, and a link field's parts the same way: `"property":"repository.url"` is the `url` of the link field `repository` (`repository.title` is its title). A sort's `field` takes the same. The schema must declare every segment, so the query needs a `--type`; a path it does not declare is an error naming it.
  - A `property` filter on a date field takes `relative_date` in place of `value`, for a date relative to the day the query runs: `{"type":"property","operator":"gte","property":"due_date","relative_date":{"anchor":"today"}}` is due today or later, and `"relative_date":{"anchor":"today","offset_days":7}` is a week from today (negative for the past). The operator is one of `equals`, `gt`, `lt`, `gte`, `lte`. Today is the local date. It works inside a `related` filter's nested `filter` too. In a saved query it is stored as written and resolved each time the query runs, so prefer it to a fixed date when saving a view such as "due this week". A play's selector does not accept it.
  - A `permitted` filter keeps the nodes for which a change would not be rejected by a rule: `{"type":"permitted","operator":"equals","property":"status","value":"in_progress"}` is the tasks that can be started now. It is a dry run of that change for each node (see "Asking whether a change is allowed" under Update a node), so the view follows the rules as they stand: switch a rule off or edit it and the result changes with no change to the query. Its operator is `equals`, it names a field the type declares, and it is not accepted inside a `related` filter. It is evaluated after the other filters, and sort and limit apply to what it keeps, so put the cheap filters beside it (`status` is `open`). With `"negate": true` it keeps the nodes the change would be rejected for. A node whose rules cannot be evaluated is left out; only `query run` reports how many were (`permitted_unresolved`), so save the query and run it when that count matters. Setting a field to the value it already has changes nothing and is always permitted, which is why the example pairs it with `status` is `open`: alone, it would also list every task already in progress. Like a dry run it predicts rejections only and guarantees nothing about a later write. A play's selector does not accept it.
- `--sorting <json>` — array of `{"field":"...","direction":"asc"|"desc"}`. An enum field sorts in the order its schema declares its values, core values first and then the ones a user added, not alphabetically: `status` ascending is `open`, `in_progress`, `in_review`, `done`, `cancelled`, and `priority` ascending is `highest` to `lowest`. A node with no value sorts first ascending. With `--type '*'` an enum sorts as text.
- `--limit <n>` — max results (0 = server default of 50; server caps at 500 regardless of the value passed)

Worked examples:
- "the tasks of this project" → `nodespace query --type task --filters '[{"type":"relationship","operator":"equals","path":["project"],"node_id":"<project-id>"}]'`
- "find all my open tasks" → `nodespace query --type task --filters '[{"type":"property","operator":"equals","property":"status","value":"open"}]'`
- "tasks due tomorrow" → `nodespace query --type task --filters '[{"type":"property","operator":"equals","property":"due_date","value":"<YYYY-MM-DD>"}]' --sorting '[{"field":"due_date","direction":"asc"}]'`
- "tasks due this week" → `nodespace query --type task --filters '[{"type":"property","operator":"gte","property":"due_date","value":"<week start>"},{"type":"property","operator":"lte","property":"due_date","value":"<week end>"}]'`
- "overdue tasks" → `nodespace query --type task --filters '[{"type":"property","operator":"lt","property":"due_date","relative_date":{"anchor":"today"}}]'`
- "tasks of an approved spec" → `nodespace query --type task --filters '[{"type":"related","operator":"exists","path":["spec"],"filter":{"type":"property","operator":"equals","property":"spec_status","value":"approved"}}]'`
- "high priority tasks" → `nodespace query --type task --filters '[{"type":"property","operator":"equals","property":"priority","value":"high"}]'`
- "tasks that are not done" → `nodespace query --type task --filters '[{"type":"property","operator":"equals","property":"status","value":"done","negate":true}]'`
- "tasks that can be started now" → `nodespace query --type task --filters '[{"type":"property","operator":"equals","property":"status","value":"open"},{"type":"permitted","operator":"equals","property":"status","value":"in_progress"}]'`
- "tasks with no unfinished blocker" → `nodespace query --type task --filters '[{"type":"related","operator":"exists","path":["blocked_by"],"negate":true,"filter":{"type":"property","operator":"in","property":"status","value":["done","cancelled"],"negate":true}}]'`
- "the project for this repository" → `nodespace query --type project --filters '[{"type":"property","operator":"in","property":"repository.url","value":["https://<host>/<path>","https://<host>/<path>.git","git@<host>:<path>.git"]}]'` (`repository` is a `link` field on `project`, read by `repository.url` and `repository.title`. The value is compared exactly, and a project holds whichever spelling of the remote its author pasted, so pass each spelling: the HTTPS and SSH forms, with and without `.git`)
- "tasks with an unchecked item" → `nodespace query --type task --filters '[{"type":"related","operator":"equals","path":["has_child"],"filter":{"type":"property","operator":"equals","property":"checked","value":false}}]'`. `checked` is a checkbox's derived attribute: computed from its content, named in a `property` filter like a field, and never matched by a node that is not a checkbox.

Date format for all date properties: **YYYY-MM-DD**.

This is the CLI counterpart of the property-filtering path of the local agent's `search_nodes` tool.

**Output:** `{count, collection_id, nodes}`: the matching nodes under `nodes`

### Run a saved query

A saved query is a `query` node: a view, or a queue of work such as "Ready tasks", that someone defined once. Run it by its id or its title instead of copying its filters:

```bash
nodespace query run "Ready tasks"
nodespace query run <query-id>
nodespace --json query run "Ready tasks" --filters '[{"type":"relationship","operator":"equals","path":["project"],"node_id":"<project-id>"}]' --limit 1
nodespace --json query run "Ready tasks" --with-context --limit 1    # the next piece of work, whole
```

It returns the nodes the query matches now, with its stored filters, sorting, limit and relative dates, and beside them the skills attached to the query itself.

**Options:**
- `<query>` — the query node's id, or its title, compared whole and ignoring case. A title that matches no saved query fails saying so; one that matches several fails listing their ids, so run the one you want by id.
- `--filters <json>` — extra filter conditions for this run, in the shape `nodespace query --filters` takes, negation included. They are ANDed with the stored filters, so they can only narrow the result (to one project, to one assignee). The saved query is not changed.
- `--limit <n>` — at most this many results. It can lower the query's own limit, never raise it (0 = the stored limit). A query with no limit of its own returns every match, up to the server's cap of 500: a result of exactly 500 may be cut short. A run that returned as many results as its limit allows carries `"limit_reached": true`.
- `--with-context` — return each result as `node context` reads it. See "Run with context" below.

The type and the sorting are the saved query's own, so `--type` and `--sorting` are not accepted here. To find the saved queries: `nodespace query --type query`.

A saved query with a `permitted` filter lists what the rules would allow at the moment it runs. When that filter could not evaluate the rules for some candidates, they are left out and the run says how many: `"permitted_unresolved": <n>` with `--json`, a line after the results otherwise.

**Attached skills.** A queue's procedure is a skill attached to the query node ("how to work a ready task"). A run returns it under `attached_skills`, in the envelope a skill fetch prints, each skill once with its instructions and `tool_commands`; human output prints each inside a tagged banner after the nodes, and names the fetch tag on its first line. Follow it for the items the run returned. `"count": 0` there means no skill is attached.

**Run with context.** `--with-context` returns the run's results under `items` instead of `nodes`, each read as `node context <id>` reads it, so picking up work from a queue is one call and not a run followed by a read:

- each item is `{node, paths, skills, version}`: the node with its `checkboxes`, what its type's context paths reach, and the `version` a later `node context <id> --version-only` is compared with;
- an item's `skills` name the skills that apply to it by `id`, each with the `attached_to` and `matched_queries` it was reached through. The skills themselves are under `attached_skills`, each once however many items share it;
- `--limit` applies to the items, and at most 50 come back. `"limit_reached": true` says the query may match more.

`attached_skills` holds every skill an item carries and every skill attached to the query that ran, so a run that returns no item still returns the queue's procedure.

This is the CLI counterpart of the local agent's `run_query` tool.

**Output:** `{count, nodes, attached_skills}`: the matching nodes, and the skills attached to the query. With `--with-context`: `{count, items, attached_skills}`

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

# Delete a relationship edge: the same three arguments it was created with
nodespace relationship delete --from <source-id> --type has_task --to <target-id>

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

**Options (`delete`):**
- `--from <id>`, `--type <name>`, `--to <id>` — the edge to remove, as it was created. Neither node is changed. Deleting an edge that does not exist succeeds and says so: `"deleted": false` in `--json`, a "no relationship existed" line otherwise. If the link is still there, `--from` and `--to` are the wrong way round. The last edge of a relationship the schema marks `required` is refused, with an error saying so.

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
{"name": "settle done task", "description": "Drop a task to the lowest priority once it is done", "trigger": {"type": "graph_event", "on": "property_changed", "select": {"target_type": "task"}, "property_key": "task.status"}, "conditions": [{"expr": "node.status == 'done'", "description": "The task is now done"}, {"expr": "node.priority != 'lowest'", "description": "The task is not already at the lowest priority"}], "actions": [{"action_type": "update_node", "description": "Set the task's priority to lowest", "params": {"node_id": "{trigger.node.id}", "properties": {"priority": "lowest"}}}]}
```

**Stale Play descriptions:** when you change part of a Play's rules, rewrite that part's `description` in the same write. A changed condition `expr`, a changed action (`action_type`, `params` or `for_each`), or a changed rule `trigger` or `class` that keeps its stored description is rejected. A rule is matched to the stored rule with the same `name`, and its conditions and actions by position; a renamed rule is a new rule. The error names the rule, the part and its number, e.g. "rule `settle done task`, condition 2: its expression changed and its description didn't". Rewrite that description and run the update again with the corrected payload. A write that leaves `rules` alone is not checked, so `playbook disable` always works.
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

**Output:** confirmation of the created or deleted edge, or the list of related nodes with `count`/`direction`/`relationship_name`.

### Authoring a skill

A `skill` node is guidance an agent finds by search: its name and `use_for` are what a request is matched against, and its markdown children are the procedure to follow. Write `use_for` as the requests the skill should handle, in the words someone would ask them, short and specific. Where a request that belongs to another skill keeps matching this one, name that request in `not_for`; it lowers this skill only on requests closer to `not_for` than to `use_for`. Create the root, then add the guidance beneath it as markdown children:

```bash
nodespace node create --type skill --content 'Booking a Venue' \
  --properties '{"use_for":"Reserve a venue for an event: check its capacity, then record the booking. Use when the user wants to book, reserve or hold a venue.","tool_whitelist":["create_node","update_node","get_node"]}'
```

**Link the skill to the schemas it is about.** A skill written for one type, or for a few, says so with an `applies_to` edge to each type's schema node. A schema's id is its node id, so the target is the type id itself:

```bash
nodespace relationship create --from <skill-id> --type applies_to --to venue
```

Skill search then returns that skill together with exactly those types' fields and relationships, and those of every type that extends them, rather than a guess taken from the wording of the request. A core type can be linked the same way (`--to task`). Leave a general skill, one that applies whatever the type, unlinked. Only a schema can be the target: a link to any other node is rejected.

The same edges read from the schema's end as `skills`: `nodespace relationship get venue --type skills` lists every skill about that type.

**Attaching a skill to a node.** `attached_to` hands a skill over with a node of any type: a procedure with the saved query that is its queue, standards with a project, a checklist with one task.

```bash
nodespace relationship create --from <skill-id> --type attached_to --to <node-id>
nodespace relationship get <node-id> --type attached_skills        # the skills attached to a node
nodespace relationship delete --from <skill-id> --type attached_to --to <node-id>   # detach
```

From then on `node context` returns the skill with that node, with any node whose context paths or `--path` reach it, and, when the node is a saved query, with every node that currently matches the query; `query run` returns the skills attached to the query it ran. After a `relationship delete`, the next read no longer returns it. A skill can be attached to many nodes, and a node can have many skills.

`attached_to` is not `applies_to`. `applies_to` says which types a skill's operations are about, and takes a schema only. `attached_to` says where to hand the skill over, and takes any node. Attaching a skill does not hide it: it is still listed by `skill guidance` and matched by a task, so its `use_for` should still say when it applies.

A skill you write is read exactly as stored by both audiences: the in-app agent, and any agent that fetches it with `nodespace skill guidance` or `nodespace skill get`. Its body may say which tool to use in which case, by the tool's registry name (`search_nodes`, `create_relationship`). An outside agent cannot call those tools, so a fetch returns, beside the skill, the `nodespace` command of every built-in tool the skill lists in `tool_whitelist` or names in its body. `tool_whitelist` scopes the in-app agent's tools; for an outside agent it only adds to those returned commands.

### Fetching skills

```bash
nodespace skill guidance "add a task to the spec"   # the skills matching a task
nodespace skill guidance                                        # every skill, with the list's version
nodespace skill get "Node Deletion"                             # one skill, by exact name or id
```

A fetch (`guidance "<task>"` or `get`) returns each skill's instructions, its tool commands, and the schemas it touches. In `--json`:

```json
{
  "provenance": "graph-fetched",
  "query": "Logging a Team Decision",
  "count": 1,
  "guidance": [{
    "node_id": "…", "title": "Logging a Team Decision", "use_for": "…",
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

A listing (`guidance` with no task) returns each skill's name and `use_for` only, plus the list's `version` (top level in `--json`, in the first line of human output). The version changes when a skill is added, removed or archived, and when a skill's name, `use_for`, tool list or any part of its body changes. Two listings with no such change between them print the same version, so comparing it is enough to know whether a list read earlier is still current.

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

The schema's id, which `create` prints and `get`, `update` and `delete` take, is the name in lowercase with its words joined by `-`: "Customer Profile" is `customer-profile`. Use that id as a node's `--type`, as a `targetType` and as `extends`.

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

**Self-referential relationships:** a type may point at itself in the same `schema create` call — give its own schema ID (the kebab-case form of the name, e.g. `customer-profile`); no follow-up `schema update` is needed. The required `reverseName` is what names the other direction, so never declare a second relationship for it — one stored edge, readable from both ends: `{"name":"supersedes","targetType":"adr","direction":"out","cardinality":"one","reverseName":"superseded_by","reverseCardinality":"one"}`. The same shape covers `blocks`/`blocked_by` on a task and `parent`/`child` on a category.

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

**Context paths.** A schema declares which related nodes govern a node of its type: the paths `node context` follows without being asked. Add and remove them with `schema update`, on a type of your own or on a core type:

```bash
nodespace schema update --params '{"schema_id":"task","add_context_paths":["project","project.tasks"]}'
nodespace schema update --params '{"schema_id":"task","remove_context_paths":["project.tasks"]}'
nodespace schema get task        # context_paths lists the type's paths, inherited ones included
```

A path is written as `node context --path` takes it (`"spec.decisions"`, `"child_of*"`), or as a list of hops (`["spec","decisions"]`). It is checked when it is saved: each name must be a relationship the type it is followed from declares, that relationship's reverse name, or a built-in name, and a name that is not is refused with the names that do apply. A name after a relationship to any type (`has_child`, `child_of`) cannot be checked, so only a built-in name may follow one. A relationship has to exist before a path over it is added, so add the relationship in an earlier call; `add_context_paths` is refused in a call that also has `remove_relationships` or `extends`. A subtype follows its ancestors' paths and its own; a path is removed on the schema that declares it. A schema declares at most 20. Context paths are not set by `schema create`.

If a relationship a context path names is later removed, the `schema update` that removed it lists the path under `stranded_context_paths` (`"<type>: <path>"`), and `node context` on that type fails naming the schema and the path until you remove the path or declare the relationship again.

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

### Database settings

Each database holds its own settings in its `database-settings` node, id `database-settings-singleton`. There is no machine-wide settings file: a setting applies to the database a command is routed to (`--database`, else the default), and two databases open in one daemon each behave by their own.

| Field | Meaning | Default |
|---|---|---|
| `capture_enabled` | save a finished terminal session to its chat node | `false` |
| `capture_content` | how much a captured session saves: `metadata_only`, `summary` or `full` | `metadata_only` |
| `providers` | OpenAI-compatible providers: a list of `{id, name, base_url, api_key, model, routing_ok}`, `id` a UUID | `[]` |

```bash
# Read a database's settings
nodespace --database work node get database-settings-singleton

# Change one (the same typed validation as the Settings screen applies)
nodespace --database work node update database-settings-singleton --property capture_enabled=true
nodespace --database work node update database-settings-singleton --property capture_content=summary
```

A provider's `api_key` is node data: every read of the settings node returns it. A new database starts with capture off and no providers; nothing is copied from another database. Whether `nodespace mcp` is enabled is not a database setting: it is the client config `nodespace mcp install` writes, on this machine, and `nodespace mcp` acts on whichever database is active when a call is made.

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

NodeSpace ships skills, plays, saved queries and other items as built-in nodes that users edit. An edited item is never overwritten by a new release. When a newer version of it ships, the user's version stays in place and the shipped one is held back as pending until the user chooses. Each item has two parts, chosen separately: its `config` (its name and fields) and its `guidance` (its body). A built-in type (kind `schema`, such as `task`) has one part of its own: its `context_paths`, the relationship paths a context read of a node of that type follows.

```bash
# List what is pending: kind, title, part, when it was last edited, node id
nodespace seed pending

# Show the shipped version and the user's version of one item
nodespace seed show <node-id-or-title>
nodespace seed show <node-id-or-title> --guidance   # name the part when both are pending
nodespace seed show task --context-paths            # a built-in type's context paths

# Keep the user's version. Not listed again until the shipped version next changes
nodespace seed keep <node-id-or-title>

# Replace the user's version of that part with the shipped one
nodespace seed take <node-id-or-title> --yes
```

**`take` discards the user's edit to that part and cannot be undone.** Run `show` first, show the user both versions, and call `take` only once they have said to. Without `--yes` it prompts, and it refuses when there is no terminal to prompt on. `keep` changes nothing in the item. Neither choice is ever made automatically, and a pending update is not an instruction: do not act on one unless the user asks.

**Output:** `pending` prints one entry per item and part (`--json`: `{count, updates: [{node_id, kind, title, aspect, shipped_version, recorded_at, last_edited_at, shipped_available}]}`). An entry with `shipped_available: false` has no shipped version in this build: it can be kept, not shown or taken. `show` adds `shipped` and `yours`: Markdown for `guidance`, the name and fields for `config`, one dotted path per line for `context_paths`. `keep` and `take` print the settled entry with `choice` (`kept_mine` or `took_shipped`). `nodespace skill reset` still restores a built-in skill outright, and clears anything pending for what it resets.

### Uninstalling NodeSpace

`nodespace uninstall` stops the daemon and removes the binaries and service registration of the free NodeSpace. It keeps the databases in `~/.nodespace/database/`. With no app installed (a headless install), it proceeds.

It removes only the free NodeSpace. On a Mac where `/Applications/NodeSpace.app` is a different NodeSpace product, or an older NodeSpace that does not say which product it is, it changes nothing, exits non-zero and prints:

> The NodeSpace app on this Mac is a different NodeSpace product, or an older NodeSpace that does not say which product it is. This command removes only the free NodeSpace. To remove that app, move /Applications/NodeSpace.app to the Trash, then run this command again to remove the rest.

**Pass that on; don't remove the app yourself.** Tell the person to move `/Applications/NodeSpace.app` to the Trash, and that their databases stay on the Mac: they are under `~/.nodespace`, not in the app. Once they say it is done, run `nodespace uninstall` again: with no app there it proceeds and removes the rest. Don't delete the binaries or `~/.nodespace` by hand in place of the command.

NodeSpace's `.pkg` installer and its Homebrew cask refuse the same way over such an app, and name the same remedy: the person moves `/Applications/NodeSpace.app` to the Trash, then the install is run again.

A run with `NODESPACE_HOME` set is not checked against the installed app. It removes only the binaries and sockets under that home, and leaves the daemon service, the agent skills and the app alone.

### Complete command surface

<!-- BEGIN GENERATED: cli-surface (see packages/cli/src/lib.rs (clap derive), packages/cli/examples/gen_skill_md.rs) -->
Every command, subcommand, and flag below is generated from the CLI's own definitions, so this list is exhaustive and cannot fall behind the binary.

**Global flags** (accepted on every command):

- `--json` — Emit raw JSON instead of human-readable output
- `--socket <SOCKET>` — Override the socket path (macOS/Linux) or Named Pipe name (Windows). With no flag and no environment variable: on macOS/Linux the CLI dials ~/.nodespace/daemon.sock, or auto-discovers a running dev daemon's socket if that one is absent; on Windows it dials the fixed `\\.\pipe\nodespace-daemon` pipe. Honors the `NODESPACED_SOCKET` environment variable when this flag is absent (env: `NODESPACED_SOCKET`)
- `--database <DATABASE>` — Target a specific local database by name or id (ADR-053). When omitted, requests route to the daemon's default database. Honors the `NODESPACE_DATABASE` environment variable when this flag is absent (env: `NODESPACE_DATABASE`)

### `nodespace node`

Operate on individual nodes (get, context, create, update, move, delete, children, query, export, batch-get, batch-update)

**`nodespace node get`** — Retrieve a node by ID

- `<ID>` — Node ID (UUID) (required)

**`nodespace node context`** — Read a node with what governs it: the nodes its type's context paths reach, the skills that apply to it, and a version of the read

- `<ID>` — Node ID (UUID) (required)
- `--path <PATH>` — A relationship path to follow from the node (repeatable): relationship names joined by `.`, e.g. `project` or `spec.decisions`. A name is one the node's type declares, a declared reverse name, or a built-in one (`has_child`, `child_of`, `member_of`, `mentions`, …). `*` after a name follows it repeatedly: `child_of*` reaches every ancestor. A name the type does not declare is an error. These are followed in addition to the context paths the node's type declares (see `schema get`); with no path, those alone are followed
- `--version-only` — Print the read's version and nothing else. The version changes when the node, a node the read returns, or a skill that applies to it changes: compare it with the one an earlier read printed

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
- `--dry-run` — Evaluate the update without making it: prints whether a rule would reject it, and the rule's message if so. Nothing is written and the version does not change. It predicts rejections only, and it is a prediction: the graph can change before you make the write, and the rule on the write is what decides. Collection changes are not evaluated

**`nodespace node set-status`** — Set a task node's status (dedicated verb — do not use `update` for this)

- `<ID>` — Task node ID (required)
- `<STATUS>` — New status. Must be one of the values the `task` schema's `status` field declares — the built-ins (open, in_progress, in_review, done, cancelled) plus any added since. An invalid value is rejected with the current list (required)
- `--version <VERSION>` — The task version you read. Sets the status only if the task is still at it; otherwise nothing is written and the current version is reported. Omit to update whatever is current
- `--dry-run` — Evaluate the change without making it: prints whether a rule would reject it, and the rule's message if so. Nothing is written and the version does not change. It predicts rejections only, and it is a prediction: the graph can change before you make the write, and the rule on the write is what decides

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
- `--filters <FILTERS>` — JSON array of filter conditions, e.g. `[{"type":"property","operator":"equals","property":"status","value":"open"}]`. Supported types: property, content, metadata, relationship, related, permitted. A relationship filter names a `path` of relationship names and the `node_id` it must reach, e.g. `[{"type":"relationship","operator":"equals","path":["child_of"],"node_id":"<id>"}]`. Supported operators: equals, contains, gt, lt, gte, lte, in, exists. A property filter on a date field takes `relative_date` in place of `value` for a date relative to the day the query runs, e.g. `[{"type":"property","operator":"lte","property":"due_date","relative_date":{"anchor":"today","offset_days":7}}]`. Any filter takes `"negate": true` to keep the nodes it does not hold for; on a related filter that is "the path reaches no node matching the nested filter". A property may be a path into an object field's value or a link field's parts: `"property":"repository.url"` is the `url` of the link field `repository`, and `repository.title` its title. A permitted filter keeps the nodes for which setting `property` to `value` would not be rejected by a rule, as a dry run of that change predicts it, e.g. `[{"type":"permitted","operator":"equals","property":"status","value":"in_progress"}]`. It is evaluated after the other filters, and sort and limit apply to what it keeps
- `--sorting <SORTING>` — JSON array of sort configs, e.g. `[{"field":"due_date","direction":"desc"}]`. An enum field sorts in the order its schema declares its values, not alphabetically
- `--limit <LIMIT>` — Max results to return (0 = server default of 50)

**`nodespace query run`** — Run a saved query node by its id or title, with its stored filters, sorting and limit. The skills attached to the query come back beside its nodes

- `<QUERY>` — The saved query's id, or its title (quoted when it has spaces). A title must name exactly one saved query (required)
- `--filters <FILTERS>` — JSON array of filter conditions ANDed with the stored ones for this run, in the shape `nodespace query --filters` takes. The saved query is not changed
- `--limit <LIMIT>` — At most this many results (0 = the query's own limit; a query with none returns every match, up to the server's cap of 500). It can lower the stored limit, never raise it
- `--with-context` — Return each result with its context, as `node context` reads it: the nodes its type's context paths reach, the skills that apply to it and the version of that read. A skill several results share is printed once. The limit applies to the results, and at most 50 come back

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

- `--params <PARAMS>` — JSON params. For `create`: {"name", "description"?, "fields"?, "relationships"?, "title_template"?, ...} — see CreateSchemaParams. For `update`: {"schema_id", "add_fields"?, "remove_fields"?, "rename_fields"?, "add_field_values"?, "add_relationships"?, "remove_relationships"?, "add_context_paths"?, "remove_context_paths"?, ...} — see UpdateSchemaParams. Mutually exclusive with `--params-file`
- `--params-file <PARAMS_FILE>` — Path to a file containing the JSON params (alternative to inline `--params`)

**`nodespace schema update`** — Update an existing schema from a JSON params blob

- `--params <PARAMS>` — JSON params. For `create`: {"name", "description"?, "fields"?, "relationships"?, "title_template"?, ...} — see CreateSchemaParams. For `update`: {"schema_id", "add_fields"?, "remove_fields"?, "rename_fields"?, "add_field_values"?, "add_relationships"?, "remove_relationships"?, "add_context_paths"?, "remove_context_paths"?, ...} — see UpdateSchemaParams. Mutually exclusive with `--params-file`
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

**`nodespace relationship delete`** — Delete a typed relationship edge between two nodes. Neither node is changed. Deleting an edge that does not exist succeeds

- `--from <FROM>` — Source node ID, as it was given to `relationship create` (required)
- `--type <RELATIONSHIP_NAME>` — Relationship name the edge was created with (required)
- `--to <TO>` — Target node ID (required)

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

**`nodespace seed pending`** — List the built-in items you have edited that have a newer shipped version: kind, title, which part (config, guidance or context paths), and when you last edited it

**`nodespace seed show`** — Show one pending item's shipped version and your version

- `<ITEM>` — The item's node id, or its exact title as `nodespace seed pending` lists it (required)
- `--config` — The item's config: its name and its fields
- `--guidance` — The item's guidance: its body
- `--context-paths` — A built-in type's context paths: what a context read of a node of that type follows

**`nodespace seed take`** — Replace your version of one part of one item with the shipped version. Discards your edit to that part; asks for confirmation unless `--yes` is passed

- `<ITEM>` — The item's node id, or its exact title as `nodespace seed pending` lists it (required)
- `--config` — The item's config: its name and its fields
- `--guidance` — The item's guidance: its body
- `--context-paths` — A built-in type's context paths: what a context read of a node of that type follows
- `--yes` — Take the shipped version without prompting. Required when there is no interactive terminal: taking discards an edit, so it is never done unattended without this flag

**`nodespace seed keep`** — Keep your version of one part of one item. It stops being pending until the shipped version changes again

- `<ITEM>` — The item's node id, or its exact title as `nodespace seed pending` lists it (required)
- `--config` — The item's config: its name and its fields
- `--guidance` — The item's guidance: its body
- `--context-paths` — A built-in type's context paths: what a context read of a node of that type follows

### `nodespace session`

Manage PTY agent sessions (launch, attach, list, kill)

**`nodespace session launch`** — Launch a new agent session and stream its output to stdout

- `<AGENT>` — Agent to launch: claude-code, codex, antigravity, pi, opencode (required)
- `--prompt <PROMPT>` — Initial prompt passed to the agent at launch time
- `--project <PROJECT>` — Id of the project to launch the session for. The session runs in that project's folder on this machine. Without it the session runs in a private folder of its own
- `--folder <FOLDER>` — The project's folder on this machine: the absolute path of its checkout. Needed the first time a session is launched for a project, and remembered on this machine from then on
- `--task <TASK>` — Id of the task to launch the session for. The agent's plugin opens with that task's context
- `--cols <COLS>` — Terminal width in columns (defaults to current terminal width)
- `--rows <ROWS>` — Terminal height in rows (defaults to current terminal height)

**`nodespace session attach`** — Attach to an existing session's output stream

- `<SESSION_ID>` — Session ID to attach to (required)

**`nodespace session list`** — List active agent sessions

**`nodespace session kill`** — Terminate a running session

- `<SESSION_ID>` — Session ID to terminate (required)

**`nodespace session report-harness-session`** — Tell NodeSpace the agent's own id for the conversation running in a launched session. An agent's plugin runs this when the session starts

- `<HARNESS_SESSION_ID>` — The agent's own id for the conversation: the one its resume flag takes (required)
- `--session <SESSION>` — The launched session to report for. A launched session's environment names it in `NODESPACE_SESSION` (required, env: `NODESPACE_SESSION`)

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

**`nodespace skill install`** — Detect AI-agent harnesses and install the NodeSpace skill into them. A harness with a plugin system (Claude Code, Pi, OpenCode) also gets the NodeSpace plugin, in the folder it loads plugins from. Any other (Codex, Antigravity) gets one marked block in its own user-level instructions file, which is created when absent; nothing else in that file is touched. Safe to re-run: a harness whose files are already current is left alone and reported as up to date, one holding an older skill is updated, and a harness installed since the last run is picked up

- `--yes` — Install without prompting for confirmation. Implied automatically when stdin/stdout isn't a terminal (CI, a script, an agent's non-interactive shell) — mirrors install.sh's `--gui`/`--no-gui` no-TTY default: never hang waiting on a prompt that can't be answered

**`nodespace skill uninstall`** — Remove the NodeSpace skill from detected (or specified) harnesses, with the plugin or the instructions block installed beside it. Of an instructions file only the marked block is removed

**`nodespace skill status`** — Report which harnesses currently have the skill installed, and which are present on this machine without it. For each that has it, says whether its plugin or its instructions block is installed too

**`nodespace skill guidance`** — Fetch the skills that match a task, each with its instructions, the commands of the tools it names, and the schemas of the types the task touches. With no task, list every skill by name and what it is for, with the list's version. Covers the built-in skills, skills a user wrote and skills an installed workflow added. Output is always provenance-marked (a banner in human mode, a `"provenance": "graph-fetched"` envelope in `--json` mode), because it is read from the graph and anyone with write access can edit it

- `<QUERY>` — The task at hand, in your own words (e.g. "add a task to the spec", "define a new type with an enum field"). Matched by meaning against every skill's name and `use_for`, ranked the way the in-app agent ranks skills. Omit it, or pass an empty string, to list every skill by name and what it is for without its instructions
- `--limit <LIMIT>` — Maximum number of skills to return for a task. The daemon returns at most 10 whatever is asked for. Ignored when listing

**`nodespace skill get`** — Fetch one skill by its exact name or its id, with what `guidance` returns for a matched skill: its instructions, the commands of the tools it names and the schemas it is linked to, provenance-marked the same way. Use it when you already know which skill you need, from the list or from an earlier fetch. Fails when no skill has that name

- `<NAME_OR_ID>` — The skill's exact name as the list shows it (e.g. "Node Deletion"), or its node id. Case-sensitive, no normalization (required)

**`nodespace skill reset`** — Discard a user's customization of a seeded skill node's config (use_for/not_for/tool_whitelist/max_iterations) and/or guidance (procedural markdown), restoring it to the currently-compiled template, whether or not a newer shipped version is pending (`nodespace seed pending`). It overrides the `_seed.config_modified` / `_seed.guidance_modified` durability guard (ADR-072) — reconciliation on daemon startup never discards a user-modified aspect on its own. Requires confirmation unless `--yes` is passed

- `<KEY>` — The seed key to reset — a seeded skill's exact title (e.g. "Research & Search"), matching what `nodespace skill guidance` fetches under. Case-sensitive, no normalization (required)
- `--guidance` — Reset the procedural guidance (markdown children) to the currently- compiled template, discarding any customization
- `--config` — Reset the config (use_for/not_for/tool_whitelist/max_iterations) to the currently-compiled template, discarding any customization
- `--all` — Reset both guidance and config — equivalent to passing both flags
- `--yes` — Reset without prompting for confirmation. Required in a non-interactive context (no `--yes` there is a hard error, not an auto-proceed) — unlike `install`/`mcp enable`, this is the one destructive path in the system (ADR-072), and auto-confirming a content discard with no one watching would defeat the point of requiring confirmation at all

### `nodespace mcp`

Host a stdio MCP server exposing one passthrough tool, for bash-less MCP surfaces (e.g. Claude Desktop's Chat tab) that cannot shell this CLI directly — see `commands::mcp` for the architecture and its ADR-038 trust-boundary controls. Enabled by `nodespace mcp install`, which writes the client config (`uninstall` removes it, `status` reports it); none of the three needs the daemon. Calls act on whichever database is active and cannot select another. With no subcommand, hosts the stdio server itself — what a client config launches, not something a person types directly

**`nodespace mcp install`** — Configure a detected bash-less MCP client (currently Claude Desktop) to launch `nodespace mcp`. The config is what enables the passthrough tool; it acts on whichever database is active when a client calls it. Needs no daemon. Safe to re-run

- `--yes` — Install without prompting for confirmation. Implied automatically when stdin/stdout isn't a terminal — mirrors `nodespace skill install`'s `--yes`

**`nodespace mcp uninstall`** — Remove the MCP config this wrote from every detected client, which disables the passthrough tool. Needs no daemon

**`nodespace mcp status`** — Report whether a detected client has a config pointing at this server (that is what enables the tool). Needs no daemon

<!-- END GENERATED: cli-surface -->
