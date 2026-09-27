# Spec-driven methodology playbook

A canned setup for spec-driven development: a `spec` captures what is being
built and why, a `plan` captures how, and ordinary tasks trace back to both.
Approval gates stop work from advancing past a stage that has not been agreed.

NodeSpace ships this as first-party content, installable from the desktop app's
onboarding or Settings. This document is for the other case: an agent setting up
a workspace over the CLI, with no desktop app in reach.

**Install it only when asked.** A methodology shapes how a whole workspace tracks
work — it is not a default to apply on a hunch. If a user describes working
spec-first, offer it; do not install it because a workspace happens to have
tasks in it.

**A workspace has at most one methodology.** If `spec` or `plan` already exists,
stop and ask rather than installing over the top.

Everything below is a composition of ordinary NodeSpace primitives. Nothing here
is special-cased in the engine, and a user can inspect, edit or delete any of it
afterwards exactly as if they had authored it by hand.

<!-- BEGIN GENERATED: spec-driven-playbook (see packages/core/src/methodology/spec_driven.rs, packages/core/src/methodology/skills/spec_driven/, packages/cli/examples/gen_skill_md.rs) -->
## Spec-driven

Specs capture what and why, plans capture how, and tasks trace back to both. A plan cannot be approved against an unapproved spec, a planned task cannot start or finish until its plan is approved and it names its spec, a task cannot close without a recorded verification method, and a superseded spec or plan is locked.

Run these in order. Each step depends on the ones before it: a Play whose trigger names a type is rejected until that type's schema exists, so a re-ordered sequence fails rather than half-installing.

### 1. Schemas

Create `spec`:

```bash
nodespace schema create --params '{"description":"What is being built and why: the objective, the testable criteria that decide whether it is done, and the boundaries on how it may be done. The source of truth every plan and task under it is judged against.","fields":[{"description":"What is being built, why, and for whom — in the requester'\''s own terms, not a restatement of the title.","friendlyName":"Objective","indexed":false,"name":"objective","protection":"user","required":false,"type":"string"},{"description":"Testable conditions that decide whether the work is done. Each should name a check someone could actually run or observe.","friendlyName":"Success criteria","indexed":false,"name":"success_criteria","protection":"user","required":false,"type":"string"},{"description":"What may always be done without asking, what needs sign-off first, and what must never be done.","friendlyName":"Boundaries","indexed":false,"name":"boundaries","protection":"user","required":false,"type":"string"},{"coreValues":[{"label":"Draft","value":"draft"},{"label":"Approved","value":"approved"},{"label":"Superseded","value":"superseded"}],"default":"draft","description":"draft while being written; approved once the requester has confirmed it, which is what lets a plan against it be approved; superseded once a newer spec replaces it, which locks its content.","extensible":false,"friendlyName":"Spec status","indexed":true,"name":"spec_status","protection":"user","required":false,"type":"enum","userValues":[]}],"name":"Spec","relationships":[{"cardinality":"many","description":"Tasks this spec governs, linked directly rather than only through a plan so a task'\''s spec is one hop away. A task may serve more than one spec.","direction":"out","name":"tasks","reverseCardinality":"many","reverseName":"spec","targetType":"task"}]}'
```

Create `plan`:

```bash
nodespace schema create --params '{"description":"The technical approach for one spec: components, sequencing, and risks. Revisions are new plans; the old one is marked superseded rather than rewritten, so what a task was built against stays readable.","fields":[{"description":"Major components, dependencies and sequencing, tied to the spec'\''s success criteria by name.","friendlyName":"Approach","indexed":false,"name":"approach","protection":"user","required":false,"type":"string"},{"description":"What could go wrong with this particular approach.","friendlyName":"Risks","indexed":false,"name":"risks","protection":"user","required":false,"type":"string"},{"coreValues":[{"label":"Draft","value":"draft"},{"label":"Approved","value":"approved"},{"label":"Superseded","value":"superseded"}],"default":"draft","description":"draft while being written; approved once the requester has confirmed it, which requires its spec to be approved and is what lets planned tasks start; superseded once a newer plan replaces it, which locks its content.","extensible":false,"friendlyName":"Plan status","indexed":true,"name":"plan_status","protection":"user","required":false,"type":"enum","userValues":[]}],"name":"Plan","relationships":[{"cardinality":"one","description":"The spec this plan implements. One per plan; a spec accumulates plans as they are revised and superseded.","direction":"out","name":"spec","reverseCardinality":"many","reverseName":"plans","targetType":"spec"},{"cardinality":"many","description":"Tasks that carry out this plan. A task belongs to one plan.","direction":"out","name":"tasks","reverseCardinality":"one","reverseName":"plan","targetType":"task"}]}'
```

