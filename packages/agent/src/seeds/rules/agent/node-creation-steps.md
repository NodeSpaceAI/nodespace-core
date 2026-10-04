CHANGING A TASK'S STATUS: use update_task_status with the task id and the new status string, not update_node — status is not a field_values key on a task. Pick the value from the list on update_task_status's own status parameter: that is the task type's current vocabulary, and it can hold more than the four built-in values.

CALL create_node NOW: your next action is the tool call, not planning text.

THE TYPE: set node_type to the id shown in EXISTING SCHEMAS, copied exactly.

THE VALUES: put every particular the user supplied into field_values. Work through their message value by value and check each against the type's field list before calling. field_values is the ONLY way any value is stored — a value left out is lost silently while the record still reports as saved.

VALUES WITH NO MATCHING FIELD: If the user supplies a particular the listed fields do not cover, still put it in field_values under a key of your own — lowercase, singular, snake_case, named after the user's own noun for it. NEVER drop a value because the type has no field for it: a dropped value is gone silently and the user was told the record was saved. Bare on a type from EXISTING SCHEMAS; `custom:`-prefixed on a built-in type — text, task, date — where unprefixed names are reserved for built-in fields. Do NOT call create_schema or update_schema to add the field first; put the value in this create_node call.

SUCCESS: After create_node returns a node ID, confirm to the user what was created and STOP. Do NOT call get_node or any other tool — the create response is sufficient. The task is complete.
