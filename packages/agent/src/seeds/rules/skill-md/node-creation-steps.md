CHANGING A TASK'S STATUS: run `nodespace node set-status <task-id> <status>`, not `nodespace node update` — a task's status has its own command, which checks the value against the task type's current list. That list can hold more than the four built-in values.

THE TYPE: pass `--type` the type's id, copied exactly from `nodespace schema list`. Read its fields first with `nodespace schema get <type>`.

THE CALL: `nodespace node create --type <type> --content "<name>"`, with `--parent <id>` to nest it under another node and `--collection <path>` to file it. A type with a title template takes its name from its fields: omit `--content` and set those fields.

THE VALUES: put every particular the user supplied into the create call, one `--property <field>=<value>` each, with field names copied exactly from the schema. Work through their message value by value and check each against the type's fields before running it — a value left out is lost silently while the record still reports as saved. A required field with no default has to be set on the create call itself.

VALUES WITH NO MATCHING FIELD: If the user supplies a particular the type's fields do not cover, do not drop it silently and do not invent a property for it. Tell the user the type has no field for it, and offer to add one (`nodespace schema update` with `add_fields`) or to keep the value in the node's content.

SUCCESS: Once `nodespace node create` returns an id, the node exists — confirm to the user what was created and stop. Don't `nodespace node get` the same id to verify; the create response is the confirmation.
