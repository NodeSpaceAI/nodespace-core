# Jira-style methodology playbook

A canned setup for Jira-style work tracking: Epics, Stories and Bugs on top of
ordinary tasks, and Sprints with their own future → active → closed lifecycle.
Gates stop a sprint from starting without dates or being reopened, and lock a
closed sprint's dates and issues.

NodeSpace ships this as first-party content, installable from the desktop app's
onboarding or Settings. This document is for the other case: an agent setting up
a workspace over the CLI, with no desktop app in reach.

**Install it only when asked.** A methodology shapes how a whole workspace tracks
work — it is not a default to apply on a hunch. If a user describes a Jira-like
workflow, offer it; do not install it because a workspace happens to have tasks
in it.

**A workspace has at most one methodology.** If `story`, `bug`, `epic` or
`sprint` already exists, stop and ask rather than installing over the top.

Everything below is a composition of ordinary NodeSpace primitives. Nothing here
is special-cased in the engine, and a user can inspect, edit or delete any of it
afterwards exactly as if they had authored it by hand.

<!-- BEGIN GENERATED: jira-playbook (see packages/core/src/methodology/jira.rs, packages/core/src/methodology/skills/jira/, packages/cli/examples/gen_skill_md.rs) -->
## Jira-style

Epics, Stories and Bugs on top of tasks, with story points and bug severity, and Sprints that move future → active → closed. A sprint cannot start without dates or be reopened once closed, and a closed sprint's dates and issues are locked.

Run these in order. Each step depends on the ones before it: a Play whose trigger names a type is rejected until that type's schema exists, so a re-ordered sequence fails rather than half-installing.

### 1. Schemas

Create `story`:

```bash
nodespace schema create --params '{"description":"A requirement described from the user'\''s point of view, in a Jira-style workflow. Extends task with story points. Grouped into an epic via epic.issues and planned into sprints via sprint.issues.","extends":"task","fields":[{"description":"Relative size of the story. A number rather than a fixed scale: teams choose their own — Fibonacci, powers of two, or plain integers — and a closed list would fight whichever one they use.","friendlyName":"Story points","indexed":true,"name":"story_points","protection":"user","required":false,"type":"number"}],"name":"Story"}'
```

Create `bug`:

```bash
nodespace schema create --params '{"description":"A defect, in a Jira-style workflow. Extends task with severity and the environment it was seen in. Severity is how bad the defect is; the inherited priority is how urgently to fix it — separate axes, separate fields.","extends":"task","fields":[{"coreValues":[{"label":"Critical","value":"critical"},{"label":"Major","value":"major"},{"label":"Minor","value":"minor"},{"label":"Trivial","value":"trivial"}],"description":"How bad the defect is: critical (data loss, outage, no workaround), major (a feature broken), minor (broken with a workaround), trivial (cosmetic). Independent of priority — a trivial bug on the landing page can be urgent.","extensible":true,"friendlyName":"Severity","indexed":true,"name":"severity","protection":"user","required":false,"type":"enum","userValues":[]},{"description":"Where the defect was seen — deployment, browser and OS, version. Free text: environments are open-ended.","friendlyName":"Environment","indexed":false,"name":"environment","protection":"user","required":false,"type":"string"}],"name":"Bug"}'
```

Create `epic`:

```bash
nodespace schema create --params '{"description":"A large body of work, in a Jira-style workflow, grouping the tasks, stories and bugs that deliver it through epic.issues. Extends task, so it carries its own status, assignee and priority.","extends":"task","fields":[{"description":"When the epic is expected to be delivered. A target, not a deadline enforced by anything.","friendlyName":"Target date","indexed":true,"name":"target_date","protection":"user","required":false,"type":"date"}],"name":"Epic","relationships":[{"cardinality":"many","description":"The work in this epic — Jira'\''s Epic Link. Targets task, so tasks, stories and bugs can all belong. An issue is in at most one epic; linking it to another moves it. Flat membership, not nesting: sub-tasks use the outline.","direction":"out","name":"issues","reverseCardinality":"one","reverseName":"epic","targetType":"task"}]}'
```

Create `sprint`:

