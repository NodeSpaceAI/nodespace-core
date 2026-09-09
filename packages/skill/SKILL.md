# NodeSpace Skill

NodeSpace is context infrastructure for AI-native development — a local-first knowledge graph running on this machine.

**Repositories contain what was built. NodeSpace contains why it was built, how it should be built, and what agents need to know to build it correctly.** Specs, architecture decisions, ADRs, designs, plans, standards, tasks, and findings live here, not scattered across chat logs and stale docs. Code is the artifact; this is the context behind it.

It persists across sessions — what you save today is searchable tomorrow, and your context window does not survive the turn.

**So: read from NodeSpace before you write code or documents, and write back to it when you decide something.** If you're about to author a spec, an ADR, a design, or a plan — or you need the reasoning and constraints behind existing code — check here first. Whatever the repository cannot tell you about *why*, this can.

## How NodeSpace Thinks (Mental Model)

**Everything is a node.** A node has a type, markdown content, and optional typed properties. Node types are schema-defined: `text`, `task`, and `date` are built-ins; custom types come from user-defined schemas (`nodespace schema list` shows what's registered).

**Built-in node types:**
- `text` — freeform notes, documents, findings, summaries
- `task` — structured to-do items; carry `status` (`open`/`in_progress`/`done`/`cancelled`), `due_date` (YYYY-MM-DD), and `priority` (`low`/`medium`/`high`)
- `date` — daily container nodes (e.g. "2026-05-30"); each day has one. Attach time-sensitive findings under the relevant date node so they're retrievable by day.

**Hierarchy is first-class edges, not nesting.** A node has one parent edge. Children are ordered via fractional ordering — siblings have a stable position without gap-numbering. Moving or reordering a node is an edge operation (change the parent or sibling position), not a recreate-and-delete.

**Relationships are distinct from hierarchy and mentions.** A relationship is a named, schema-defined edge between two nodes (e.g. `billed_to`, `has_task`) — different from the one parent edge and from inline `mention` links captured from markdown content. Relationships must be defined on a schema (via `nodespace schema create`/`nodespace schema update`) before they can be used; `nodespace relationship create` on a node whose schema has no matching relationship name fails.

**Content is markdown.** Store prose, code blocks, lists — whatever fits the note. The export commands render it back as clean markdown.

## When to Use NodeSpace (Session Judgment)

Use NodeSpace as a working memory across sessions:

1. **Search at session start** — run the preflight, then search for prior context before you begin. (`nodespace search "topic"`)
2. **Save as you go** — save discoveries, decisions, and summaries during the session. Don't wait until the end.
3. **It persists across sessions** — your context window does not. Anything worth remembering next time should be stored.

Date nodes make temporal retrieval reliable: if a finding is time-bound, attach it under today's date node so future searches can scope by day.

## Shared Workspaces (Multi-User)

A NodeSpace collection can be synced and shared with a teammate through NodeSpace Pro: the daemon is launched already bound to it, so another engineer — or their agent — reads and writes the same graph. If that is the case here, read **`references/shared-workspaces.md`** before your first write to a shared collection: it covers write visibility, attribution, sync latency for search/recall, and why date-node findings do not sync yet. Skip it entirely for a private, single-user database.

## Preflight Check

**Before starting any multi-step NodeSpace operation**, work out which of three capability branches you're on, then follow that branch. Check capability first, before running anything — the branches below differ in how (or whether) you can run a `nodespace` command at all, so branching on a command's output only works once you already know you have a way to run commands.

**This is a soft inference, not an API call.** There is no "do I have Bash?" check to run — decide from the tools you were actually given this turn:

1. **A Bash/shell tool is available** → "Branch 1: Shell available" below.
2. **No Bash, but a `nodespace` tool is available** (its schema takes one `args: string` parameter — the MCP passthrough) → "Branch 2: No shell, `nodespace` MCP tool available" below.
3. **Neither** → "Branch 3: Neither shell nor MCP tool available" below.

A wrong guess should degrade gracefully, not dead-end: if Branch 1's commands come back as though there's no shell at all, or the `nodespace` tool you expected never appears in your tool list, fall through to the next branch rather than repeating the same failed approach.

**Consent discipline is identical on every branch.** Never run the installer, start the daemon, or delete a node or type without the user's explicit confirmation — the MCP passthrough is not an exception just because it's a tool call instead of a shell line; see "Delete a node" below, which applies unchanged regardless of which branch dispatched it.

### Branch 1: Shell available

Run these two commands to confirm the tooling is present and healthy:

```bash
nodespace --version
nodespace diagnostics
```

Run this preflight once per session or task, not before every individual command.

#### Failure recovery

| Symptom | Cause | Recovery |
|---------|-------|----------|
| `command not found: nodespace` | CLI not installed or not on `$PATH` | Tell the user NodeSpace CLI is not installed and propose installing it — never run the installer without their explicit confirmation. If they confirm, run `sh -c "$(curl -fsSL https://nodespace.ai/install.sh)" -- --no-gui` (installs the CLI only, non-interactively — the same script the one-line install and `brew install --cask nodespaceai/nodespace/nodespace` both use). Then retry the original command. If it still fails because this shell session hasn't picked up the updated `$PATH`, tell the user to open a new terminal and try again. If they decline the install, stop. |
| `Could not connect to nodespaced` | Daemon not running | Surface the CLI's own message to the user: start the daemon with `nodespaced`. Do not retry automatically — wait for confirmation. |
| `diagnostics` shows entries in `errors` | Database issues | Report the specific error messages to the user before continuing. |

### Branch 2: No shell, `nodespace` MCP tool available

There's no shell, but a `nodespace` tool is on your tool list: a passthrough with one `args` parameter — the exact argument list that would follow `nodespace` on a shell line. Every command in this document works verbatim through it, with no separate command set to learn: what Branch 1 runs as `nodespace search "auth tokens"` on a shell line, this branch calls the tool with `args: "search \"auth tokens\""`; `nodespace node get <id>` becomes `args: "node get <id>"`; and so on for every example elsewhere in this file, including "Delete a node" below.

Run the same preflight by calling the tool twice:

```
args: "--version"
args: "diagnostics"
```

The tool's result text carries the underlying CLI's own output, so read it the way you'd read a shell command's output — but you cannot self-heal by running an installer or starting a daemon; you can only tell the user what's wrong.

#### Failure recovery

| Symptom (in the tool result) | Cause | Recovery |
|---------|-------|----------|
| `Failed to run the nodespace CLI at ...` | The CLI binary backing this passthrough is missing or broken | Tell the user NodeSpace needs to be reinstalled — point at the desktop app or `brew install --cask nodespaceai/nodespace/nodespace`. You cannot install it yourself from here; do not propose a command to run. |
| `Could not connect to nodespaced` | Daemon not running | Tell the user to start it with `nodespaced` on the machine hosting this connector — same fix as Branch 1, but you cannot run it yourself. Do not retry automatically. |
| `diagnostics` call (`args: "diagnostics"`) shows entries in `errors` | Database issues | Report the specific error messages to the user before continuing — same as Branch 1. |
| `did not complete within 120s` | The dispatched command streams or blocks (e.g. `session launch`/`session attach`) — this passthrough kills and reports it as a timeout rather than staying open | Do not retry it through this tool. Tell the user this NodeSpace command needs an interactive session and isn't supported through this connector; point them at a shell-capable surface (Branch 1: Claude Code, or Claude Desktop's Code tab) for it. |

