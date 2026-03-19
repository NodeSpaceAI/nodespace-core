<!--
  GenericSchemaForm - Schema-driven property form for custom node types (Issue #965)

  Renders fields from a SchemaNode definition as appropriate inputs.
  Used by BaseNodeViewer when a node's nodeType is a UUID (custom schema type)
  that has no registered plugin schema form.

  Field type → input mapping:
  - string/text → text input
  - number → number input
  - boolean → checkbox
  - enum → dropdown (using coreValues + userValues)
  - date → date picker

  Values are stored/read from node.properties[field.name] (flat, not namespaced).

  Props:
  - nodeId: ID of the node to display properties for
  - schema: SchemaNode definition to render fields from
-->

<script lang="ts">
  import { Collapsible } from 'bits-ui';
  import * as Select from '$lib/components/ui/select';
  import * as Popover from '$lib/components/ui/popover';
  import { Calendar } from '$lib/components/ui/calendar';
  import { Input } from '$lib/components/ui/input';
  import { sharedNodeStore } from '$lib/services/shared-node-store.svelte';
  import type { SchemaNode, SchemaField, EnumValue } from '$lib/types/schema-node';
  import type { Node } from '$lib/types';
  import { parseDate, type DateValue } from '@internationalized/date';
  import { createLogger } from '$lib/utils/logger';

  const log = createLogger('GenericSchemaForm');

  let { nodeId, schema }: { nodeId: string; schema: SchemaNode } = $props();

  let isOpen = $state(false);
  let node = $state<Node | null>(null);

  $effect(() => {
    if (!nodeId) {
      node = null;
      return;
    }

    node = sharedNodeStore.getNode(nodeId) ?? null;

    const unsubscribe = sharedNodeStore.subscribe(nodeId, (updatedNode) => {
      node = updatedNode ?? null;
    });

    return () => {
      unsubscribe();
    };
  });

  const fieldStats = $derived(() => {
    let filled = 0;
    for (const field of schema.fields) {
      const value = getFieldValue(field.name);
      if (value !== null && value !== undefined && value !== '') filled++;
    }
    return { filled, total: schema.fields.length };
  });

  function getFieldValue(fieldName: string): unknown {
    if (!node) return undefined;
    return node.properties?.[fieldName] ?? null;
  }

  function updateField(fieldName: string, value: unknown) {
    if (!node) return;
    sharedNodeStore.updateNode(
      nodeId,
      { properties: { ...node.properties, [fieldName]: value } },
      { type: 'viewer', viewerId: 'generic-schema-form' }
    );
  }

  function getEnumValues(field: SchemaField): EnumValue[] {
    const values: EnumValue[] = [];
    if (field.coreValues) values.push(...field.coreValues);
    if (field.userValues) values.push(...field.userValues);
    return values;
  }

  function formatFieldLabel(fieldName: string): string {
    return fieldName
      .replace(/[_-]/g, ' ')
      .split(' ')
      .map((word) => word.charAt(0).toUpperCase() + word.slice(1).toLowerCase())
      .join(' ');
  }

  function parseDateFromValue(value: string | null | undefined): DateValue | undefined {
    if (!value) return undefined;
    try {
      const dateOnly = typeof value === 'string' && value.includes('T') ? value.split('T')[0] : value;
      return parseDate(dateOnly as string);
    } catch (error) {
      log.warn(`Failed to parse date value "${value}":`, error);
      return undefined;
    }
  }

  function formatDateDisplay(value: string | null | undefined): string {
    if (!value) return 'Pick a date';
    const date = parseDateFromValue(value as string);
    return date ? date.toString() : (value as string);
  }

  function formatDateForStorage(value: DateValue | undefined): string | null {
    if (!value) return null;
    return `${value.year}-${String(value.month).padStart(2, '0')}-${String(value.day).padStart(2, '0')}`;
  }
</script>

{#if node && schema.fields.length > 0}
  <div class="schema-form-wrapper">
    <Collapsible.Root bind:open={isOpen}>
      <Collapsible.Trigger
        class="flex w-full items-center justify-between py-3 font-medium transition-all hover:opacity-80"
      >
        <span class="text-sm font-medium"></span>
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
          {#each schema.fields as field (field.name)}
            {@const fieldId = `generic-${nodeId}-${field.name}`}
            <div class="space-y-2">
              <label for={fieldId} class="text-sm font-medium">
                {field.description || formatFieldLabel(field.name)}
              </label>

              {#if field.type === 'enum'}
                {@const enumValues = getEnumValues(field)}
                {@const currentValue = (getFieldValue(field.name) as string) || ''}
                <Select.Root
                  type="single"
                  value={currentValue}
                  onValueChange={(newValue) => updateField(field.name, newValue)}
                >
                  <Select.Trigger class="w-full">
                    {enumValues.find((ev) => ev.value === currentValue)?.label ||
                      currentValue ||
                      `Select ${formatFieldLabel(field.name)}...`}
                  </Select.Trigger>
                  <Select.Content>
                    {#each enumValues as ev}
                      <Select.Item value={ev.value} label={ev.label} />
                    {/each}
                  </Select.Content>
                </Select.Root>
              {:else if field.type === 'date'}
                {@const rawValue = getFieldValue(field.name) as string | null}
                {@const dateVal = parseDateFromValue(rawValue)}
                <Popover.Root>
                  <Popover.Trigger
                    id={fieldId}
                    class="flex h-10 w-full items-center justify-between rounded-md border border-input bg-background px-3 py-2 text-sm focus-visible:outline-none"
                  >
                    <span class={dateVal ? '' : 'text-muted-foreground'}>
                      {formatDateDisplay(rawValue)}
                    </span>
                    <svg class="h-4 w-4 opacity-50" viewBox="0 0 16 16" fill="none">
                      <rect x="2" y="3" width="12" height="11" rx="1" stroke="currentColor" stroke-width="1.5" />
                      <path d="M5 1v3M11 1v3M2 6h12" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" />
                    </svg>
                  </Popover.Trigger>
                  <Popover.Content class="w-auto p-0" align="start">
                    <Calendar
                      value={dateVal as never}
                      onValueChange={(newValue: DateValue | DateValue[] | undefined) => {
                        const singleValue = Array.isArray(newValue) ? newValue[0] : newValue;
                        updateField(field.name, formatDateForStorage(singleValue));
                      }}
                      type="single"
                    />
                  </Popover.Content>
                </Popover.Root>
              {:else if field.type === 'number'}
                <Input
                  id={fieldId}
                  type="number"
                  value={(getFieldValue(field.name) as number) ?? (field.default as number) ?? 0}
                  oninput={(e) => updateField(field.name, parseFloat(e.currentTarget.value) || 0)}
                />
              {:else if field.type === 'boolean'}
                <div class="flex items-center gap-2 h-10">
                  <input
                    id={fieldId}
                    type="checkbox"
                    checked={!!(getFieldValue(field.name) as boolean)}
                    onchange={(e) => updateField(field.name, e.currentTarget.checked)}
                    class="h-4 w-4 rounded border-input"
                  />
                </div>
              {:else if field.type === 'string' || field.type === 'text'}
                <Input
                  id={fieldId}
                  type="text"
                  value={(getFieldValue(field.name) as string) || ''}
                  oninput={(e) => updateField(field.name, e.currentTarget.value)}
                  placeholder={field.default ? String(field.default) : ''}
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
  .schema-form-wrapper {
    width: calc(100% + (var(--viewer-padding-horizontal) * 2));
    margin-left: calc(-1 * var(--viewer-padding-horizontal));
    padding: 0 var(--viewer-padding-horizontal);
    border-bottom: 1px solid hsl(var(--border));
  }
</style>
