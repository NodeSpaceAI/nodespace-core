# Linear-style methodology playbook

A canned setup for Linear-style work tracking: Issues with point estimates and a
richer status vocabulary, time-boxed Cycles that roll their work into a
successor when they end, and two validation gates that refuse a status change
the workflow does not allow.

NodeSpace ships this as first-party content, installable from the desktop app's
onboarding or Settings. This document is for the other case: an agent setting up
a workspace over the CLI, with no desktop app in reach.

**Install it only when asked.** A methodology shapes how a whole workspace tracks
work — it is not a default to apply on a hunch. If a user describes a Linear-like
workflow, offer it; do not install it because a workspace happens to have tasks
in it.

**A workspace has at most one methodology.** If `issue` or `cycle` already
exists, stop and ask rather than installing over the top. A second install
colliding with the first is not a merge — it is two half-configured
vocabularies in one graph.

Everything below is a composition of ordinary NodeSpace primitives. Nothing here
is special-cased in the engine, and a user can inspect, edit or delete any of it
afterwards exactly as if they had authored it by hand.

<!-- BEGIN GENERATED: linear-playbook (see packages/core/src/methodology/linear.rs, packages/core/src/methodology/skills/linear/, packages/cli/examples/gen_skill_md.rs) -->
## Linear-style

Issues with point estimates and a richer status vocabulary, time-boxed Cycles with automatic creation and rollover, and validation gates that stop an issue closing with open sub-issues or starting with an unresolved blocker.

Run these in order. Each step depends on the ones before it: a Play whose trigger names a type is rejected until that type's schema exists, so a re-ordered sequence fails rather than half-installing.

### 1. Schemas

Create `issue`:

```bash
nodespace schema create --params '{"description":"A unit of work in a Linear-style workflow. Extends task with a point estimate and a richer status vocabulary. Sub-issues are ordinary child nodes; labels and teams are Collections; blocking and relation edges are inherited from task.","extends":"task","fields":[{"coreValues":[{"label":"1 point","value":"1"},{"label":"2 points","value":"2"},{"label":"3 points","value":"3"},{"label":"5 points","value":"5"},{"label":"8 points","value":"8"}],"description":"Relative size in points, on the modified-Fibonacci scale Linear uses. Points rather than hours: the scale is deliberately coarse and gappy at the top so large items are estimated as clearly large rather than precisely wrong. Extensible, so a team wanting 13 or 21 can add them.","extensible":true,"friendlyName":"Estimate","indexed":true,"name":"estimate","protection":"user","required":false,"type":"enum","userValues":[]}],"name":"Issue"}'
```

Create `cycle`:

```bash
nodespace schema create --params '{"description":"A time-boxed iteration. Its upcoming/active/past state is derived by comparing start_date and end_date to today — deliberately not stored, so there is no second copy of the truth to keep in sync.","fields":[{"description":"First day of the cycle.","friendlyName":"Start date","indexed":true,"name":"start_date","protection":"user","required":true,"type":"date"},{"description":"Last day of the cycle. A cycle whose end_date has passed is over; on that day the rollover Play moves its tasks into the successor.","friendlyName":"End date","indexed":true,"name":"end_date","protection":"user","required":true,"type":"date"},{"default":14,"description":"How many days the NEXT cycle should span. Read by the cycle-creation Play when it computes the successor'\''s end_date, so changing cadence is a field edit rather than a Play rewrite.","friendlyName":"Duration (days)","indexed":false,"name":"duration_days","protection":"user","required":false,"type":"number"}],"name":"Cycle","relationships":[{"cardinality":"many","description":"Work assigned to this cycle. Targets `task`, not `issue`: subtype-aware querying already reaches issues through it, and targeting the base type keeps a plain task assignable to a cycle.","direction":"out","name":"tasks","reverseCardinality":"one","reverseName":"cycle","targetType":"task"}]}'
```

### 2. Schema extensions

A vocabulary extension appends values to a field. When the field is **inherited** from a base type, every new value carries `mapsTo` naming the base value it collapses to when something reading at the base scope looks at it. Without that a base-scoped Play or query would meet a value it has never heard of.

Extend `issue.status`:

```bash
nodespace schema update --params '{"add_field_values":[{"field":"status","values":[{"label":"Triage","mapsTo":"open","value":"triage"},{"label":"Backlog","mapsTo":"open","value":"backlog"},{"label":"In Review","mapsTo":"in_progress","value":"in_review"}]}],"schema_id":"issue"}'
```