### Branch 3: Neither shell nor MCP tool available

NodeSpace is not reachable from this surface. There is no command to run and nothing to propose running — do not attempt a `nodespace` invocation, and do not fabricate or guess at a result. Tell the user plainly that NodeSpace can't be reached from here, and point them at a surface that can: the NodeSpace desktop app, or a shell- or MCP-capable agent harness (e.g. Claude Code, or Claude Desktop's Code tab). Installing a connector is a step the user takes in their own client, not something you can do on their behalf.

## Prerequisites

NodeSpace daemon must be running. The `nodespace` CLI communicates with `nodespaced` over a Unix socket. If the daemon is not running, CLI commands will fail with a connection error.

Start the daemon: `nodespaced` (or it starts automatically on login if installed via DMG).

## Tool Decision Guide

Use this to pick the right command for the task at hand.

### Finding things

| Goal | Command |
|------|---------|
| Find nodes by keywords or meaning | `nodespace search "<query>"` |
| List all nodes of a type | `nodespace search "" --type <type>` |
| Filter by property values (status, due_date, priority, etc.) | `nodespace query --type <type> --filters '<json>'` |
| Filter with comparison operators (gt, lt, gte, lte, in) | `nodespace query --type <type> --filters '<json>'` |
| Exact substring match on content or title | `nodespace node query --content-contains "..."` / `--title-contains "..."` |
| Get a specific node by ID | `nodespace node get <id>` |