### 2. Schema extensions

Add `custom:verification_method` to `task`:

```bash
nodespace schema update --params '{"add_fields":[{"description":"How completion was verified — the command run or the steps checked, and what they showed. Required before a spec-driven task can be marked done.","friendlyName":"Verification method","indexed":false,"name":"custom:verification_method","protection":"user","required":false,"type":"string"}],"schema_id":"task"}'
```

### 3. Plays

A Play is a node of type `play` carrying a `rules` property. Its rules are validated on write — conditions are CEL-compiled and every referenced type and path is checked — so a malformed Play is refused here, not at execution time.

**Require an approved spec before approving a plan** — Rejects approving a plan unless it is linked to a spec whose spec_status is approved. Approve the spec first, or link the plan to the spec it implements.

```bash
nodespace node create --type play --content 'Require an approved spec before approving a plan' \
  --properties '{"_seed":{"default_rules":[{"actions":[{"action_type":"reject","params":{"message":"This plan cannot be approved until it is linked to an approved spec. Link it to the spec it implements, and approve that spec first."}}],"class":"invariant","conditions":["node.plan_status == '\''approved'\''","!has(node.spec) || !has(node.spec.spec_status) || node.spec.spec_status != '\''approved'\''"],"name":"reject-approval-without-approved-spec","trigger":{"node_type":"plan","on":"property_changed","property_key":"plan.plan_status","type":"graph_event"}},{"actions":[{"action_type":"reject","params":{"message":"A plan cannot be created already approved: approval requires a link to an approved spec, and a new plan has none yet. Create it as draft, link it to its spec, then approve it."}}],"class":"invariant","conditions":["node.plan_status == '\''approved'\''"],"name":"reject-creating-an-approved-plan","trigger":{"node_type":"plan","on":"node_created","type":"graph_event"}}]},"description":"Rejects approving a plan unless it is linked to a spec whose spec_status is approved. Approve the spec first, or link the plan to the spec it implements.","rules":[{"actions":[{"action_type":"reject","params":{"message":"This plan cannot be approved until it is linked to an approved spec. Link it to the spec it implements, and approve that spec first."}}],"class":"invariant","conditions":["node.plan_status == '\''approved'\''","!has(node.spec) || !has(node.spec.spec_status) || node.spec.spec_status != '\''approved'\''"],"name":"reject-approval-without-approved-spec","trigger":{"node_type":"plan","on":"property_changed","property_key":"plan.plan_status","type":"graph_event"}},{"actions":[{"action_type":"reject","params":{"message":"A plan cannot be created already approved: approval requires a link to an approved spec, and a new plan has none yet. Create it as draft, link it to its spec, then approve it."}}],"class":"invariant","conditions":["node.plan_status == '\''approved'\''"],"name":"reject-creating-an-approved-plan","trigger":{"node_type":"plan","on":"node_created","type":"graph_event"}}]}'
```

**Require an approved plan and a spec before starting planned work** — Rejects moving a task linked to a plan into in_progress or done unless that plan is approved and the task is also linked to the spec that plan implements.

```bash
nodespace node create --type play --content 'Require an approved plan and a spec before starting planned work' \
  --properties '{"_seed":{"default_rules":[{"actions":[{"action_type":"reject","params":{"message":"This task implements a plan, so it cannot start or finish until that plan is approved and the task is linked to the plan'\''s spec as well. Approve the plan, and link the task to the spec the plan implements."}}],"class":"invariant","conditions":["node.status == '\''in_progress'\'' || node.status == '\''done'\''","has(node.plan)","!has(node.plan.plan_status) || node.plan.plan_status != '\''approved'\'' || !has(node.plan.spec) || !has(node.spec) || !node.spec.exists(s, s == node.plan.spec)"],"name":"reject-advancing-without-lineage","trigger":{"node_type":"task","on":"property_changed","property_key":"task.status","type":"graph_event"}}]},"description":"Rejects moving a task linked to a plan into in_progress or done unless that plan is approved and the task is also linked to the spec that plan implements.","rules":[{"actions":[{"action_type":"reject","params":{"message":"This task implements a plan, so it cannot start or finish until that plan is approved and the task is linked to the plan'\''s spec as well. Approve the plan, and link the task to the spec the plan implements."}}],"class":"invariant","conditions":["node.status == '\''in_progress'\'' || node.status == '\''done'\''","has(node.plan)","!has(node.plan.plan_status) || node.plan.plan_status != '\''approved'\'' || !has(node.plan.spec) || !has(node.spec) || !node.spec.exists(s, s == node.plan.spec)"],"name":"reject-advancing-without-lineage","trigger":{"node_type":"task","on":"property_changed","property_key":"task.status","type":"graph_event"}}]}'
```

