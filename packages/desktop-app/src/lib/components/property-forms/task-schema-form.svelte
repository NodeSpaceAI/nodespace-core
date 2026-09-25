<!--
  TaskSchemaForm - Type-Safe Task Property Form

  Hybrid approach:
  - Core task properties (status, priority, dueDate, startedAt, completedAt) render
    through the shared SchemaFieldLeaf, driven by the real task schema's coreValues/
    userValues — no locally hardcoded enum options or date-picker markup. Their WRITES
    still go through the type-safe sharedNodeStore.updateTaskNode() functions below,
    unchanged: SchemaFieldLeaf is a controlled, presentational component (value + onChange)
    that never touches the store itself, so swapping its markup in does not change how a
    core field is persisted (same optimistic-write/OCC/field-sequencing path as before).
  - Dynamic rendering for user-defined schema extensions: leaf fields render through the
    shared SchemaFieldLeaf, object/array fields render a summary trigger that opens the
    shared NestedPropertyModal.
  - Shell chrome (Collapsible, trigger row, gated Relationships button, NestedPropertyModal)
    is owned by TypedFormShell — this component supplies only the task-specific field grid.

  User-defined values (nested ones included) are read and written flat, as
  properties[field.name], via getUserFieldValue/updateUserField.

  Props:
  - nodeId: ID of the task node to display properties for
-->

