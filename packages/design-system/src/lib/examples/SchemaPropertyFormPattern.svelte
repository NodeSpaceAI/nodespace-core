<!--
  Schema-Driven Property Form - Correct Svelte 5 Reactivity Pattern

  This example demonstrates the proper architecture for:
  1. Dynamic schema-driven field generation
  2. Two-way binding with SharedNodeStore
  3. No infinite loops or circular dependencies
  4. Proper use of Svelte 5 runes ($derived, $state, $effect)

  KEY ARCHITECTURAL DECISIONS:

  1. SINGLE SOURCE OF TRUTH: SharedNodeStore
     - Node properties are stored in the store (Record<string, unknown>)
     - Component derives field values from store, never maintains separate state

  2. DERIVED STATE: Use $derived for field values
     - Field values are computed from node.properties on every render
     - Select components receive derived { value, label } objects
     - No synchronization needed - values are always fresh from store

  3. ONE-WAY DATA FLOW: Store → Derived → UI, UI Events → Store
     - Read path: node.properties → $derived → component props
     - Write path: UI event → updateProperty() → store.updateNode() → subscribers notified
     - NO circular dependencies: writes go through explicit functions, not reactive declarations

  4. CONTROLLED COMPONENTS: Don't use bind:selected with store data
     - Use selected={derivedValue} (one-way binding)
     - Use onSelectedChange={(e) => updateProperty(...)} (explicit callback)
     - This breaks the reactivity loop: binding doesn't trigger reactive updates

  5. DYNAMIC SCHEMAS: Handle schema changes gracefully
     - Schema loaded async from database
     - Fields generated dynamically from schema.fields
     - Each field type (enum, date, text) has dedicated rendering logic
     - $derived automatically recalculates when schema or node changes
-->

<script lang="ts">
  import * as Select from '$lib/components/ui/select';
  import * as Popover from '$lib/components/ui/popover';
  import { Calendar } from '$lib/components/ui/calendar';
  import { Input } from '$lib/components/ui/input';
  import { parseDate, type DateValue } from '@internationalized/date';

  // Mock types (replace with actual imports)
  interface SchemaField {
    name: string;
    type: 'enum' | 'date' | 'text' | 'number';
    coreValues?: string[];
    userValues?: string[];
    default?: unknown;
    description?: string;
  }

  interface SchemaDefinition {
    fields: SchemaField[];
  }

  interface Node {
    id: string;
    properties: Record<string, unknown>;
  }

  // Props
  let {
    nodeId,
    nodeType
  }: {
    nodeId: string;
    nodeType: string;
  } = $props();

  // Mock store (replace with actual SharedNodeStore)
  let mockNode = $state<Node>({
    id: nodeId,
    properties: {
      status: 'IN_PROGRESS',
      priority: 'HIGH',
      assignee: 'alice'
    }
  });

  // Mock schema (replace with schemaService)
  let mockSchema = $state<SchemaDefinition>({
    fields: [
      { name: 'status', type: 'enum', coreValues: ['OPEN', 'IN_PROGRESS', 'DONE', 'BLOCKED'] },
      { name: 'priority', type: 'enum', coreValues: ['LOW', 'MEDIUM', 'HIGH'] },
      { name: 'assignee', type: 'text' }
    ]
  });

  // ========================================================================
  // PATTERN 1: Derive field values from store (NO separate $state)
  // ========================================================================

  /**
   * Derive select values directly from node properties
   *
   * This is the KEY to avoiding infinite loops:
   * - Values are computed from store on every render
   * - No separate state to synchronize
   * - Changes to store automatically reflect in UI via $derived
   * - UI changes go through updateProperty() which updates store
   * - No circular dependency: derived doesn't trigger on writes
   */
  const selectValues = $derived.by(() => {
    if (!mockSchema || !mockNode) return {};

    const values: Record<string, { value: string; label: string }> = {};

    mockSchema.fields
      .filter(f => f.type === 'enum')
      .forEach(field => {
        const currentValue = getEnumValue(field);
        values[field.name] = {
          value: currentValue,
          label: formatEnumLabel(currentValue)
        };
      });

    return values;
  });

  /**
   * Derive date values from node properties
   * Separate from selectValues for clarity and type safety
   */
  const dateValues = $derived.by(() => {
    if (!mockSchema || !mockNode) return {};

    const values: Record<string, DateValue | undefined> = {};

    mockSchema.fields
      .filter(f => f.type === 'date')
      .forEach(field => {
        const rawValue = mockNode.properties[field.name];
        if (typeof rawValue === 'string') {
          try {
            values[field.name] = parseDate(rawValue);
          } catch {
            values[field.name] = undefined;
          }
        }
      });

    return values;
  });

  // ========================================================================
  // PATTERN 2: Update store through explicit functions (breaks loops)
  // ========================================================================

  /**
   * Update node property in store
   *
   * This is called by UI event handlers (not reactive declarations)
   * Writing through a function prevents circular dependencies
   */
  function updateProperty(fieldName: string, value: unknown) {
    // In real implementation:
    // sharedNodeStore.updateNode(
    //   nodeId,
    //   { properties: { ...mockNode.properties, [fieldName]: value } },
    //   { type: 'viewer', viewerId: 'schema-property-form' }
    // );

    // Mock implementation:
    mockNode.properties = {
      ...mockNode.properties,
      [fieldName]: value
    };

    console.log('[SchemaPropertyForm] Updated property:', {
      fieldName,
      value,
      allProperties: mockNode.properties
    });
  }

  // ========================================================================
  // Helper Functions (Pure, No Side Effects)
  // ========================================================================

  function getEnumValues(field: SchemaField): string[] {
    const values: string[] = [];
    if (field.coreValues) values.push(...field.coreValues);
    if (field.userValues) values.push(...field.userValues);
    return values;
  }

  function getEnumValue(field: SchemaField): string {
    const value = mockNode?.properties?.[field.name];
    return value ? String(value) : field.default ? String(field.default) : '';
  }

  function formatEnumLabel(value: string): string {
    return value
      .split('_')
      .map(word => word.charAt(0) + word.slice(1).toLowerCase())
      .join(' ');
  }

  function formatDateForStorage(value: DateValue | undefined): string | null {
    if (!value) return null;
    return `${value.year}-${String(value.month).padStart(2, '0')}-${String(value.day).padStart(2, '0')}`;
  }