```bash
nodespace schema create --params '{"description":"A time-boxed iteration with its own lifecycle: future while being planned, active once started, closed once completed. A closed sprint is a record — only its name and goal can still change.","fields":[{"coreValues":[{"label":"Future","value":"future"},{"label":"Active","value":"active"},{"label":"Closed","value":"closed"}],"default":"future","description":"future while being planned; active once started, which needs both dates; closed once completed, which is final. Moves only future → active → closed.","extensible":false,"friendlyName":"Sprint status","indexed":true,"name":"sprint_status","protection":"user","required":false,"type":"enum","userValues":[]},{"description":"When the sprint is planned to start. May be set ahead of time and moved until the sprint closes.","friendlyName":"Start date","indexed":true,"name":"start_date","protection":"user","required":false,"type":"date"},{"description":"When the sprint is planned to end — the anticipated close, not the actual one, which is completed_date.","friendlyName":"End date","indexed":true,"name":"end_date","protection":"user","required":false,"type":"date"},{"description":"When the sprint was actually closed. Set automatically on close; never set it by hand.","friendlyName":"Completed","indexed":false,"name":"completed_date","protection":"user","required":false,"type":"date"},{"description":"What the sprint sets out to achieve, in a sentence. Stays editable after the sprint closes.","friendlyName":"Goal","indexed":false,"name":"goal","protection":"user","required":false,"type":"string"}],"name":"Sprint","relationships":[{"cardinality":"many","description":"The work planned into this sprint. Targets task, so tasks, stories and bugs can all be planned. Many on both ends: unfinished work is carried into later sprints while staying on the closed ones'\'' record.","direction":"out","name":"issues","reverseCardinality":"many","reverseName":"sprint","targetType":"task"}]}'
```

### 2. Schema extensions

None — this playbook adds no fields or values to an existing schema.

### 3. Plays

A Play is a node of type `play` carrying a `rules` property. Its rules are validated on write — conditions are CEL-compiled and every referenced type and path is checked — so a malformed Play is refused here, not at execution time.

**Keep sprints moving future → active → closed** — Rejects any sprint_status change other than future → active or active → closed, rejects starting a sprint without both dates, and rejects creating a sprint that is already started or closed.

```bash
nodespace node create --type play --content 'Keep sprints moving future → active → closed' \
  --properties '{"_seed":{"default_rules":[{"actions":[{"action_type":"reject","params":{"message":"A sprint moves only from future to active, and from active to closed. A closed sprint cannot be reopened — plan the remaining work into a new sprint instead."}}],"class":"invariant","conditions":["!(trigger.properties.exists(p, p.key == '\''sprint.sprint_status'\'' && (p.old_value == null) && p.new_value == '\''future'\'') || trigger.properties.exists(p, p.key == '\''sprint.sprint_status'\'' && (p.old_value == '\''future'\'' || p.old_value == null) && p.new_value == '\''active'\'') || trigger.properties.exists(p, p.key == '\''sprint.sprint_status'\'' && (p.old_value == '\''active'\'') && p.new_value == '\''closed'\''))"],"name":"reject-illegal-sprint-transition","trigger":{"node_type":"sprint","on":"property_changed","property_key":"sprint.sprint_status","type":"graph_event"}},{"actions":[{"action_type":"reject","params":{"message":"A sprint cannot start without a start_date and an end_date. Set both, then start it — they can be in the same update."}}],"class":"invariant","conditions":["node.sprint_status == '\''active'\''","!has(node.start_date) || node.start_date == null || !has(node.end_date) || node.end_date == null"],"name":"reject-starting-without-dates","trigger":{"node_type":"sprint","on":"property_changed","property_key":"sprint.sprint_status","type":"graph_event"}},{"actions":[{"action_type":"reject","params":{"message":"A sprint is created as future, with no completed_date. Create it, then start it once its dates are set."}}],"class":"invariant","conditions":["(has(node.sprint_status) && node.sprint_status != '\''future'\'') || (has(node.completed_date) && node.completed_date != null)"],"name":"reject-creating-a-started-sprint","trigger":{"node_type":"sprint","on":"node_created","type":"graph_event"}}]},"description":"Rejects any sprint_status change other than future → active or active → closed, rejects starting a sprint without both dates, and rejects creating a sprint that is already started or closed.","rules":[{"actions":[{"action_type":"reject","params":{"message":"A sprint moves only from future to active, and from active to closed. A closed sprint cannot be reopened — plan the remaining work into a new sprint instead."}}],"class":"invariant","conditions":["!(trigger.properties.exists(p, p.key == '\''sprint.sprint_status'\'' && (p.old_value == null) && p.new_value == '\''future'\'') || trigger.properties.exists(p, p.key == '\''sprint.sprint_status'\'' && (p.old_value == '\''future'\'' || p.old_value == null) && p.new_value == '\''active'\'') || trigger.properties.exists(p, p.key == '\''sprint.sprint_status'\'' && (p.old_value == '\''active'\'') && p.new_value == '\''closed'\''))"],"name":"reject-illegal-sprint-transition","trigger":{"node_type":"sprint","on":"property_changed","property_key":"sprint.sprint_status","type":"graph_event"}},{"actions":[{"action_type":"reject","params":{"message":"A sprint cannot start without a start_date and an end_date. Set both, then start it — they can be in the same update."}}],"class":"invariant","conditions":["node.sprint_status == '\''active'\''","!has(node.start_date) || node.start_date == null || !has(node.end_date) || node.end_date == null"],"name":"reject-starting-without-dates","trigger":{"node_type":"sprint","on":"property_changed","property_key":"sprint.sprint_status","type":"graph_event"}},{"actions":[{"action_type":"reject","params":{"message":"A sprint is created as future, with no completed_date. Create it, then start it once its dates are set."}}],"class":"invariant","conditions":["(has(node.sprint_status) && node.sprint_status != '\''future'\'') || (has(node.completed_date) && node.completed_date != null)"],"name":"reject-creating-a-started-sprint","trigger":{"node_type":"sprint","on":"node_created","type":"graph_event"}}]}'
```