<script lang="ts">
  import { onMount } from 'svelte';
  import { backendAdapter } from '$lib/services/backend-adapter';
  import { sharedNodeStore } from '$lib/services/shared-node-store.svelte';
  import { type SchemaNode, type SchemaField, isSchemaNode } from '$lib/types/schema-node';
  import type { TaskStatus } from '$lib/types/task-node';
  import { nodeToTaskNode } from '$lib/types/task-node';
  import { createLogger } from '$lib/utils/logger';
  import { labelForField } from '$lib/utils/schema-field-label';
  import { enumValueLabel } from '$lib/utils/schema-enum-values';
  import { formatDateDisplay } from '$lib/utils/schema-date-values';
  import { isUserVisibleField } from '$lib/utils/schema-field-visibility';
  import SchemaFieldLeaf from '$lib/components/schema/schema-field-leaf.svelte';
  import NestedFieldTrigger from '$lib/components/schema/nested-field-trigger.svelte';
  import TypedFormShell from '$lib/components/schema/typed-form-shell.svelte';
  import { isNestedField } from '$lib/utils/nested-property-ops';

  // Logger instance for TaskSchemaForm component
  const log = createLogger('TaskSchemaForm');

  // Props - only nodeId needed since we know it's a task
  let { nodeId }: { nodeId: string } = $props();

  // State
  let schema = $state<SchemaNode | null>(null);
  // True once the schema fetch has settled without producing a usable schema
  // (a thrown error, or a response that fails isSchemaNode) — distinct from
  // "still loading", so a core field's control can tell the two apart instead
  // of looking identically blank in both states. See the per-field "unavailable"
  // fallback in the template below.
  let schemaLoadFailed = $state(false);

  // Reactive node data - direct read from the store's SvelteMap, converted to TaskNode
  const node = $derived.by(() => {
    if (!nodeId) return null;
    const rawNode = sharedNodeStore.getNode(nodeId);
    return rawNode?.nodeType === 'task' ? nodeToTaskNode(rawNode) : null;
  });

  // Load the task schema once on mount (constant type — never re-fetches). ADR-049:
  // a mount-time load, not a reactive-state watch.
  onMount(() => {
    async function loadSchema() {
      try {
        const schemaNode = await backendAdapter.getSchema('task');
        if (isSchemaNode(schemaNode)) {
          schema = schemaNode;
        } else {
          schemaLoadFailed = true;
        }
      } catch (error) {
        log.error('Failed to load schema:', error);
        schemaLoadFailed = true;
      }
    }
    loadSchema();
  });

  // ============================================================================
  // Core Fields
  // ============================================================================
  // Options/labels come straight from the real task schema's coreValues/userValues
  // (SchemaFieldLeaf reads them the same way) — no locally hardcoded enum lists.

  // The real task schema's own field names (core_schemas.rs) — always snake_case,
  // matching what the backend actually returns from getSchema('task').
  const CORE_FIELD_NAMES = ['status', 'priority', 'due_date', 'started_at', 'completed_at'];

  // A schema field by name, or undefined while `schema` hasn't loaded yet (or, in the
  // unexpected case of a schema-fetch failure — see loadSchema's catch above — never).
  // Each core field's SchemaFieldLeaf is individually gated on its own lookup succeeding
  // (see the template below) rather than gating the whole form on `schema`, so a slow or
  // failed schema fetch degrades to "this one field's control is momentarily/permanently
  // unavailable" rather than blanking the entire properties panel.
  function getSchemaField(name: string): SchemaField | undefined {
    return schema?.fields.find((f) => f.name === name);
  }

  // Get user-defined fields: not core, and user-visible per the shared predicate.
  // The two conjuncts answer different questions and deliberately stay separate — core
  // fields are excluded because they already render through their own dedicated controls
  // above, which is specific to this form, whereas isUserVisibleField answers "may a user
  // ever see this?" for every surface that renders schema fields.
  const userDefinedFields = $derived.by(() => {
    if (!schema) return [];
    return schema.fields.filter((f) => !CORE_FIELD_NAMES.includes(f.name) && isUserVisibleField(f));
  });

  // Calculate field completion stats
  const fieldStats = $derived.by(() => {
    if (!node) return { filled: 0, total: 5 };

    let filled = 0;
    let total = 5; // Core fields: status, priority, dueDate, startedAt, completedAt

    // Core fields
    if (node.status) filled++;
    if (node.priority !== undefined && node.priority !== null) filled++;
    if (node.dueDate) filled++;
    if (node.startedAt) filled++;
    if (node.completedAt) filled++;

    // User-defined fields
    const userFields = userDefinedFields;
    total += userFields.length;

    for (const field of userFields) {
      const value = getUserFieldValue(field.name);
      if (value !== null && value !== undefined && value !== '') {
        filled++;
      }
    }

    return { filled, total };
  });

  // Get status label for header display — same coreValues/userValues lookup
  // SchemaFieldLeaf uses for the field control itself, so the collapsed header
  // and the open control always agree on how a status value is humanized.
  const statusLabel = $derived.by(() => {
    if (!node) return null;
    const statusField = getSchemaField('status');
    if (!statusField) return node.status;
    return enumValueLabel(statusField, node.status) ?? node.status;
  });

  // ============================================================================
  // User-Defined Field Helpers
  // ============================================================================

  // User-defined fields are read and written flat, under `properties.<fieldName>` —
  // the shape every transport delivers. The backend moves bare keys into the
  // type's storage bucket on write.
  function getUserFieldValue(fieldName: string): unknown {
    if (!node) return undefined;
    return sharedNodeStore.getNode(nodeId)?.properties?.[fieldName];
  }

  function updateUserField(fieldName: string, value: unknown) {
    if (!node) return;

    const rawNode = sharedNodeStore.getNode(nodeId);
    if (!rawNode) return;

    sharedNodeStore.updateNode(
      nodeId,
      { properties: { ...rawNode.properties, [fieldName]: value } },
      { type: 'viewer', viewerId: 'task-schema-form' }
    );
  }

  // ============================================================================
  // Type-Safe Core Field Update Functions
  // ============================================================================
  // Use sharedNodeStore.updateTaskNode for type-safe task property updates

  function updateStatus(status: TaskStatus) {
    if (!node) return;
    sharedNodeStore.updateTaskNode(nodeId, { status }, { type: 'viewer', viewerId: 'task-schema-form' });
  }

  function updatePriority(priority: string | undefined) {
    if (!node) return;
    sharedNodeStore.updateTaskNode(
      nodeId,
      { priority: priority ?? null },
      { type: 'viewer', viewerId: 'task-schema-form' }
    );
  }

  function updateDueDate(dueDate: string | null) {
    if (!node) return;
    sharedNodeStore.updateTaskNode(
      nodeId,
      { dueDate },
      { type: 'viewer', viewerId: 'task-schema-form' }
    );
  }

  function updateStartedAt(startedAt: string | null) {
    if (!node) return;
    sharedNodeStore.updateTaskNode(
      nodeId,
      { startedAt },
      { type: 'viewer', viewerId: 'task-schema-form' }
    );
  }

  function updateCompletedAt(completedAt: string | null) {
    if (!node) return;
    sharedNodeStore.updateTaskNode(
      nodeId,
      { completedAt },
      { type: 'viewer', viewerId: 'task-schema-form' }
    );
  }
</script>