**`nodespace node query` is for exact substring/type matching only** (`--content-contains`, `--title-contains`, `--mentioned-by`, `--type`). It has no property-filter flags.

**`nodespace query` is the command for structured property queries** — status, due_date, priority, or any comparison operator. Examples:
- "find all my open tasks" → `nodespace query --type task --filters '[{"type":"property","operator":"equals","property":"status","value":"open"}]'`
- "tasks due tomorrow" → `nodespace query --type task --filters '[{"type":"property","operator":"equals","property":"due_date","value":"<YYYY-MM-DD>"}]' --sorting '[{"field":"due_date","direction":"asc"}]'`
- "tasks due this week" → `nodespace query --type task --filters '[{"type":"property","operator":"gte","property":"due_date","value":"<week start>"},{"type":"property","operator":"lte","property":"due_date","value":"<week end>"}]'`
- "high priority tasks" → `nodespace query --type task --filters '[{"type":"property","operator":"equals","property":"priority","value":"high"}]'`

Date format for all date properties: **YYYY-MM-DD**. Available operators: `equals`, `contains`, `gt`, `lt`, `gte`, `lte`, `in`, `exists`. Filter types: `property`, `content`, `relationship`, `metadata`.

**`nodespace search` is semantic** (embedding-based similarity), ranked by relevance. Pass `--type` to narrow to one or more node types, `--limit` to cap results (default 20), `--include-content` to also read the top 5 hits (a bare result is just its heading). No graph-boost, cross-collection exclusion, or edge-inclusion — fall back to `nodespace query` plus `nodespace relationship get` for those.

**Multiple topics:** run `nodespace search` once per topic rather than one broad search plus per-result fetches.

## CLI Reference

The complete command reference — every command, flag, argument shape, and output
format, with worked examples — is in **`references/cli.md`**. Read that file when
you need exact syntax.

All commands accept `--json` for machine-readable output.

**Selecting a database.** A single daemon can serve several local databases. Data
commands accept a global `--database <name|id>` flag; `NODESPACE_DATABASE` sets
the same target when the flag is absent. Without either, requests go to the
daemon's default database.

```bash
nodespace --database work node create --type text --content "work note"
NODESPACE_DATABASE=work nodespace search "meeting notes"
```

**The schemas on this machine are live data — never assume them.** Node types are
user-defined and differ per database, so read them at the moment you need them
rather than relying on anything written here:

```bash
nodespace schema list --json          # what types exist right now
nodespace schema get <type> --json    # a type's exact fields before writing one
```

## Common Agent Tasks

### Save a note for later

```bash
nodespace node create --type text --content "Key insight: the auth token expires after 1 hour"
```

### Search for previously stored context

```bash
nodespace search "authentication token refresh"
```

### Create a task

```bash
nodespace node create --type task --content "Implement rate limiting on the API gateway"
```

### Change a task's status

```bash
nodespace node set-status <task-id> done
```

### Organize under a parent

```bash
# Create a parent project node
nodespace node create --type text --content "Project: API Redesign"
# → returns {"id": "abc123", ...}

# Add sub-notes under it
nodespace node create --type text --content "Decision: use REST not GraphQL" --parent abc123
```

### Attach a finding to today's date node

Date node IDs are the date string itself (`"2026-05-30"`). Pass the date string directly as `--parent` — the daemon auto-creates the date node if it doesn't exist yet.

```bash
# Attach a finding under today — date node is created automatically if absent
nodespace node create --type text --content "Discovered: rate limiter uses fixed windows" --parent "2026-05-30"

# To retrieve an existing date node directly
nodespace node get "2026-05-30"
```

### Define a new entity type, then create an instance