Extend `issue.priority`:

```bash
nodespace schema update --params '{"add_field_values":[{"field":"priority","values":[{"label":"Urgent","mapsTo":"highest","value":"urgent"},{"label":"No priority","mapsTo":"lowest","value":"none"}]}],"schema_id":"issue"}'
```

### 3. Plays

A Play is a node of type `play` carrying a `rules` property. Its rules are validated on write — conditions are CEL-compiled and every referenced type and path is checked — so a malformed Play is refused here, not at execution time.

**Close out the ending cycle** — On the day a cycle ends, create its successor — starting the next day and spanning that cycle's own duration_days — then move the ending cycle's unfinished tasks into it. Done and cancelled tasks stay with the ending cycle as its record.

```bash
nodespace node create --type play --content 'Close out the ending cycle' \
  --properties '{"_seed":{"default_rules":[{"actions":[{"action_type":"create_node","description":"Create the next cycle, starting the day after this one ends and lasting as many days","params":{"content":"Next cycle","node_type":"cycle","properties":{"duration_days":"{trigger.node.duration_days}","end_date":"{add_days(trigger.node.end_date, trigger.node.duration_days)}","start_date":"{add_days(trigger.node.end_date, 1)}"}}},{"action_type":"add_relationship","description":"Add each unfinished task to the next cycle","for_each":"trigger.node.tasks.where(status != '\''done'\'' && status != '\''cancelled'\'')","params":{"relationship_type":"tasks","source_id":"{actions[0].result.id}","target_id":"{item.id}"}},{"action_type":"remove_relationship","description":"Take each unfinished task out of the ending cycle","for_each":"trigger.node.tasks.where(status != '\''done'\'' && status != '\''cancelled'\'')","params":{"relationship_type":"tasks","source_id":"{trigger.node.id}","target_id":"{item.id}"}}],"conditions":[{"description":"The cycle ends today","expr":"node.end_date == today()"}],"description":"On the day a cycle ends, create the next cycle and move the unfinished tasks into it","name":"create-successor-and-roll-over","trigger":{"cron":"0 5 0 * * * *","select":{"target_type":"cycle"},"type":"scheduled"}}]},"description":"On the day a cycle ends, create its successor — starting the next day and spanning that cycle'\''s own duration_days — then move the ending cycle'\''s unfinished tasks into it. Done and cancelled tasks stay with the ending cycle as its record.","rules":[{"actions":[{"action_type":"create_node","description":"Create the next cycle, starting the day after this one ends and lasting as many days","params":{"content":"Next cycle","node_type":"cycle","properties":{"duration_days":"{trigger.node.duration_days}","end_date":"{add_days(trigger.node.end_date, trigger.node.duration_days)}","start_date":"{add_days(trigger.node.end_date, 1)}"}}},{"action_type":"add_relationship","description":"Add each unfinished task to the next cycle","for_each":"trigger.node.tasks.where(status != '\''done'\'' && status != '\''cancelled'\'')","params":{"relationship_type":"tasks","source_id":"{actions[0].result.id}","target_id":"{item.id}"}},{"action_type":"remove_relationship","description":"Take each unfinished task out of the ending cycle","for_each":"trigger.node.tasks.where(status != '\''done'\'' && status != '\''cancelled'\'')","params":{"relationship_type":"tasks","source_id":"{trigger.node.id}","target_id":"{item.id}"}}],"conditions":[{"description":"The cycle ends today","expr":"node.end_date == today()"}],"description":"On the day a cycle ends, create the next cycle and move the unfinished tasks into it","name":"create-successor-and-roll-over","trigger":{"cron":"0 5 0 * * * *","select":{"target_type":"cycle"},"type":"scheduled"}}]}'
```

**Block closing an issue with open sub-issues** — Rejects a status change to done while any child issue is still open. Close the children first, or move them out from under this issue.

