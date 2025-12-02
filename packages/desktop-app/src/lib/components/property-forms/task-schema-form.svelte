<!--
  TaskSchemaForm - Type-Safe Task Property Form (Issue #709)

  Hybrid approach:
  - Hardcoded UI for core task spoke fields (status, priority, dueDate, assignee)
  - Dynamic rendering for user-defined schema extensions

  Props:
  - nodeId: ID of the task node to display properties for
-->

<script lang="ts">
  import { Collapsible } from 'bits-ui';
  import * as Select from '$lib/components/ui/select';
  import * as Popover from '$lib/components/ui/popover';
  import { Calendar } from '$lib/components/ui/calendar';
  import { Input } from '$lib/components/ui/input';
  import { backendAdapter } from '$lib/services/backend-adapter';
  import { sharedNodeStore } from '$lib/services/shared-node-store.svelte';
  import {
    type SchemaNode,
    type SchemaField,
    type EnumValue,
    isSchemaNode
  } from '$lib/types/schema-node';
  import type { TaskNode, TaskStatus } from '$lib/types/task-node';
  import { parseDate, type DateValue } from '@internationalized/date';

  // Props - only nodeId needed since we know it's a task
  let { nodeId }: { nodeId: string } = $props();

  // State
  let isOpen = $state(false); // Collapsed by default
  let schema = $state<SchemaNode | null>(null);

  // Typed node state - TaskNode (not generic Node)
  let node = $state<TaskNode | null>(null);

  // Subscribe to node changes with type assertion
  $effect(() => {
    if (!nodeId) {
      node = null;
      return;
    }

    // Initial load - assert as TaskNode (validated by parent component)
    const rawNode = sharedNodeStore.getNode(nodeId);
    node = rawNode?.nodeType === 'task' ? (rawNode as unknown as TaskNode) : null;

    // Subscribe to updates
    const unsubscribe = sharedNodeStore.subscribe(nodeId, (updatedNode) => {
      node = updatedNode?.nodeType === 'task' ? (updatedNode as unknown as TaskNode) : null;
    });

    return () => {
      unsubscribe();
    };
  });

  // Load schema for user-defined extensions
  $effect(() => {
    async function loadSchema() {
      try {
        const schemaNode = await backendAdapter.getSchema('task');
        if (isSchemaNode(schemaNode)) {
          schema = schemaNode;
        }
      } catch (error) {
        console.error('[TaskSchemaForm] Failed to load schema:', error);
      }
    }
    loadSchema();
  });

  // Combobox state
  let assigneeOpen = $state(false);
  let assigneeSearch = $state('');
  let dueDateOpen = $state(false);

  /**
   * Assignee options - currently empty placeholder
   * TODO: Populate from UserService once implemented
   */
  const assigneeOptions: Array<{ value: string; label: string }> = [];

  // ============================================================================
  // Core Fields (Hardcoded)
  // ============================================================================

  const CORE_FIELD_NAMES = ['status', 'priority', 'dueDate', 'due_date', 'assignee'];

  const STATUS_OPTIONS: Array<{ value: TaskStatus; label: string }> = [
    { value: 'open', label: 'Open' },
    { value: 'in_progress', label: 'In Progress' },
    { value: 'done', label: 'Done' },
    { value: 'cancelled', label: 'Cancelled' }
  ];

  const PRIORITY_OPTIONS: Array<{ value: string; label: string }> = [
    { value: 'low', label: 'Low' },
    { value: 'medium', label: 'Medium' },
    { value: 'high', label: 'High' }
  ];

  // Get user-defined status extensions from schema
  const statusOptionsWithExtensions = $derived(() => {
    const options = [...STATUS_OPTIONS];

    if (schema) {
      const statusField = schema.fields.find((f) => f.name === 'status');
      if (statusField?.userValues) {
        // Add user-defined values that aren't already in core
        const coreValues = new Set(STATUS_OPTIONS.map((o) => o.value));
        for (const uv of statusField.userValues) {
          if (!coreValues.has(uv.value)) {
            options.push({ value: uv.value as TaskStatus, label: uv.label });
          }
        }
      }
    }

    return options;
  });

  // Get user-defined priority extensions from schema
  const priorityOptionsWithExtensions = $derived(() => {
    const options = [...PRIORITY_OPTIONS];

    if (schema) {
      const priorityField = schema.fields.find((f) => f.name === 'priority');
      if (priorityField?.userValues) {
        // Add user-defined values that aren't already in core
        const coreValues = new Set(PRIORITY_OPTIONS.map((o) => o.value));
        for (const uv of priorityField.userValues) {
          if (!coreValues.has(uv.value)) {
            options.push({ value: uv.value, label: uv.label });
          }
        }
      }
    }

    return options;
  });

  // Get user-defined fields (not core fields)
  const userDefinedFields = $derived(() => {
    if (!schema) return [];

    return schema.fields.filter((f) => !CORE_FIELD_NAMES.includes(f.name));
  });

  // Calculate field completion stats
  const fieldStats = $derived(() => {
    if (!node) return { filled: 0, total: 4 };

    let filled = 0;
    let total = 4; // Core fields

    // Core fields
    if (node.status) filled++;
    if (node.priority !== undefined && node.priority !== null) filled++;
    if (node.dueDate) filled++;
    if (node.assignee) filled++;

    // User-defined fields
    const userFields = userDefinedFields();
    total += userFields.length;

    for (const field of userFields) {
      const value = getUserFieldValue(field.name);
      if (value !== null && value !== undefined && value !== '') {
        filled++;
      }
    }

    return { filled, total };
  });

  // Get status label for header display
  const statusLabel = $derived(() => {
    if (!node) return null;
    const option = statusOptionsWithExtensions().find((o) => o.value === node?.status);
    return option?.label || node.status;
  });

  // ============================================================================
  // User-Defined Field Helpers
  // ============================================================================

  // Get value for a user-defined field from node properties
  function getUserFieldValue(fieldName: string): unknown {
    if (!node) return undefined;

    // User fields are stored in properties.task namespace
    const rawNode = sharedNodeStore.getNode(nodeId);
    if (!rawNode) return undefined;

    const taskProps = rawNode.properties?.task as Record<string, unknown> | undefined;
    return taskProps?.[fieldName] ?? rawNode.properties?.[fieldName];
  }

  // Update a user-defined field
  function updateUserField(fieldName: string, value: unknown) {
    if (!node) return;

    // User fields go through generic properties update
    const rawNode = sharedNodeStore.getNode(nodeId);
    if (!rawNode) return;

    const taskNamespace = (rawNode.properties?.task as Record<string, unknown>) || {};
    const updatedTaskNamespace = { ...taskNamespace, [fieldName]: value };

    sharedNodeStore.updateNode(
      nodeId,
      { properties: { ...rawNode.properties, task: updatedTaskNamespace } },
      { type: 'viewer', viewerId: 'task-schema-form' }
    );
  }

  // Get enum values for a schema field
  function getEnumValues(field: SchemaField): EnumValue[] {
    const values: EnumValue[] = [];
    if (field.coreValues) values.push(...field.coreValues);
    if (field.userValues) values.push(...field.userValues);
    return values;
  }

  // Format enum label
  function formatEnumLabel(value: string): string {
    return value
      .split('_')
      .map((word) => word.charAt(0).toUpperCase() + word.slice(1).toLowerCase())
      .join(' ');
  }

  // Format date for display
  function formatDateDisplay(value: string | null | undefined): string {
    if (!value) return 'Pick a date';
    try {
      const date = parseDate(value);
      return date.toString();
    } catch {
      return value;
    }
  }

  // Format date value for storage (ISO string)
  function formatDateForStorage(value: DateValue | undefined): string | null {
    if (!value) return null;
    return `${value.year}-${String(value.month).padStart(2, '0')}-${String(value.day).padStart(2, '0')}`;
  }

  // ============================================================================
  // Type-Safe Core Field Update Functions
  // ============================================================================

  function updateStatus(status: TaskStatus) {
    if (!node) return;
    sharedNodeStore.updateNode(nodeId, { status }, { type: 'viewer', viewerId: 'task-schema-form' });
  }

  function updatePriority(priority: string | undefined) {
    if (!node) return;
    sharedNodeStore.updateNode(
      nodeId,
      { priority: priority ?? null } as Record<string, unknown>,
      { type: 'viewer', viewerId: 'task-schema-form' }
    );
  }

  function updateDueDate(dueDate: string | null) {
    if (!node) return;
    sharedNodeStore.updateNode(
      nodeId,
      { dueDate },
      { type: 'viewer', viewerId: 'task-schema-form' }
    );
  }

  function updateAssignee(assignee: string | null) {
    if (!node) return;
    sharedNodeStore.updateNode(
      nodeId,
      { assignee },
      { type: 'viewer', viewerId: 'task-schema-form' }
    );
  }
</script>

{#if node}
  <div class="schema-form-wrapper">
    <Collapsible.Root bind:open={isOpen}>
      <Collapsible.Trigger
        class="flex w-full items-center justify-between py-3 font-medium transition-all hover:opacity-80"
      >
        <div class="flex items-center gap-3">
          {#if node.status}
            <span
              class="inline-flex items-center rounded-md border border-border bg-muted px-2.5 py-0.5 text-xs font-medium text-foreground"
            >
              {statusLabel()}
            </span>
          {/if}
          <span class="text-sm text-muted-foreground">
            Due: {node.dueDate ? formatDateDisplay(node.dueDate) : 'None'}
          </span>
        </div>

        <div class="flex items-center gap-2">
          <span class="text-sm text-muted-foreground">
            {fieldStats().filled}/{fieldStats().total} fields
          </span>
          <svg
            class="h-4 w-4 text-muted-foreground transition-transform duration-200"
            class:rotate-180={isOpen}
            viewBox="0 0 16 16"
            fill="none"
          >
            <path
              d="M4 6l4 4 4-4"
              stroke="currentColor"
              stroke-width="2"
              stroke-linecap="round"
              stroke-linejoin="round"
            />
          </svg>
        </div>
      </Collapsible.Trigger>

      <Collapsible.Content class="pb-4">
        <div class="grid grid-cols-2 gap-4">
          <!-- ============================================================ -->
          <!-- CORE FIELDS (Hardcoded, Type-Safe) -->
          <!-- ============================================================ -->

          <!-- Status Field -->
          <div class="space-y-2">
            <label for="task-status" class="text-sm font-medium">Status</label>
            <Select.Root
              type="single"
              value={node.status}
              onValueChange={(newValue) => {
                if (newValue) updateStatus(newValue as TaskStatus);
              }}
            >
              <Select.Trigger class="w-full">
                {statusOptionsWithExtensions().find((o) => o.value === node?.status)?.label ||
                  node?.status ||
                  'Select status...'}
              </Select.Trigger>
              <Select.Content>
                {#each statusOptionsWithExtensions() as option}
                  <Select.Item value={option.value} label={option.label} />
                {/each}
              </Select.Content>
            </Select.Root>
          </div>

          <!-- Priority Field -->
          <div class="space-y-2">
            <label for="task-priority" class="text-sm font-medium">Priority</label>
            <Select.Root
              type="single"
              value={node.priority !== undefined && node.priority !== null
                ? String(node.priority)
                : ''}
              onValueChange={(newValue) => updatePriority(newValue || undefined)}
            >
              <Select.Trigger class="w-full">
                {#if node.priority !== undefined && node.priority !== null}
                  {priorityOptionsWithExtensions().find((o) => o.value === String(node?.priority))
                    ?.label || String(node?.priority)}
                {:else}
                  <span class="text-muted-foreground">Select priority...</span>
                {/if}
              </Select.Trigger>
              <Select.Content>
                {#each priorityOptionsWithExtensions() as option}
                  <Select.Item value={option.value} label={option.label} />
                {/each}
              </Select.Content>
            </Select.Root>
          </div>

          <!-- Due Date Field -->
          {#if node}
            {@const dateValue = node.dueDate ? parseDate(node.dueDate) : undefined}
            <div class="space-y-2">
              <label for="task-due-date" class="text-sm font-medium">Due Date</label>
              <Popover.Root bind:open={dueDateOpen}>
              <Popover.Trigger
                id="task-due-date"
                class="flex h-10 w-full items-center justify-between rounded-md border border-input bg-background px-3 py-2 text-sm focus-visible:outline-none"
              >
                <span class={dateValue ? '' : 'text-muted-foreground'}>
                  {formatDateDisplay(node.dueDate)}
                </span>
                <svg class="h-4 w-4 opacity-50" viewBox="0 0 16 16" fill="none">
                  <rect
                    x="2"
                    y="3"
                    width="12"
                    height="11"
                    rx="1"
                    stroke="currentColor"
                    stroke-width="1.5"
                  />
                  <path
                    d="M5 1v3M11 1v3M2 6h12"
                    stroke="currentColor"
                    stroke-width="1.5"
                    stroke-linecap="round"
                  />
                </svg>
              </Popover.Trigger>
              <Popover.Content class="w-auto p-0" align="start">
                <!-- Cast to `never` works around bits-ui Calendar expecting CalendarDate but we pass
                     DateValue from @internationalized/date. Both are compatible at runtime. -->
                <Calendar
                  value={dateValue as never}
                  onValueChange={(newValue: DateValue | DateValue[] | undefined) => {
                    const singleValue = Array.isArray(newValue) ? newValue[0] : newValue;
                    updateDueDate(formatDateForStorage(singleValue));
                    dueDateOpen = false;
                  }}
                  type="single"
                />
              </Popover.Content>
            </Popover.Root>
            </div>
          {/if}

          <!-- Assignee Field -->
          <div class="space-y-2">
            <label for="task-assignee" class="text-sm font-medium">Assignee</label>
            <Popover.Root bind:open={assigneeOpen}>
              <Popover.Trigger
                class="flex h-10 w-full items-center justify-between rounded-md border border-input bg-background px-3 py-2 text-sm focus-visible:outline-none"
              >
                <span class={node.assignee ? '' : 'text-muted-foreground'}>
                  {node.assignee || 'Select assignee...'}
                </span>
                <svg class="h-4 w-4 opacity-50" viewBox="0 0 16 16" fill="none">
                  <path
                    d="M4 6l4 4 4-4"
                    stroke="currentColor"
                    stroke-width="2"
                    stroke-linecap="round"
                    stroke-linejoin="round"
                  />
                </svg>
              </Popover.Trigger>
              <Popover.Content class="w-[200px] p-0" align="start">
                <div class="flex flex-col">
                  <input
                    type="text"
                    placeholder="Search assignee..."
                    value={assigneeSearch}
                    oninput={(e) => (assigneeSearch = e.currentTarget.value)}
                    class="flex h-10 w-full border-b border-input bg-background px-3 py-2 text-sm focus-visible:outline-none"
                  />
                  <div class="max-h-[200px] overflow-y-auto">
                    {#if assigneeOptions.length === 0}
                      <div class="px-3 py-2 text-sm text-muted-foreground">
                        No assignees available
                      </div>
                    {:else}
                      {#each assigneeOptions.filter((a) => a.label
                          .toLowerCase()
                          .includes(assigneeSearch.toLowerCase())) as assignee}
                        <button
                          type="button"
                          class="relative flex w-full cursor-pointer select-none items-center rounded-sm px-3 py-2 text-sm outline-none hover:bg-muted"
                          onclick={() => {
                            updateAssignee(assignee.value);
                            assigneeOpen = false;
                            assigneeSearch = '';
                          }}
                        >
                          {assignee.label}
                          {#if node?.assignee === assignee.value}
                            <svg class="ml-auto h-4 w-4" viewBox="0 0 16 16" fill="none">
                              <path
                                d="M3 8l4 4 6-8"
                                stroke="currentColor"
                                stroke-width="2"
                                stroke-linecap="round"
                                stroke-linejoin="round"
                              />
                            </svg>
                          {/if}
                        </button>
                      {/each}
                    {/if}
                  </div>
                </div>
              </Popover.Content>
            </Popover.Root>
          </div>

          <!-- ============================================================ -->
          <!-- USER-DEFINED FIELDS (Dynamic from Schema) -->
          <!-- ============================================================ -->

          {#each userDefinedFields() as field (field.name)}
            {@const fieldId = `task-user-${field.name}`}
            <div class="space-y-2">
              <label for={fieldId} class="text-sm font-medium">
                {field.description || formatEnumLabel(field.name)}
              </label>

              {#if field.type === 'enum'}
                {@const enumValues = getEnumValues(field)}
                {@const currentValue = (getUserFieldValue(field.name) as string) || ''}
                <Select.Root
                  type="single"
                  value={currentValue}
                  onValueChange={(newValue) => updateUserField(field.name, newValue)}
                >
                  <Select.Trigger class="w-full">
                    {enumValues.find((ev) => ev.value === currentValue)?.label ||
                      currentValue ||
                      `Select ${field.name}...`}
                  </Select.Trigger>
                  <Select.Content>
                    {#each enumValues as ev}
                      <Select.Item value={ev.value} label={ev.label} />
                    {/each}
                  </Select.Content>
                </Select.Root>
              {:else if field.type === 'date'}
                {@const rawDateValue = getUserFieldValue(field.name) as string | null}
                {@const dateVal = rawDateValue ? parseDate(rawDateValue) : undefined}
                <Popover.Root>
                  <Popover.Trigger
                    id={fieldId}
                    class="flex h-10 w-full items-center justify-between rounded-md border border-input bg-background px-3 py-2 text-sm focus-visible:outline-none"
                  >
                    <span class={dateVal ? '' : 'text-muted-foreground'}>
                      {formatDateDisplay(rawDateValue)}
                    </span>
                    <svg class="h-4 w-4 opacity-50" viewBox="0 0 16 16" fill="none">
                      <rect
                        x="2"
                        y="3"
                        width="12"
                        height="11"
                        rx="1"
                        stroke="currentColor"
                        stroke-width="1.5"
                      />
                      <path
                        d="M5 1v3M11 1v3M2 6h12"
                        stroke="currentColor"
                        stroke-width="1.5"
                        stroke-linecap="round"
                      />
                    </svg>
                  </Popover.Trigger>
                  <Popover.Content class="w-auto p-0" align="start">
                    <!-- Cast to `never` works around bits-ui Calendar type mismatch (see core dueDate field comment) -->
                    <Calendar
                      value={dateVal as never}
                      onValueChange={(newValue: DateValue | DateValue[] | undefined) => {
                        const singleValue = Array.isArray(newValue) ? newValue[0] : newValue;
                        updateUserField(field.name, formatDateForStorage(singleValue));
                      }}
                      type="single"
                    />
                  </Popover.Content>
                </Popover.Root>
              {:else if field.type === 'text' || field.type === 'string'}
                <Input
                  id={fieldId}
                  type="text"
                  value={(getUserFieldValue(field.name) as string) || ''}
                  oninput={(e) => updateUserField(field.name, e.currentTarget.value)}
                  placeholder={field.default ? String(field.default) : ''}
                />
              {:else if field.type === 'number'}
                <Input
                  id={fieldId}
                  type="number"
                  value={(getUserFieldValue(field.name) as number) || field.default || 0}
                  oninput={(e) => updateUserField(field.name, parseFloat(e.currentTarget.value) || 0)}
                />
              {:else}
                <div class="text-sm text-muted-foreground">Unknown field type: {field.type}</div>
              {/if}
            </div>
          {/each}
        </div>
      </Collapsible.Content>
    </Collapsible.Root>
  </div>
{/if}

<style>
  /* Extend border to container edges using negative margins */
  .schema-form-wrapper {
    width: calc(100% + (var(--viewer-padding-horizontal) * 2));
    margin-left: calc(-1 * var(--viewer-padding-horizontal));
    padding: 0 var(--viewer-padding-horizontal);
    border-bottom: 1px solid hsl(var(--border));
  }
</style>