</script>

<!-- ========================================================================
     PATTERN 3: Controlled Components (NO bind:selected)
     ======================================================================== -->

<div class="space-y-4 p-4">
  <h2 class="text-lg font-semibold">Schema-Driven Property Form (Correct Pattern)</h2>

  {#if mockSchema && mockNode}
    <div class="grid grid-cols-2 gap-4">
      {#each mockSchema.fields as field (field.name)}
        <div class="space-y-2">
          <label class="text-sm font-medium">
            {field.description || formatEnumLabel(field.name)}
          </label>

          {#if field.type === 'enum'}
            <!-- CORRECT: Controlled component pattern -->
            <!-- selected={...} provides current value (one-way) -->
            <!-- onSelectedChange={...} handles updates (explicit callback) -->
            <!-- NO bind:selected - breaks the reactivity loop -->
            {#if selectValues[field.name]}
              <Select.Root
                type="single"
                selected={selectValues[field.name]}
                onSelectedChange={(selected) => {
                  if (selected?.value) {
                    updateProperty(field.name, selected.value);
                  }
                }}
              >
                <Select.Trigger class="w-full">
                  {selectValues[field.name].label}
                </Select.Trigger>
                <Select.Content>
                  {#each getEnumValues(field) as enumValue}
                    <Select.Item value={enumValue} label={formatEnumLabel(enumValue)} />
                  {/each}
                </Select.Content>
              </Select.Root>
            {/if}

          {:else if field.type === 'date'}
            <!-- Date field with controlled Calendar component -->
            <Popover.Root>
              <Popover.Trigger
                class="flex h-10 w-full items-center justify-between rounded-md border border-input bg-background px-3 py-2 text-sm"
              >
                <span class={dateValues[field.name] ? '' : 'text-muted-foreground'}>
                  {dateValues[field.name]?.toString() || 'Pick a date'}
                </span>
              </Popover.Trigger>
              <Popover.Content class="w-auto p-0" align="start">
                <Calendar
                  value={dateValues[field.name]}
                  onValueChange={(newValue) => {
                    const singleValue = Array.isArray(newValue) ? newValue[0] : newValue;
                    updateProperty(field.name, formatDateForStorage(singleValue));
                  }}
                  type="single"
                />
              </Popover.Content>
            </Popover.Root>

          {:else if field.type === 'text'}
            <!-- Text input with controlled pattern -->
            <Input
              type="text"
              value={(mockNode.properties[field.name] as string) || ''}
              oninput={(e) => {
                updateProperty(field.name, e.currentTarget.value);
              }}
            />
          {/if}
        </div>
      {/each}
    </div>

    <!-- Debug output -->
    <div class="mt-4 rounded border p-2 text-xs">
      <strong>Current Properties:</strong>
      <pre>{JSON.stringify(mockNode.properties, null, 2)}</pre>
    </div>
  {/if}
</div>

<style>
  /* Add component-specific styles if needed */
</style>