**Record when a sprint closes** — When a sprint moves from active to closed, sets its completed_date to the time of the close.

```bash
nodespace node create --type play --content 'Record when a sprint closes' \
  --properties '{"_seed":{"default_rules":[{"actions":[{"action_type":"update_node","params":{"node_id":"{trigger.node.id}","properties":{"completed_date":"{trigger.node.modifiedAt}"}}}],"conditions":["trigger.properties.exists(p, p.key == '\''sprint.sprint_status'\'' && (p.old_value == '\''active'\'') && p.new_value == '\''closed'\'')"],"name":"stamp-completed-date","trigger":{"node_type":"sprint","on":"property_changed","property_key":"sprint.sprint_status","type":"graph_event"}}]},"description":"When a sprint moves from active to closed, sets its completed_date to the time of the close.","rules":[{"actions":[{"action_type":"update_node","params":{"node_id":"{trigger.node.id}","properties":{"completed_date":"{trigger.node.modifiedAt}"}}}],"conditions":["trigger.properties.exists(p, p.key == '\''sprint.sprint_status'\'' && (p.old_value == '\''active'\'') && p.new_value == '\''closed'\'')"],"name":"stamp-completed-date","trigger":{"node_type":"sprint","on":"property_changed","property_key":"sprint.sprint_status","type":"graph_event"}}]}'
```

**Lock closed sprints** — Rejects changing a closed sprint's dates or its issues, and rejects setting completed_date by hand. A closed sprint's name and goal stay editable.