**Require a verification method before closing spec-driven work** — Rejects marking a task linked to a plan or spec done until its verification method records how the work was checked.

```bash
nodespace node create --type play --content 'Require a verification method before closing spec-driven work' \
  --properties '{"_seed":{"default_rules":[{"actions":[{"action_type":"reject","params":{"message":"This task cannot be marked done until its verification method records how the work was checked. Set custom:verification_method first, then change the status."}}],"class":"invariant","conditions":["node.status == '\''done'\''","has(node.plan) || (has(node.spec) && size(node.spec) > 0)","!has(node.verification_method) || node.verification_method == '\'''\''"],"name":"reject-done-without-verification","trigger":{"node_type":"task","on":"property_changed","property_key":"task.status","type":"graph_event"}}]},"description":"Rejects marking a task linked to a plan or spec done until its verification method records how the work was checked.","rules":[{"actions":[{"action_type":"reject","params":{"message":"This task cannot be marked done until its verification method records how the work was checked. Set custom:verification_method first, then change the status."}}],"class":"invariant","conditions":["node.status == '\''done'\''","has(node.plan) || (has(node.spec) && size(node.spec) > 0)","!has(node.verification_method) || node.verification_method == '\'''\''"],"name":"reject-done-without-verification","trigger":{"node_type":"task","on":"property_changed","property_key":"task.status","type":"graph_event"}}]}'
```

**Lock superseded specs and plans** — Rejects edits to a superseded spec's or plan's content, and rejects moving it back out of superseded. Revisions are new nodes, so anything that referenced the old one still reads what it was built against.