```bash
# 1. Create the schema (one schema per request; `references/cli.md` covers field/enum/relationship shape)
nodespace schema create --params '{"name":"Ticket","fields":[{"name":"status","type":"text"}]}'

# 2. Create an instance
nodespace node create --type ticket --content "Fix flaky retry test" --parent <parent-id>
nodespace node update <the-new-id> --property status=in_dev
```

### Link two nodes with a typed relationship

```bash
# Relationship must already be defined on the source's schema (e.g. Ticket.belongs_to_sprint)
nodespace relationship create --from <ticket-id> --type belongs_to_sprint --to <sprint-id>
```

### Organize a node into a collection

Collections are how NodeSpace tags and groups things — a flat label and a nested `:`-delimited path (`docs:rust`) are one mechanism at two depths, the same syntax `import` and `search` take. Crucially, **collection membership is an argument to the create call**, not a follow-up write: pass `--collection` and every missing segment is created for you. Never look a collection up first or ask the user to pre-create one.

```bash
# One call: creates the node, creates `docs` and `rust`, files the node under `rust`
nodespace node create --type text --content "Pin tokio to 1.40" --collection docs:rust

# Repeatable, and it works on an existing node too
nodespace node create --type text --content "Retry budget" --collection docs:rust --collection decisions
nodespace node update <node-id> --collection docs:rust
```

A collection costs the same one flag as a single `tags` array element, so prefer it for any durable grouping: don't add a `tags`/`categories`/`topics`/`labels` field to a schema for something collections already model. Unlike an array value, a collection shows in the UI, is renamed once not per member, nests, and needs no schema change to join — `member_of` is structural, legal between any two nodes undeclared.

### Delete a node, or a whole node type

Deletion is permanent and takes the node's children with it. Resolve the node first and confirm with the user before deleting anything you did not just create — a wrong id here is not recoverable.

```bash
nodespace node query --content-contains "draft spec"   # resolve the id first
nodespace node delete <node-id>
```

If the user wants something out of the way rather than gone, prefer moving it (re-parent it, or drop it from a collection) over deleting it.

**A node type can be deleted too** — `nodespace schema delete <type>`, once `schema update` has cleared any relationship declarations pointing at or from it. Asked to remove, drop or clean up a type, including a throwaway one you just created, reach for this; never call it unsupported or strip a schema to an empty shell instead. Sequence in `references/cli.md`.

### Resolve a duplicate or colliding record

```bash
nodespace conflicts list --status open                 # see what's outstanding
nodespace conflicts show <conflict-id>                  # confirm the participants and evidence
nodespace conflicts dismiss <conflict-id>                # acceptable as-is — no node changes
nodespace conflicts adopt <conflict-id> --keep <node-id> # continue with the existing node — no node changes
nodespace conflicts merge --survivor <node-id> --conflict-id <conflict-id>  # combine into one record
```

`merge` is the one irreversible-feeling action here — it archives the losing node and re-points its edges immediately when called. Only call it once the user has confirmed which node should survive; `dismiss` and `adopt` don't touch either node. Full options and output shape in `references/cli.md`.

### Bulk import from markdown

```bash
nodespace import file ./notes.md
nodespace import dir ./docs ./adr --auto-collection-routing
```

Top-level headings become root nodes, sub-headings become children — for any multi-section document, not only bulk import. Report the number of nodes created; don't follow up with search calls to verify.

### Export a document for AI context

```bash
# Export with OCC tokens so AI can update individual nodes
nodespace node export <doc-id> --json | jq '.markdown'

# Clean export for reading
nodespace node export <doc-id> --node-ids false
```

## Output Format

All `--json` commands output to stdout. Errors are written to stderr with a non-zero exit code.

```json
{
  "id": "550e8400-e29b-41d4-a716-446655440000",
  "node_type": "task",
  "content": "Buy groceries",
  "parent_id": null,
  "properties": {
    "status": "open",
    "priority": "high"
  },
  "version": 1,
  "lifecycle_status": "active",
  "created_at": "2026-01-01T00:00:00Z",
  "modified_at": "2026-01-01T00:00:00Z"
}
```

`properties` is flat, keyed by the schema's field names: read `jq '.properties.status'`
directly. Those same bare names are what `--property` and `--filters` take. Empty is `{}`.