```bash
nodespace node create --type play --content 'Block closing an issue with open sub-issues' \
  --properties '{"_seed":{"default_rules":[{"actions":[{"action_type":"reject","description":"Refuse the change and say the sub-issues must be closed first","params":{"message":"This issue still has open sub-issues. Close or cancel them first, or move them out from under this issue."}}],"class":"invariant","conditions":[{"description":"The issue is being marked done","expr":"node.status == '\''done'\''"},{"description":"At least one sub-issue is neither done nor cancelled","expr":"node.has_child.exists(c, c.status != '\''done'\'' && c.status != '\''cancelled'\'')"}],"description":"Refuse to mark an issue done while it has open sub-issues","name":"reject-done-with-open-children","trigger":{"on":"property_changed","property_key":"issue.status","select":{"target_type":"issue"},"type":"graph_event"}}]},"description":"Rejects a status change to done while any child issue is still open. Close the children first, or move them out from under this issue.","rules":[{"actions":[{"action_type":"reject","description":"Refuse the change and say the sub-issues must be closed first","params":{"message":"This issue still has open sub-issues. Close or cancel them first, or move them out from under this issue."}}],"class":"invariant","conditions":[{"description":"The issue is being marked done","expr":"node.status == '\''done'\''"},{"description":"At least one sub-issue is neither done nor cancelled","expr":"node.has_child.exists(c, c.status != '\''done'\'' && c.status != '\''cancelled'\'')"}],"description":"Refuse to mark an issue done while it has open sub-issues","name":"reject-done-with-open-children","trigger":{"on":"property_changed","property_key":"issue.status","select":{"target_type":"issue"},"type":"graph_event"}}]}'
```

**Block starting an issue with an open blocker** — Rejects a status change to in_progress while anything blocking this issue is still open. Resolve the blocker, or drop the blocks edge if it no longer applies.

```bash
nodespace node create --type play --content 'Block starting an issue with an open blocker' \
  --properties '{"_seed":{"default_rules":[{"actions":[{"action_type":"reject","description":"Refuse the change and say the blocker must be resolved first","params":{"message":"This issue is blocked by work that is not finished yet. Resolve the blocker first, or remove the blocks relationship if it no longer applies."}}],"class":"invariant","conditions":[{"description":"The issue is being started","expr":"node.status == '\''in_progress'\''"},{"description":"At least one blocker is neither done nor cancelled","expr":"node.blocked_by.exists(b, b.status != '\''done'\'' && b.status != '\''cancelled'\'')"}],"description":"Refuse to start an issue while something blocking it is unfinished","name":"reject-start-with-open-blocker","trigger":{"on":"property_changed","property_key":"issue.status","select":{"target_type":"issue"},"type":"graph_event"}}]},"description":"Rejects a status change to in_progress while anything blocking this issue is still open. Resolve the blocker, or drop the blocks edge if it no longer applies.","rules":[{"actions":[{"action_type":"reject","description":"Refuse the change and say the blocker must be resolved first","params":{"message":"This issue is blocked by work that is not finished yet. Resolve the blocker first, or remove the blocks relationship if it no longer applies."}}],"class":"invariant","conditions":[{"description":"The issue is being started","expr":"node.status == '\''in_progress'\''"},{"description":"At least one blocker is neither done nor cancelled","expr":"node.blocked_by.exists(b, b.status != '\''done'\'' && b.status != '\''cancelled'\'')"}],"description":"Refuse to start an issue while something blocking it is unfinished","name":"reject-start-with-open-blocker","trigger":{"on":"property_changed","property_key":"issue.status","select":{"target_type":"issue"},"type":"graph_event"}}]}'
```

### 4. Guidance skills

Skill nodes carrying usage guidance, discovered through the ordinary skill-search mechanism. Each is a `skill` root node whose markdown body becomes ordinary child nodes. Deliberately several narrow skills rather than one broad one: retrieval scores a precise match far better than a skill diluted across every intent.

Create each as below, then add its guidance as markdown children. The properties carry the whole retrieval surface — `description`, and an `exclusion` where the skill has one, which keeps general requests from ranking it above a built-in — so set them verbatim. The bodies are long-form prose; read them from `packages/core/src/methodology/skills/linear/` rather than reproducing them here.

After creating a skill, link it to the types it is about with an `applies_to` edge to each type's schema, as shown under it. Skill search then hands an agent the skill together with exactly those types' definitions. If a type landed under another id (step 1 reported a re-key), link to that id.

**Creating an Issue** — Report a bug, defect, crash or something broken, open a ticket, or raise an issue. Use when the user wants to file or log a bug, open a ticket, report a problem, or request a feature.

```bash
nodespace node create --type skill --content 'Creating an Issue' \
  --properties '{"description":"Report a bug, defect, crash or something broken, open a ticket, or raise an issue. Use when the user wants to file or log a bug, open a ticket, report a problem, or request a feature.","exclusion":"Add a task or a reminder.","max_iterations":3,"tool_whitelist":["create_node","update_node","search_nodes","get_node"]}'
nodespace relationship create --from <skill-id> --type applies_to --to issue
```

