<!--
  GenericSchemaForm - Schema-driven property form for custom node types

  Renders fields from a SchemaNode definition as appropriate inputs.
  Used by BaseNodeViewer for any node type with no registered plugin schema form —
  user-defined schema types and core types without a hardcoded form (e.g. project).

  Field type → control:
  - leaf fields (string/text, number, boolean, enum, date) → SchemaFieldLeaf
  - object/array → summary trigger opening the shared NestedPropertyModal

  `protection: 'system'` fields (e.g. ai-chat's `capture:transcript`, never
  actually reached today since ai-chat has its own dedicated viewer) are
  filtered out of every list below: system-managed fields must never render
  as a user-editable control.

  Values are read from node.properties[nodeType][field.name] when the type namespaces its
  properties (core types with backend behavior), falling back to flat
  node.properties[field.name] (user-defined schema types). Writes use the same precedence —
  see schema-field-resolution.ts, where both shapes are resolved and unit-tested.

  Shell chrome (Collapsible, trigger row, Relationships gate, NestedPropertyModal) is owned
  by TypedFormShell — this component supplies only the field grid.

  Props:
  - nodeId: ID of the node to display properties for
  - schema: SchemaNode definition to render fields from
-->

<script lang="ts">
  import { sharedNodeStore } from '$lib/services/shared-node-store.svelte';
  import { resolveFieldValue, buildFieldWrite } from '$lib/components/schema/schema-field-resolution';
  import type { SchemaNode, SchemaField } from '$lib/types/schema-node';
  import type { Node } from '$lib/types';
  import { labelForField } from '$lib/utils/schema-field-label';
  import { isUserVisibleField } from '$lib/utils/schema-field-visibility';
  import TypedFormShell from './typed-form-shell.svelte';
  import SchemaFieldLeaf from './schema-field-leaf.svelte';
  import NestedFieldTrigger from './nested-field-trigger.svelte';
  import { isNestedField } from '$lib/utils/nested-property-ops';

  let { nodeId, schema, autoOpen = false }: { nodeId: string; schema: SchemaNode; autoOpen?: boolean } = $props();

  // System-managed fields (e.g. ai-chat's `capture:transcript`) are never
  // user-editable — filtered out of every list below (rendering, field-count
  // stats) rather than just the one that happened to be reachable when this
  // was written. Not currently reachable in production (the only core types
  // with system fields, ai-chat and collection, both have dedicated viewers
  // that bypass this component entirely), but a real gap if anything ever
  // renders a system-field type through this generic per-field loop.
  const visibleFields = $derived(schema.fields.filter(isUserVisibleField));

  const node = $derived<Node | null>(nodeId ? (sharedNodeStore.getNode(nodeId) ?? null) : null);

  const fieldStats = $derived.by(() => {
    let filled = 0;
    for (const field of visibleFields) {
      const value = getFieldValue(field.name);
      if (value !== null && value !== undefined && value !== '') filled++;
    }
    return { filled, total: visibleFields.length };
  });

  function getFieldValue(fieldName: string): unknown {
    if (!node) return undefined;
    return resolveFieldValue(node, fieldName);
  }

  function updateField(fieldName: string, value: unknown) {
    if (!node) return;
    // Write in whichever shape the node already stores, matching getFieldValue's precedence
    // — a flat write into a namespaced node is silently discarded by the backend.
    sharedNodeStore.updateNode(
      nodeId,
      { properties: buildFieldWrite(node, fieldName, value) },
      { type: 'viewer', viewerId: 'generic-schema-form' }
    );
  }
</script>

{#if node}
  <TypedFormShell
    {nodeId}
    {fieldStats}
    hasFields={visibleFields.length > 0}
    {autoOpen}
    {getFieldValue}
    onFieldChange={updateField}
  >
    {#snippet fields(openNestedModal: (_field: SchemaField) => void)}
      <div class="grid grid-cols-2 gap-4">
        {#each visibleFields as field (field.name)}
          {@const fieldId = `generic-${nodeId}-${field.name}`}
          <div class="space-y-2">
            <label for={fieldId} class="text-sm font-medium">
              {labelForField(field)}
            </label>

            {#if isNestedField(field)}
              <NestedFieldTrigger
                {field}
                {fieldId}
                value={getFieldValue(field.name)}
                onopen={() => openNestedModal(field)}
              />
            {:else}
              <SchemaFieldLeaf
                {field}
                {fieldId}
                value={getFieldValue(field.name)}
                onChange={(newValue) => updateField(field.name, newValue)}
              />
            {/if}
          </div>
        {/each}
      </div>
    {/snippet}
  </TypedFormShell>
{/if}
