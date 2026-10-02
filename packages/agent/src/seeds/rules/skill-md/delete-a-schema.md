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