{#if node}
  <TypedFormShell
    {nodeId}
    {fieldStats}
    getFieldValue={getUserFieldValue}
    onFieldChange={updateUserField}
  >
    {#snippet headerLeft()}
      {#if node.status}
        <span
          class="inline-flex items-center rounded-md border border-border bg-muted px-2.5 py-0.5 text-xs font-medium text-foreground"
        >
          {statusLabel}
        </span>
      {/if}
      <span class="text-sm text-muted-foreground">
        Due: {node.dueDate ? formatDateDisplay(node.dueDate) : 'None'}
      </span>
    {/snippet}

    {#snippet fields(openNestedModal: (_field: SchemaField) => void)}
      {@const statusField = getSchemaField('status')}
      {@const priorityField = getSchemaField('priority')}
      {@const dueDateField = getSchemaField('due_date')}
      {@const startedAtField = getSchemaField('started_at')}
      {@const completedAtField = getSchemaField('completed_at')}
      <!-- Placeholder for a core field whose schema lookup hasn't resolved yet — matches
           a SchemaFieldLeaf control's height so the grid doesn't jump once it appears.
           Blank while the fetch is still in flight (typically sub-frame; not worth a
           spinner), a visible hint once it's permanently failed (see schemaLoadFailed). -->
      {#snippet fieldUnavailable()}
        <div class="flex h-10 items-center text-sm text-muted-foreground">
          {schemaLoadFailed ? 'Unable to load' : ''}
        </div>
      {/snippet}
      <div class="grid grid-cols-2 gap-4">
        <!-- ============================================================ -->
        <!-- CORE FIELDS -->
        <!-- ============================================================ -->

        <!-- Status Field -->
        <div class="space-y-2">
          <label for="task-status" class="text-sm font-medium">Status</label>
          {#if statusField}
            <SchemaFieldLeaf
              field={statusField}
              fieldId="task-status"
              value={node.status}
              onChange={(newValue) => {
                // Status is required — guard against a falsy write the way the
                // pre-refactor inline Select.Root did (`if (newValue) ...`). Not
                // reachable today (bits-ui's Select toggle-to-empty needs
                // allowDeselect, which this app's Select wrapper never sets), but
                // cheap defense-in-depth to keep on a required field.
                if (newValue) updateStatus(newValue as TaskStatus);
              }}
            />
          {:else}
            {@render fieldUnavailable()}
          {/if}
        </div>

        <!-- Priority Field -->
        <div class="space-y-2">
          <label for="task-priority" class="text-sm font-medium">Priority</label>
          {#if priorityField}
            <SchemaFieldLeaf
              field={priorityField}
              fieldId="task-priority"
              value={node.priority !== undefined && node.priority !== null ? String(node.priority) : ''}
              onChange={(newValue) => updatePriority((newValue as string) || undefined)}
            />
          {:else}
            {@render fieldUnavailable()}
          {/if}
        </div>

        <!-- Due Date Field -->
        <div class="space-y-2">
          <label for="task-due-date" class="text-sm font-medium">Due Date</label>
          {#if dueDateField}
            <SchemaFieldLeaf
              field={dueDateField}
              fieldId="task-due-date"
              value={node.dueDate}
              onChange={(newValue) => updateDueDate(newValue as string | null)}
            />
          {:else}
            {@render fieldUnavailable()}
          {/if}
        </div>

        <!-- Started At Field -->
        <div class="space-y-2">
          <label for="task-started-at" class="text-sm font-medium">Started At</label>
          {#if startedAtField}
            <SchemaFieldLeaf
              field={startedAtField}
              fieldId="task-started-at"
              value={node.startedAt}
              onChange={(newValue) => updateStartedAt(newValue as string | null)}
            />
          {:else}
            {@render fieldUnavailable()}
          {/if}
        </div>

        <!-- Completed At Field -->
        <div class="space-y-2">
          <label for="task-completed-at" class="text-sm font-medium">Completed At</label>
          {#if completedAtField}
            <SchemaFieldLeaf
              field={completedAtField}
              fieldId="task-completed-at"
              value={node.completedAt}
              onChange={(newValue) => updateCompletedAt(newValue as string | null)}
            />
          {:else}
            {@render fieldUnavailable()}
          {/if}
        </div>

        <!-- ============================================================ -->
        <!-- USER-DEFINED FIELDS (Dynamic from Schema) -->
        <!-- ============================================================ -->

        {#each userDefinedFields as field (field.name)}
          {@const fieldId = `task-user-${field.name}`}
          <div class="space-y-2">
            <label for={fieldId} class="text-sm font-medium">
              {labelForField(field)}
            </label>

            {#if isNestedField(field)}
              <NestedFieldTrigger
                {field}
                {fieldId}
                value={getUserFieldValue(field.name)}
                onopen={() => openNestedModal(field)}
              />
            {:else}
              <SchemaFieldLeaf
                {field}
                {fieldId}
                value={getUserFieldValue(field.name)}
                onChange={(newValue) => updateUserField(field.name, newValue)}
              />
            {/if}
          </div>
        {/each}
      </div>
    {/snippet}
  </TypedFormShell>
{/if}
