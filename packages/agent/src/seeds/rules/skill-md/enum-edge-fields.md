**Edge fields.** A relationship can carry attributes on the edge itself via `edgeFields` — facts about the *connection*, not about either node (an access level on a membership, a billing date on an invoice link). Give an edge field a fixed vocabulary by declaring it as an enum with `coreValues`, the same shape a node field uses:

```json
{"name": "access", "type": "enum",
 "coreValues": [{"value": "owner", "label": "Owner"},
                {"value": "editor", "label": "Editor"},
                {"value": "viewer", "label": "Viewer"}]}
```

`coreValues` is required on an enum edge field and rejected on any other type; a `default` must be one of the declared values; values must be unique. Edge enums are closed — no `userValues`/`extensible` half. Creating or editing an edge validates the value against the declared set (including via `--edge-data`), and the relationships UI renders a picker instead of a free-text box.

Two limits worth knowing. Only relationships you declare can carry `edgeFields`: the built-in structural names (`member_of`, `has_child`, `mentions`, `has_role`) are reserved and rejected as declarations, so an edge field cannot be attached to them. And `required`/`default` on an edge field are recorded but not enforced at write time — an omitted enum key is stored absent rather than filled in from `default`, so don't rely on a default to supply a value.
