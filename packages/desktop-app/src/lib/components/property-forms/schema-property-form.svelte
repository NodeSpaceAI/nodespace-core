<!--
  SchemaPropertyForm - Schema-Driven Property UI

  Dynamically generates form fields based on schema definitions.
  Supports enum (Select), date (Calendar), text (Input), and other field types.
  Uses Collapsible UI pattern with field completion tracking.

  Props:
  - nodeId: ID of the node to display properties for
  - nodeType: Type of the node (used to fetch schema)

  Features:
  - Automatic field type detection and rendering
  - Field completion tracking (filled fields / total fields)
  - Collapsible section with status summary in header
  - Reactive updates to node properties
  - Schema caching for performance
-->

<script lang="ts">
  import { Collapsible } from 'bits-ui';
  import * as Select from '$lib/components/ui/select';
  import * as Popover from '$lib/components/ui/popover';
  import { Calendar } from '$lib/components/ui/calendar';
  import { Input } from '$lib/components/ui/input';
  import { schemaService } from '$lib/services/schema-service';
  import { sharedNodeStore } from '$lib/services/shared-node-store';
  import type { SchemaDefinition, SchemaField } from '$lib/types/schema';
  import type { Node } from '$lib/types';
  import { parseDate, type DateValue } from '@internationalized/date';

  // Props
  let {
    nodeId,
    nodeType
  }: {
    nodeId: string;
    nodeType: string;
  } = $props();

  // State
  let schema = $state<SchemaDefinition | null>(null);
  let isOpen = $state(false); // Collapsed by default
  let isLoadingSchema = $state(false);
  let schemaError = $state<string | null>(null);

  // Reactive node data - updates when store changes via subscription
  let node = $state<Node | null>(nodeId ? sharedNodeStore.getNode(nodeId) || null : null);

  // Subscribe to node changes
  $effect(() => {
    if (!nodeId) {
      node = null;
      return;
    }

    // Initial load
    node = sharedNodeStore.getNode(nodeId) || null;

    // Subscribe to updates
    const unsubscribe = sharedNodeStore.subscribe(nodeId, (updatedNode) => {
      node = updatedNode;
    });

    return () => {
      unsubscribe();
    };
  });

  // Combobox state for text fields that could have autocomplete (like assignee)
  let comboboxOpen = $state<Record<string, boolean>>({});
  let comboboxSearch = $state<Record<string, string>>({});

  /**
   * Assignee options - currently empty placeholder
   *
   * TODO: Populate from UserService once implemented
   * - Will integrate with user management system (planned)
   * - Should provide autocomplete for user names/emails
   * - Consider caching user list for performance
   *
   * Related: User service integration (future enhancement)
   */
  const assigneeOptions: Array<{ value: string; label: string }> = [];

  // Load schema when nodeType changes
  $effect(() => {
    async function loadSchema() {
      if (!nodeType) return;

      isLoadingSchema = true;
      schemaError = null;

      try {
        const schemaDefinition = await schemaService.getSchema(nodeType);
        schema = schemaDefinition;
      } catch (error) {
        console.error('[SchemaPropertyForm] Failed to load schema:', error);
        schemaError = error instanceof Error ? error.message : 'Failed to load schema';
        schema = null;
      } finally {
        isLoadingSchema = false;
      }
    }

    loadSchema();
  });

  // Calculate field completion stats
  const fieldStats = $derived(() => {
    if (!schema || !node) {
      return { filled: 0, total: 0 };
    }

    // Count all fields (core, user, and system)
    const allFields = schema.fields;
    const total = allFields.length;

    // Count filled fields (non-null, non-undefined, non-empty)
    const filled = allFields.filter((field) => {
      // Type guard to ensure node is not null
      if (!node) return false;
      const value = node.properties?.[field.name];
      if (value === null || value === undefined) return false;
      if (typeof value === 'string' && value.trim() === '') return false;
      return true;
    }).length;

    return { filled, total };
  });

  // Get display value for header (e.g., status badge, due date)
  const headerSummary = $derived(() => {
    if (!schema || !node) return null;

    // Find status field (enum type, common in task schemas)
    const statusField = schema.fields.find((f) => f.name === 'status' && f.type === 'enum');
    // Use current value or default value from schema
    const statusValue = statusField
      ? node.properties?.[statusField.name] || statusField.default || null
      : null;
    // Ensure status is a string - handle arrays incorrectly stored
    let status: string | null = null;
    if (statusValue) {
      if (Array.isArray(statusValue)) {
        status = statusValue.join(''); // Fix incorrectly stored array
      } else {
        status = String(statusValue);
      }
    }

    // Find due date field
    const dueDateField = schema.fields.find((f) => f.name === 'dueDate' || f.name === 'due_date');
    const dueDate = dueDateField ? node.properties?.[dueDateField.name] : null;

    return { status, dueDate };
  });

  // Update node property
  function updateProperty(fieldName: string, value: unknown) {
    if (!node) return;

    const updatedProperties = {
      ...node.properties,
      [fieldName]: value
    };

    sharedNodeStore.updateNode(
      nodeId,
      { properties: updatedProperties },
      { type: 'viewer', viewerId: 'schema-property-form' }
    );
  }

  // Get enum values for a field (core + user values combined)
  function getEnumValues(field: SchemaField): string[] {
    const values: string[] = [];
    if (field.coreValues) values.push(...field.coreValues);
    if (field.userValues) values.push(...field.userValues);
    return values;
  }

  // Convert between different value formats
  function parsePropertyValue(field: SchemaField, value: unknown): unknown {
    if (value === null || value === undefined) return value;

    switch (field.type) {
      case 'date':
        if (typeof value === 'string') {
          try {
            return parseDate(value);
          } catch {
            return undefined;
          }
        }
        return value;
      case 'number':
        return typeof value === 'string' ? parseFloat(value) : value;
      case 'boolean':
        return typeof value === 'string' ? value === 'true' : value;
      default:
        return value;
    }
  }

  // Format date for display
  function formatDateDisplay(value: unknown): string {
    if (!value) return 'Pick a date';
    if (typeof value === 'string') {
      try {
        const date = parseDate(value);
        return date.toString();
      } catch {
        return value;
      }
    }
    if (typeof value === 'object' && 'toString' in value) {
      return (value as DateValue).toString();
    }
    return String(value);
  }

  // Format date value for storage (ISO string)
  function formatDateForStorage(value: DateValue | undefined): string | null {
    if (!value) return null;
    return `${value.year}-${String(value.month).padStart(2, '0')}-${String(value.day).padStart(2, '0')}`;
  }

  // Format enum value for display (convert SCREAMING_SNAKE_CASE to Title Case)
  function formatEnumLabel(value: string): string {
    return value
      .split('_')
      .map((word) => word.charAt(0) + word.slice(1).toLowerCase())
      .join(' ');
  }

  // Get current enum value as string (with fallback to default)
  function getEnumValue(field: SchemaField): string {
    if (!node) return field.default ? String(field.default) : '';
    const value = node.properties?.[field.name];
    // Use current value, or fall back to schema default, or empty string
    return value ? String(value) : field.default ? String(field.default) : '';
  }