```bash
nodespace node create --type play --content 'Lock closed sprints' \
  --properties '{"_seed":{"default_rules":[{"actions":[{"action_type":"reject","params":{"message":"This sprint is closed, so its start_date is locked as the record of what was planned. Only its name and goal can still change."}}],"class":"invariant","conditions":["node.sprint_status == '\''closed'\''"],"name":"lock-closed-sprint-start_date","trigger":{"node_type":"sprint","on":"property_changed","property_key":"sprint.start_date","type":"graph_event"}},{"actions":[{"action_type":"reject","params":{"message":"This sprint is closed, so its end_date is locked as the record of what was planned. Only its name and goal can still change."}}],"class":"invariant","conditions":["node.sprint_status == '\''closed'\''"],"name":"lock-closed-sprint-end_date","trigger":{"node_type":"sprint","on":"property_changed","property_key":"sprint.end_date","type":"graph_event"}},{"actions":[{"action_type":"reject","params":{"message":"completed_date records when the sprint was closed and is set automatically at that moment. It cannot be set by hand or changed afterwards."}}],"class":"invariant","conditions":["node.sprint_status != '\''closed'\'' || trigger.properties.exists(p, p.key == '\''sprint.sprint_status'\'') || trigger.properties.exists(p, p.key == '\''sprint.completed_date'\'' && p.old_value != null)"],"name":"lock-completed-date","trigger":{"node_type":"sprint","on":"property_changed","property_key":"sprint.completed_date","type":"graph_event"}},{"actions":[{"action_type":"reject","params":{"message":"This sprint is closed, so work cannot be added to it — its issues are the record of what the sprint held. Plan unfinished work into a future sprint instead."}}],"class":"invariant","conditions":["trigger.relationship.name == '\''issues'\''","node.sprint_status == '\''closed'\''"],"name":"lock-closed-sprint-membership-relationship_added","trigger":{"node_type":"sprint","on":"relationship_added","type":"graph_event"}},{"actions":[{"action_type":"reject","params":{"message":"This sprint is closed, so work cannot be removed from it — its issues are the record of what the sprint held. Plan unfinished work into a future sprint instead."}}],"class":"invariant","conditions":["trigger.relationship.name == '\''issues'\''","node.sprint_status == '\''closed'\''"],"name":"lock-closed-sprint-membership-relationship_removed","trigger":{"node_type":"sprint","on":"relationship_removed","type":"graph_event"}}]},"description":"Rejects changing a closed sprint'\''s dates or its issues, and rejects setting completed_date by hand. A closed sprint'\''s name and goal stay editable.","rules":[{"actions":[{"action_type":"reject","params":{"message":"This sprint is closed, so its start_date is locked as the record of what was planned. Only its name and goal can still change."}}],"class":"invariant","conditions":["node.sprint_status == '\''closed'\''"],"name":"lock-closed-sprint-start_date","trigger":{"node_type":"sprint","on":"property_changed","property_key":"sprint.start_date","type":"graph_event"}},{"actions":[{"action_type":"reject","params":{"message":"This sprint is closed, so its end_date is locked as the record of what was planned. Only its name and goal can still change."}}],"class":"invariant","conditions":["node.sprint_status == '\''closed'\''"],"name":"lock-closed-sprint-end_date","trigger":{"node_type":"sprint","on":"property_changed","property_key":"sprint.end_date","type":"graph_event"}},{"actions":[{"action_type":"reject","params":{"message":"completed_date records when the sprint was closed and is set automatically at that moment. It cannot be set by hand or changed afterwards."}}],"class":"invariant","conditions":["node.sprint_status != '\''closed'\'' || trigger.properties.exists(p, p.key == '\''sprint.sprint_status'\'') || trigger.properties.exists(p, p.key == '\''sprint.completed_date'\'' && p.old_value != null)"],"name":"lock-completed-date","trigger":{"node_type":"sprint","on":"property_changed","property_key":"sprint.completed_date","type":"graph_event"}},{"actions":[{"action_type":"reject","params":{"message":"This sprint is closed, so work cannot be added to it — its issues are the record of what the sprint held. Plan unfinished work into a future sprint instead."}}],"class":"invariant","conditions":["trigger.relationship.name == '\''issues'\''","node.sprint_status == '\''closed'\''"],"name":"lock-closed-sprint-membership-relationship_added","trigger":{"node_type":"sprint","on":"relationship_added","type":"graph_event"}},{"actions":[{"action_type":"reject","params":{"message":"This sprint is closed, so work cannot be removed from it — its issues are the record of what the sprint held. Plan unfinished work into a future sprint instead."}}],"class":"invariant","conditions":["trigger.relationship.name == '\''issues'\''","node.sprint_status == '\''closed'\''"],"name":"lock-closed-sprint-membership-relationship_removed","trigger":{"node_type":"sprint","on":"relationship_removed","type":"graph_event"}}]}'
```

### 4. Guidance skills

Skill nodes carrying usage guidance, discovered through the ordinary skill-search mechanism. Each is a `skill` root node whose markdown body becomes ordinary child nodes. Deliberately several narrow skills rather than one broad one: retrieval scores a precise match far better than a skill diluted across every intent.

