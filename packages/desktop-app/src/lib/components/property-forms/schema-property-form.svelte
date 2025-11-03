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
  let node = $state<Node | null | undefined>(null);
  let isOpen = $state(true);
  let isLoadingSchema = $state(false);
  let schemaError = $state<string | null>(null);

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

  // Load node data
  $effect(() => {
    if (!nodeId) {
      node = null;
      return;
    }

    node = sharedNodeStore.getNode(nodeId);
  });

  // Calculate field completion stats
  const fieldStats = $derived(() => {
    if (!schema || !node) {
      return { filled: 0, total: 0 };
    }

    // Count only user-modifiable fields (not core/system)
    const userFields = schema.fields.filter((f) => f.protection === 'user');
    const total = userFields.length;

    // Count filled fields (non-null, non-undefined, non-empty)
    const filled = userFields.filter((field) => {
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
    const status = statusField ? node.properties?.[statusField.name] : null;

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

  // Get current enum value as string
  function getEnumValue(field: SchemaField): string {
    if (!node) return '';
    const value = node.properties?.[field.name];
    return value ? String(value) : '';
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
  <div class="property-form-container">
    <Collapsible.Root bind:open={isOpen}>
      <Collapsible.Trigger
        class="flex w-full items-center justify-between py-3 font-medium transition-all hover:opacity-80 border-b border-border"
      >
        <div class="flex items-center gap-3">
          <!-- Status Badge (if available) -->
          {#if headerSummary()?.status}
            <span
              class="inline-flex items-center rounded-md border border-border bg-muted px-2.5 py-0.5 text-xs font-medium text-foreground"
            >
              {formatEnumLabel(String(headerSummary()?.status))}
            </span>
          {/if}

          <!-- Due Date (if available) -->
          {#if headerSummary()?.dueDate}
            <span class="text-sm text-muted-foreground">
              Due {formatDateDisplay(headerSummary()?.dueDate)}
            </span>
          {/if}
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
        <div class="grid grid-cols-2 gap-4 pt-4">
          {#each schema.fields.filter((f) => f.protection === 'user') as field (field.name)}
            {@const fieldId = `property-${nodeId}-${field.name}`}
            <div class="space-y-2">
              <label for={fieldId} class="text-sm font-medium">
                {field.description || formatEnumLabel(field.name)}
              </label>

              {#if field.type === 'enum'}
                <!-- Enum Field → Native Select styled with Tailwind -->
                {@const enumValues = getEnumValues(field)}
                {@const currentValue = getEnumValue(field)}
                <select
                  id={fieldId}
                  class="flex h-10 w-full items-center justify-between rounded-md border border-input bg-background px-3 py-2 text-sm ring-offset-background focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-offset-2 disabled:cursor-not-allowed disabled:opacity-50"
                  value={currentValue}
                  onchange={(e) => {
                    updateProperty(field.name, e.currentTarget.value);
                  }}
                >
                  <option value="">Select...</option>
                  {#each enumValues as enumValue}
                    <option value={enumValue}>{formatEnumLabel(enumValue)}</option>
                  {/each}
                </select>
              {:else if field.type === 'date'}
                <!-- Date Field → Calendar with Popover -->
                {@const rawDateValue = parsePropertyValue(field, node.properties?.[field.name])}
                {@const dateValue = (rawDateValue ? rawDateValue : undefined) as
                  | DateValue
                  | DateValue[]
                  | undefined}
                <Popover.Root>
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
                      }}
                      type="single"
                    />
                  </Popover.Content>
                </Popover.Root>
              {:else if field.type === 'text' || field.type === 'string'}
                <!-- Text Field → Input Component -->
                <Input
                  id={fieldId}
                  type="text"
                  value={(node.properties?.[field.name] as string) || ''}
                  oninput={(e) => {
                    updateProperty(field.name, e.currentTarget.value);
                  }}
                  placeholder={field.default ? String(field.default) : ''}
                />
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
  .property-form-container {
    margin-top: 1rem;
  }

  .property-form-loading,
  .property-form-error {
    padding: 1rem;
    text-align: center;
  }
</style>