</script>

{#if isLoadingSchema}
  <div class="property-form-loading">
    <span class="text-sm text-muted-foreground">Loading schema...</span>
  </div>
{:else if schemaError}
  <div class="property-form-error">
    <span class="text-sm text-destructive">Error: {schemaError}</span>
  </div>
{:else if schema && node && fieldStats().total > 0}
  <!-- Wrapper with border-b (matches demo structure) -->
  <div class="border-b">
    <Collapsible.Root bind:open={isOpen}>
      <Collapsible.Trigger
        class="flex w-full items-center justify-between py-3 font-medium transition-all hover:opacity-80"
      >
        <div class="flex items-center gap-3">
          <!-- Status Badge (if available) -->
          {#if headerSummary()?.status}
            {@const status = headerSummary()!.status!}
            <span
              class="inline-flex items-center rounded-md border border-border bg-muted px-2.5 py-0.5 text-xs font-medium text-foreground"
            >
              {formatEnumLabel(status)}
            </span>
          {/if}

          <!-- Due Date (always show, with "None" if not set) -->
          <span class="text-sm text-muted-foreground">
            Due: {headerSummary()?.dueDate ? formatDateDisplay(headerSummary()?.dueDate) : 'None'}
          </span>
        </div>

        <!-- Field Completion + Chevron -->
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
        <!-- Property Grid (2 columns) -->
        <div class="grid grid-cols-2 gap-4">
          {#each schema.fields as field (field.name)}
            {@const fieldId = `property-${nodeId}-${field.name}`}
            <div class="space-y-2">
              <label for={fieldId} class="text-sm font-medium">
                {field.description || formatEnumLabel(field.name)}
              </label>

              {#if field.type === 'enum'}
                <!-- Enum Field → shadcn Select Component (Fixed with bind:value) -->
                {@const enumValues = getEnumValues(field)}
                <Select.Root
                  type="single"
                  value={getEnumValue(field)}
                  onValueChange={(newValue) => {
                    if (newValue) {
                      updateProperty(field.name, newValue);
                    }
                  }}
                >
                  <Select.Trigger class="w-full">
                    {formatEnumLabel(getEnumValue(field))}
                  </Select.Trigger>
                  <Select.Content>
                    {#each enumValues as enumValue}
                      <Select.Item value={enumValue} label={formatEnumLabel(enumValue)} />
                    {/each}
                  </Select.Content>
                </Select.Root>
              {:else if field.type === 'date'}
                <!-- Date Field → Calendar with Popover -->
                {@const rawDateValue = parsePropertyValue(field, node.properties?.[field.name])}
                {@const dateValue = (rawDateValue ? rawDateValue : undefined) as
                  | DateValue
                  | DateValue[]
                  | undefined}
                {@const popoverOpenKey = `date_${field.name}`}
                {#if comboboxOpen[popoverOpenKey] === undefined}
                  {((comboboxOpen[popoverOpenKey] = false), '')}
                {/if}
                <Popover.Root bind:open={comboboxOpen[popoverOpenKey]}>
                  <Popover.Trigger
                    id={fieldId}
                    class="flex h-10 w-full items-center justify-between rounded-md border border-input bg-background px-3 py-2 text-sm focus-visible:outline-none"
                  >
                    <span class={dateValue ? '' : 'text-muted-foreground'}>
                      {formatDateDisplay(dateValue)}
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
                    <Calendar
                      value={dateValue as never}
                      onValueChange={(newValue: DateValue | DateValue[] | undefined) => {
                        // Handle single date value (not array)
                        const singleValue = Array.isArray(newValue) ? newValue[0] : newValue;
                        updateProperty(field.name, formatDateForStorage(singleValue));
                        // Close the popover after selecting a date
                        comboboxOpen[popoverOpenKey] = false;
                      }}
                      type="single"
                    />
                  </Popover.Content>
                </Popover.Root>
              {:else if field.type === 'text' || field.type === 'string'}
                <!-- Text Field → Combobox for assignee, Input for others -->
                {#if field.name === 'assignee'}
                  <!-- Assignee Combobox (matches demo structure) -->
                  {@const currentValue = (node.properties?.[field.name] as string) || ''}
                  {@const isOpen = comboboxOpen[field.name] || false}
                  {@const searchValue = comboboxSearch[field.name] || ''}
                  <Popover.Root
                    open={isOpen}
                    onOpenChange={(open) => {
                      comboboxOpen[field.name] = open;
                    }}
                  >
                    <Popover.Trigger
                      class="flex h-10 w-full items-center justify-between rounded-md border border-input bg-background px-3 py-2 text-sm focus-visible:outline-none"
                    >
                      <span class={currentValue ? '' : 'text-muted-foreground'}>
                        {currentValue || 'Select assignee...'}
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
                          value={searchValue}
                          oninput={(e) => {
                            comboboxSearch[field.name] = e.currentTarget.value;
                          }}
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
                                .includes(searchValue.toLowerCase())) as assignee}
                              <button
                                type="button"
                                class="relative flex w-full cursor-pointer select-none items-center rounded-sm px-3 py-2 text-sm outline-none hover:bg-muted"
                                onclick={() => {
                                  updateProperty(field.name, assignee.value);
                                  comboboxOpen[field.name] = false;
                                  comboboxSearch[field.name] = '';
                                }}
                              >
                                {assignee.label}
                                {#if currentValue === assignee.value}
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
                {:else}
                  <!-- Regular Text Input -->
                  <Input
                    id={fieldId}
                    type="text"
                    value={(node.properties?.[field.name] as string) || ''}
                    oninput={(e) => {
                      updateProperty(field.name, e.currentTarget.value);
                    }}
                    placeholder={field.default ? String(field.default) : ''}
                  />
                {/if}
              {:else if field.type === 'number'}
                <!-- Number Field → Input Component -->
                <Input
                  id={fieldId}
                  type="number"
                  value={(node.properties?.[field.name] as number) || field.default || 0}
                  oninput={(e) => {
                    updateProperty(field.name, parseFloat(e.currentTarget.value) || 0);
                  }}
                />
              {:else if field.type === 'boolean'}
                <!-- Boolean Field → Checkbox (future implementation) -->
                <div class="text-sm text-muted-foreground">Boolean field (not yet implemented)</div>
              {:else}
                <!-- Unknown Field Type -->
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
  .property-form-loading,
  .property-form-error {
    padding: 1rem;
    text-align: center;
  }
</style>