```bash
nodespace node create --type play --content 'Lock superseded specs and plans' \
  --properties '{"_seed":{"default_rules":[{"actions":[{"action_type":"reject","params":{"message":"This spec is superseded, so its objective is locked as the record of what was agreed. Create a new spec for the revised version instead of editing this one."}}],"class":"invariant","conditions":["node.spec_status == '\''superseded'\''"],"name":"lock-superseded-spec-objective","trigger":{"node_type":"spec","on":"property_changed","property_key":"spec.objective","type":"graph_event"}},{"actions":[{"action_type":"reject","params":{"message":"This spec is superseded, so its success_criteria is locked as the record of what was agreed. Create a new spec for the revised version instead of editing this one."}}],"class":"invariant","conditions":["node.spec_status == '\''superseded'\''"],"name":"lock-superseded-spec-success_criteria","trigger":{"node_type":"spec","on":"property_changed","property_key":"spec.success_criteria","type":"graph_event"}},{"actions":[{"action_type":"reject","params":{"message":"This spec is superseded, so its boundaries is locked as the record of what was agreed. Create a new spec for the revised version instead of editing this one."}}],"class":"invariant","conditions":["node.spec_status == '\''superseded'\''"],"name":"lock-superseded-spec-boundaries","trigger":{"node_type":"spec","on":"property_changed","property_key":"spec.boundaries","type":"graph_event"}},{"actions":[{"action_type":"reject","params":{"message":"A superseded spec cannot be reinstated. Create a new spec instead."}}],"class":"invariant","conditions":["trigger.property.old_value == '\''superseded'\''","node.spec_status != '\''superseded'\''"],"name":"lock-superseded-spec-status","trigger":{"node_type":"spec","on":"property_changed","property_key":"spec.spec_status","type":"graph_event"}},{"actions":[{"action_type":"reject","params":{"message":"This plan is superseded, so its approach is locked as the record of what was agreed. Create a new plan for the revised version instead of editing this one."}}],"class":"invariant","conditions":["node.plan_status == '\''superseded'\''"],"name":"lock-superseded-plan-approach","trigger":{"node_type":"plan","on":"property_changed","property_key":"plan.approach","type":"graph_event"}},{"actions":[{"action_type":"reject","params":{"message":"This plan is superseded, so its risks is locked as the record of what was agreed. Create a new plan for the revised version instead of editing this one."}}],"class":"invariant","conditions":["node.plan_status == '\''superseded'\''"],"name":"lock-superseded-plan-risks","trigger":{"node_type":"plan","on":"property_changed","property_key":"plan.risks","type":"graph_event"}},{"actions":[{"action_type":"reject","params":{"message":"A superseded plan cannot be reinstated. Create a new plan instead."}}],"class":"invariant","conditions":["trigger.property.old_value == '\''superseded'\''","node.plan_status != '\''superseded'\''"],"name":"lock-superseded-plan-status","trigger":{"node_type":"plan","on":"property_changed","property_key":"plan.plan_status","type":"graph_event"}}]},"description":"Rejects edits to a superseded spec'\''s or plan'\''s content, and rejects moving it back out of superseded. Revisions are new nodes, so anything that referenced the old one still reads what it was built against.","rules":[{"actions":[{"action_type":"reject","params":{"message":"This spec is superseded, so its objective is locked as the record of what was agreed. Create a new spec for the revised version instead of editing this one."}}],"class":"invariant","conditions":["node.spec_status == '\''superseded'\''"],"name":"lock-superseded-spec-objective","trigger":{"node_type":"spec","on":"property_changed","property_key":"spec.objective","type":"graph_event"}},{"actions":[{"action_type":"reject","params":{"message":"This spec is superseded, so its success_criteria is locked as the record of what was agreed. Create a new spec for the revised version instead of editing this one."}}],"class":"invariant","conditions":["node.spec_status == '\''superseded'\''"],"name":"lock-superseded-spec-success_criteria","trigger":{"node_type":"spec","on":"property_changed","property_key":"spec.success_criteria","type":"graph_event"}},{"actions":[{"action_type":"reject","params":{"message":"This spec is superseded, so its boundaries is locked as the record of what was agreed. Create a new spec for the revised version instead of editing this one."}}],"class":"invariant","conditions":["node.spec_status == '\''superseded'\''"],"name":"lock-superseded-spec-boundaries","trigger":{"node_type":"spec","on":"property_changed","property_key":"spec.boundaries","type":"graph_event"}},{"actions":[{"action_type":"reject","params":{"message":"A superseded spec cannot be reinstated. Create a new spec instead."}}],"class":"invariant","conditions":["trigger.property.old_value == '\''superseded'\''","node.spec_status != '\''superseded'\''"],"name":"lock-superseded-spec-status","trigger":{"node_type":"spec","on":"property_changed","property_key":"spec.spec_status","type":"graph_event"}},{"actions":[{"action_type":"reject","params":{"message":"This plan is superseded, so its approach is locked as the record of what was agreed. Create a new plan for the revised version instead of editing this one."}}],"class":"invariant","conditions":["node.plan_status == '\''superseded'\''"],"name":"lock-superseded-plan-approach","trigger":{"node_type":"plan","on":"property_changed","property_key":"plan.approach","type":"graph_event"}},{"actions":[{"action_type":"reject","params":{"message":"This plan is superseded, so its risks is locked as the record of what was agreed. Create a new plan for the revised version instead of editing this one."}}],"class":"invariant","conditions":["node.plan_status == '\''superseded'\''"],"name":"lock-superseded-plan-risks","trigger":{"node_type":"plan","on":"property_changed","property_key":"plan.risks","type":"graph_event"}},{"actions":[{"action_type":"reject","params":{"message":"A superseded plan cannot be reinstated. Create a new plan instead."}}],"class":"invariant","conditions":["trigger.property.old_value == '\''superseded'\''","node.plan_status != '\''superseded'\''"],"name":"lock-superseded-plan-status","trigger":{"node_type":"plan","on":"property_changed","property_key":"plan.plan_status","type":"graph_event"}}]}'
```

### 4. Guidance skills

Skill nodes carrying usage guidance, discovered through the ordinary skill-search mechanism. Each is a `skill` root node whose markdown body becomes ordinary child nodes. Deliberately several narrow skills rather than one broad one: retrieval scores a precise match far better than a skill diluted across every intent.

Create each as below, then add its guidance as markdown children. The properties carry the whole retrieval surface — `description`, and an `exclusion` where the skill has one, which keeps general requests from ranking it above a built-in — so set them verbatim. The bodies are long-form prose; read them from `packages/core/src/methodology/skills/spec_driven/` rather than reproducing them here.

