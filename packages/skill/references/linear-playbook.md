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

<!-- BEGIN GENERATED: linear-playbook (see packages/core/src/methodology/linear.rs, packages/cli/examples/gen_skill_md.rs) -->
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

### 2. Vocabulary extensions

These append values to fields `issue` **inherits** from `task`, so every new value carries `mapsTo` naming the base value it collapses to when something reading at `task` scope looks at it. Without that a base-scoped Play or query would meet a value it has never heard of.

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
  --properties '{"_seed":{"default_rules":[{"actions":[{"action_type":"create_node","params":{"content":"Next cycle","node_type":"cycle","properties":{"duration_days":"{trigger.node.duration_days}","end_date":"{add_days(trigger.node.end_date, trigger.node.duration_days)}","start_date":"{add_days(trigger.node.end_date, 1)}"}}},{"action_type":"add_relationship","for_each":"trigger.node.tasks.where(status != '\''done'\'' && status != '\''cancelled'\'')","params":{"relationship_type":"tasks","source_id":"{actions[0].result.id}","target_id":"{item.id}"}},{"action_type":"remove_relationship","for_each":"trigger.node.tasks.where(status != '\''done'\'' && status != '\''cancelled'\'')","params":{"relationship_type":"tasks","source_id":"{trigger.node.id}","target_id":"{item.id}"}}],"conditions":["node.end_date == today()"],"name":"create-successor-and-roll-over","trigger":{"cron":"0 5 0 * * * *","node_type":"cycle","type":"scheduled"}}]},"description":"On the day a cycle ends, create its successor — starting the next day and spanning that cycle'\''s own duration_days — then move the ending cycle'\''s unfinished tasks into it. Done and cancelled tasks stay with the ending cycle as its record.","rules":[{"actions":[{"action_type":"create_node","params":{"content":"Next cycle","node_type":"cycle","properties":{"duration_days":"{trigger.node.duration_days}","end_date":"{add_days(trigger.node.end_date, trigger.node.duration_days)}","start_date":"{add_days(trigger.node.end_date, 1)}"}}},{"action_type":"add_relationship","for_each":"trigger.node.tasks.where(status != '\''done'\'' && status != '\''cancelled'\'')","params":{"relationship_type":"tasks","source_id":"{actions[0].result.id}","target_id":"{item.id}"}},{"action_type":"remove_relationship","for_each":"trigger.node.tasks.where(status != '\''done'\'' && status != '\''cancelled'\'')","params":{"relationship_type":"tasks","source_id":"{trigger.node.id}","target_id":"{item.id}"}}],"conditions":["node.end_date == today()"],"name":"create-successor-and-roll-over","trigger":{"cron":"0 5 0 * * * *","node_type":"cycle","type":"scheduled"}}]}'
```

**Block closing an issue with open sub-issues** — Rejects a status change to done while any child issue is still open. Close the children first, or move them out from under this issue.

```bash
nodespace node create --type play --content 'Block closing an issue with open sub-issues' \
  --properties '{"_seed":{"default_rules":[{"actions":[{"action_type":"reject","params":{"message":"This issue still has open sub-issues. Close or cancel them first, or move them out from under this issue."}}],"class":"invariant","conditions":["node.status == '\''done'\''","node.has_child.exists(c, c.status != '\''done'\'' && c.status != '\''cancelled'\'')"],"name":"reject-done-with-open-children","trigger":{"node_type":"issue","on":"property_changed","property_key":"issue.status","type":"graph_event"}}]},"description":"Rejects a status change to done while any child issue is still open. Close the children first, or move them out from under this issue.","rules":[{"actions":[{"action_type":"reject","params":{"message":"This issue still has open sub-issues. Close or cancel them first, or move them out from under this issue."}}],"class":"invariant","conditions":["node.status == '\''done'\''","node.has_child.exists(c, c.status != '\''done'\'' && c.status != '\''cancelled'\'')"],"name":"reject-done-with-open-children","trigger":{"node_type":"issue","on":"property_changed","property_key":"issue.status","type":"graph_event"}}]}'
```

**Block starting an issue with an open blocker** — Rejects a status change to in_progress while anything blocking this issue is still open. Resolve the blocker, or drop the blocks edge if it no longer applies.

```bash
nodespace node create --type play --content 'Block starting an issue with an open blocker' \
  --properties '{"_seed":{"default_rules":[{"actions":[{"action_type":"reject","params":{"message":"This issue is blocked by work that is not finished yet. Resolve the blocker first, or remove the blocks relationship if it no longer applies."}}],"class":"invariant","conditions":["node.status == '\''in_progress'\''","node.blocked_by.exists(b, b.status != '\''done'\'' && b.status != '\''cancelled'\'')"],"name":"reject-start-with-open-blocker","trigger":{"node_type":"issue","on":"property_changed","property_key":"issue.status","type":"graph_event"}}]},"description":"Rejects a status change to in_progress while anything blocking this issue is still open. Resolve the blocker, or drop the blocks edge if it no longer applies.","rules":[{"actions":[{"action_type":"reject","params":{"message":"This issue is blocked by work that is not finished yet. Resolve the blocker first, or remove the blocks relationship if it no longer applies."}}],"class":"invariant","conditions":["node.status == '\''in_progress'\''","node.blocked_by.exists(b, b.status != '\''done'\'' && b.status != '\''cancelled'\'')"],"name":"reject-start-with-open-blocker","trigger":{"node_type":"issue","on":"property_changed","property_key":"issue.status","type":"graph_event"}}]}'
```

### 4. Guidance skills

Skill nodes carrying usage guidance, discovered through the ordinary skill-search mechanism. Each is a `skill` root node whose markdown body becomes ordinary child nodes. Deliberately several narrow skills rather than one broad one: retrieval scores a precise match far better than a skill diluted across every intent.

**Creating an Issue** — How to create an issue in a Linear-style workspace: when to use issue rather than task, the extended status and priority vocabularies, and point estimates.

**Working with Cycles** — How cycles work in a Linear-style workspace: assigning work via the tasks relationship, the derived active/past state, and what the automatic cycle-creation and rollover Plays do.

**Issue Validation Rules** — Why a status change on an issue was rejected: the sub-issue completion gate and the blocker gate, what each checks, and how to proceed when one fires.

Create each with `nodespace node create --type skill`, then add its guidance as markdown children. The bodies are long-form prose; read them from `packages/core/src/methodology/linear.rs` rather than reproducing them here.
<!-- END GENERATED: linear-playbook -->

## After installing

The scheduled Play runs daily, just after midnight, on whichever devices are
online.

**Create the first cycle yourself.** The Play triggers on an existing cycle
reaching its end date, so with no cycle in the graph nothing ever fires. Create
one with a `start_date` and `end_date`; the automation takes over from there.

Tell the user what landed: two new node types, two vocabulary extensions on the
inherited `task.status` and `task.priority` (stored on `issue`, leaving `task`
itself untouched), three Plays, and the guidance skills.

Mention two things they will otherwise meet as surprises: the validation gates
reject some status changes by design, so a later rejection is the system
working; and rollover moves only unfinished work — done and cancelled tasks stay
in the ending cycle as its record of what it accomplished.
