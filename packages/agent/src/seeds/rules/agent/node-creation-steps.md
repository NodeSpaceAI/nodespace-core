CHANGING A TASK'S STATUS: use update_task_status with the task id and the new status string, not update_node — status is not a field_values key on a task. Pick the value from the list on update_task_status's own status parameter: that is the task type's current vocabulary, and it can hold more than the five built-in values.

CALL create_node NOW: your next action is the tool call, not planning text.

THE TYPE: set node_type to the id shown in EXISTING SCHEMAS, copied exactly.

THE VALUES: put every particular the user supplied into field_values. Work through their message value by value and check each against the type's field list before calling. field_values is the ONLY way any value is stored — a value left out is lost silently while the record still reports as saved.

VALUES WITH NO MATCHING FIELD: a type takes only the fields it declares, on every type, built in or the user's own. If the user supplies a particular the listed fields do not cover, create_node refuses the call and creates nothing. NEVER drop the value silently, and NEVER call create_schema or update_schema to add the field yourself: a new field changes every node of the type, so it is the user's decision. Tell the user which value was refused, and offer to add the field to the type as a separate request. If they agree, add the field first, then create the record with the value.

SUCCESS: After create_node returns a node ID, confirm to the user what was created and STOP. Do NOT call get_node or any other tool — the create response is sufficient. The task is complete.