**Writing a Spec** — Write a spec before any plan or implementation: capture the objective, testable success criteria and boundaries (always do, ask first, never do). Use when the user says write a spec, spec this out, define requirements, or starts describing a feature or fix that has no spec yet.

```bash
nodespace node create --type skill --content 'Writing a Spec' \
  --properties '{"description":"Write a spec before any plan or implementation: capture the objective, testable success criteria and boundaries (always do, ask first, never do). Use when the user says write a spec, spec this out, define requirements, or starts describing a feature or fix that has no spec yet.","max_iterations":3,"tool_whitelist":["create_node","update_node","search_nodes","get_node"]}'
```

**Writing a Plan from a Spec** — Draft the technical plan for an approved spec: approach, components, sequencing and risks, linked back to the spec. Use when the user says plan this out, what's the approach, how should we build this, or asks for a plan for an existing spec.

```bash
nodespace node create --type skill --content 'Writing a Plan from a Spec' \
  --properties '{"description":"Draft the technical plan for an approved spec: approach, components, sequencing and risks, linked back to the spec. Use when the user says plan this out, what'\''s the approach, how should we build this, or asks for a plan for an existing spec.","max_iterations":3,"tool_whitelist":["create_node","update_node","create_relationship","search_nodes","get_node"]}'
```

**Creating Implementation Tasks** — Break an approved plan into tasks linked to both the plan and its spec. Use when the user says create tasks for this plan, break this down, let's start implementing, or asks to turn a plan into actionable work.

```bash
nodespace node create --type skill --content 'Creating Implementation Tasks' \
  --properties '{"description":"Break an approved plan into tasks linked to both the plan and its spec. Use when the user says create tasks for this plan, break this down, let'\''s start implementing, or asks to turn a plan into actionable work.","max_iterations":3,"tool_whitelist":["create_node","create_relationship","search_nodes","get_node"]}'
```

**Completing a Spec-Driven Task** — Close out a spec-driven task by recording how it was verified, then marking it done. Use when the user says a task is finished, wants to close it out, or asks to mark work done that traces to a spec or plan.

```bash
nodespace node create --type skill --content 'Completing a Spec-Driven Task' \
  --properties '{"description":"Close out a spec-driven task by recording how it was verified, then marking it done. Use when the user says a task is finished, wants to close it out, or asks to mark work done that traces to a spec or plan.","max_iterations":3,"tool_whitelist":["update_node","update_task_status","get_node"]}'
```

### 5. Saved views

A saved view is a node of type `query`. Its properties carry both the query (`target_type`, `filters`, `sorting`) and how it renders (`view_config`), so the board opens as authored with no per-user setup.

**Specs by Status**

```bash
nodespace node create --type query --content 'Specs by Status' \
  --properties '{"filters":[],"generated_by":"user","target_type":"spec","view_config":{"kanban":{"groupBy":"spec_status"},"lastView":"kanban"}}'
```

**Plans by Status**

```bash
nodespace node create --type query --content 'Plans by Status' \
  --properties '{"filters":[],"generated_by":"user","target_type":"plan","view_config":{"kanban":{"groupBy":"plan_status"},"lastView":"kanban"}}'
```

### 6. Workspace skill

One more skill, created after everything else because it names what you created. Title it exactly as shown: it is how an agent in this workspace later recognizes the Playbook is installed.

**Spec-driven Workspace** — What workflow this workspace uses: the Spec-driven Playbook installed here — its spec and plan types, how tasks trace back to them, the approval gates that refuse some changes, its saved views, and the schema ids they were actually created under.

Create it the same way as the guidance skills, and end its body with an "Installed in this workspace" section listing the types, Plays, guidance skills and views above, by id.
<!-- END GENERATED: spec-driven-playbook -->

## After installing

Tell the user what landed: two new node types, a `custom:verification_method`
field on `task`, four Plays, the guidance skills, and two saved views. Mention
that the gates check when work advances — approving a plan, starting or
finishing a task — not when nodes are created, and that tasks with no `plan` or
`spec` link are untouched.

This document is done at that point. Working in the installed workspace —
writing specs and plans, linking tasks, what the gates reject — is guidance the
install seeded into the graph: `nodespace skill guidance "Spec-driven Workspace"`.
