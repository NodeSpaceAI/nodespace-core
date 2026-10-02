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