Create each as below, then add its guidance as markdown children. The properties carry the whole retrieval surface — `description`, and an `exclusion` where the skill has one, which keeps general requests from ranking it above a built-in — so set them verbatim. The bodies are long-form prose; read them from `packages/core/src/methodology/skills/jira/` rather than reproducing them here.

**Creating Epics, Stories, and Bugs** — Create a story, file a bug, open an epic, or group work under an epic; set story points or bug severity. Use when the user says file a bug, write a user story, create an epic, add this to the epic, or how many points is this.

```bash
nodespace node create --type skill --content 'Creating Epics, Stories, and Bugs' \
  --properties '{"description":"Create a story, file a bug, open an epic, or group work under an epic; set story points or bug severity. Use when the user says file a bug, write a user story, create an epic, add this to the epic, or how many points is this.","exclusion":"Add a task or a reminder.","max_iterations":3,"tool_whitelist":["create_node","create_relationship","search_nodes","get_node"]}'
```

**Working with Sprints** — Start, plan or close a sprint, add work to the sprint, carry unfinished work into the next sprint, or set a sprint goal. Use when the user says start the sprint, close the sprint, what's in this sprint, or move this to the next sprint.

```bash
nodespace node create --type skill --content 'Working with Sprints' \
  --properties '{"description":"Start, plan or close a sprint, add work to the sprint, carry unfinished work into the next sprint, or set a sprint goal. Use when the user says start the sprint, close the sprint, what'\''s in this sprint, or move this to the next sprint.","exclusion":"Add a task or a reminder.","max_iterations":3,"tool_whitelist":["create_node","update_node","create_relationship","search_nodes","get_node"]}'
```

**Sprint Validation Rules** — Why a sprint change was rejected: a sprint won't start, won't reopen, or a closed sprint won't take edits or new work. Use when the user says it won't let me start the sprint, why can't I reopen this sprint, or why can't I add this to the sprint.

```bash
nodespace node create --type skill --content 'Sprint Validation Rules' \
  --properties '{"description":"Why a sprint change was rejected: a sprint won'\''t start, won'\''t reopen, or a closed sprint won'\''t take edits or new work. Use when the user says it won'\''t let me start the sprint, why can'\''t I reopen this sprint, or why can'\''t I add this to the sprint.","exclusion":"Link a task to a decision.","max_iterations":3,"tool_whitelist":["create_node","update_node","search_nodes","get_node"]}'
```

### 5. Saved views

A saved view is a node of type `query`. Its properties carry both the query (`target_type`, `filters`, `sorting`) and how it renders (`view_config`), so the board opens as authored with no per-user setup.

**Epics by Status**

```bash
nodespace node create --type query --content 'Epics by Status' \
  --properties '{"filters":[],"generated_by":"user","target_type":"epic","view_config":{"kanban":{"groupBy":"status"},"lastView":"kanban"}}'
```

**Sprints**

```bash
nodespace node create --type query --content 'Sprints' \
  --properties '{"filters":[],"generated_by":"user","target_type":"sprint","view_config":{"kanban":{"groupBy":"sprint_status"},"lastView":"kanban"}}'
```

### 6. Workspace skill

One more skill, created after everything else because it names what you created. Title it exactly as shown: it is how an agent in this workspace later recognizes the Playbook is installed.

**Jira-style Workspace** — What workflow this workspace uses: the Jira-style Playbook installed here — its epic, story, bug and sprint types, the Plays that gate sprints, its saved views, and the schema ids they were actually created under.

Create it the same way as the guidance skills, and end its body with an "Installed in this workspace" section listing the types, Plays, guidance skills and views above, by id.
<!-- END GENERATED: jira-playbook -->

## After installing

Tell the user what landed: four new node types, three Plays, the guidance
skills, and two saved views. Mention that a sprint is created as future, needs
both dates to start, and cannot be reopened once closed — and that a closed
sprint keeps its dates and issues as a record, so unfinished work is carried
forward by linking it to the next sprint.

This document is done at that point. Working in the installed workspace —
creating stories and bugs, running sprints, what the gates reject — is guidance
the install seeded into the graph: `nodespace skill guidance "Jira-style Workspace"`.