**Sprints and Cycles** — Start, plan or close out a sprint or cycle, put issues in the current sprint, roll unfinished issues into the next sprint, and total a sprint's points. Use when the user says start the sprint, what's in this cycle, or how many points are in the sprint.

```bash
nodespace node create --type skill --content 'Sprints and Cycles' \
  --properties '{"description":"Start, plan or close out a sprint or cycle, put issues in the current sprint, roll unfinished issues into the next sprint, and total a sprint'\''s points. Use when the user says start the sprint, what'\''s in this cycle, or how many points are in the sprint.","exclusion":"Add a task or a reminder.","max_iterations":3,"tool_whitelist":["create_node","update_node","search_nodes","get_node"]}'
nodespace relationship create --from <skill-id> --type applies_to --to cycle
nodespace relationship create --from <skill-id> --type applies_to --to issue
```

**Issue Validation Rules** — Why an issue won't close or won't start: its status change was rejected because sub-issues are still open or a blocker isn't done. Use when the user says it won't let me mark this done, it won't let me move this to in progress, why can't I close this, or why is this blocked.

```bash
nodespace node create --type skill --content 'Issue Validation Rules' \
  --properties '{"description":"Why an issue won'\''t close or won'\''t start: its status change was rejected because sub-issues are still open or a blocker isn'\''t done. Use when the user says it won'\''t let me mark this done, it won'\''t let me move this to in progress, why can'\''t I close this, or why is this blocked.","exclusion":"Link a task to a decision.","max_iterations":3,"tool_whitelist":["create_node","update_node","search_nodes","get_node"]}'
nodespace relationship create --from <skill-id> --type applies_to --to issue
```

### 5. Saved views

A saved view is a node of type `query`. Its properties carry both the query (`target_type`, `filters`, `sorting`) and how it renders (`view_config`), so the board opens as authored with no per-user setup.

**Issues by Status**

```bash
nodespace node create --type query --content 'Issues by Status' \
  --properties '{"filters":[],"generated_by":"user","target_type":"issue","view_config":{"kanban":{"columnOrder":{"status":["triage","backlog","open","in_progress","in_review","done","cancelled"]},"groupBy":"status"},"lastView":"kanban"}}'
```

**Current Cycle Issues**

```bash
nodespace node create --type query --content 'Current Cycle Issues' \
  --properties '{"filters":[{"case_sensitive":null,"filter":{"case_sensitive":null,"node_id":null,"operator":"lte","property":"start_date","relative_date":{"anchor":"today"},"type":"property","value":null},"node_id":null,"operator":"exists","path":["cycle"],"property":null,"type":"related","value":null},{"case_sensitive":null,"filter":{"case_sensitive":null,"node_id":null,"operator":"gte","property":"end_date","relative_date":{"anchor":"today"},"type":"property","value":null},"node_id":null,"operator":"exists","path":["cycle"],"property":null,"type":"related","value":null}],"generated_by":"user","target_type":"issue","view_config":{"kanban":{"columnOrder":{"status":["triage","backlog","open","in_progress","in_review","done","cancelled"]},"groupBy":"status"},"lastView":"kanban"}}'
```

### 6. Workspace skill

One more skill, created after everything else because it names what you created. Title it exactly as shown: it is how an agent in this workspace later recognizes the Playbook is installed.

**Linear-style Workspace** — What workflow this workspace uses: the Linear-style Playbook installed here — its issue and cycle types, the Plays that automate and gate them, its saved views, and the schema ids they were actually created under.

Create it the same way as the guidance skills, and end its body with an "Installed in this workspace" section listing the types, Plays, guidance skills and views above, by id. Link it to the Playbook's types:

```bash
nodespace relationship create --from <skill-id> --type applies_to --to issue
nodespace relationship create --from <skill-id> --type applies_to --to cycle
```
<!-- END GENERATED: linear-playbook -->

## After installing

Tell the user what landed: two new node types, two vocabulary extensions on the
inherited `task.status` and `task.priority` (stored on `issue`, leaving `task`
itself untouched), three Plays, the guidance skills, and two saved views.

This document is done at that point. Working in the installed workspace —
creating issues, the first cycle, what the gates reject — is guidance the
install seeded into the graph: `nodespace skill guidance "Linear-style Workspace"`.
