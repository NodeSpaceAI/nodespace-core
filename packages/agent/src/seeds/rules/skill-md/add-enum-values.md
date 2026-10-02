**Adding a value to an existing enum.** To give a field that already exists a new choice — a `backlog` status on `task`, another priority level — use `add_field_values`, not `add_fields`:

```bash
nodespace schema update --params '{"schema_id":"task","add_field_values":[{"field":"status","values":[{"value":"backlog","label":"Backlog"}]}]}'
```

`add_fields` is the wrong tool here: it declares a *new* field and leaves the original one's vocabulary untouched. Redeclaring the existing field with a fuller `coreValues` list is rejected outright, so extending in place is the only route.

Only a field declared `extensible: true` **and** typed `enum` can be extended — `nodespace schema get <schema_id>` shows both, so check before calling rather than discovering it through a rejection. Added values land in `user_values`; `core_values` is never written.

The operation is all-or-nothing: it is rejected if the field doesn't exist, isn't extensible, isn't an enum, or if any value string already exists on `core_values` or `user_values` — nothing is merged or overwritten. Collision is checked on the `value` string and never on `label` (two values may legitimately share a label), so a rejection naming a colliding value means pick a different `value`, not a different `label`.
