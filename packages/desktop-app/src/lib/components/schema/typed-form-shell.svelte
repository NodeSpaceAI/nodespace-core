<!--
  TypedFormShell — the chrome shared by every schema-driven property form.

  Owns everything that GenericSchemaForm and TaskSchemaForm used to each
  implement on their own:
  - the Collapsible shell + trigger row (X/Y-fields badge, chevron)
  - the node's typed relationships (both directions), loaded once per nodeId
    via NodeRelationshipsState and reused by whichever form renders:
    single-valued ones (`isFormPromoted`) render as RelationshipFields after
    the form's own grid, and everything else lives behind the Relationships
    entry point, shown only when the modal has something to show
  - the shared NestedPropertyModal wiring for object/array fields

  A composing form supplies only its own field grid (as the `fields` snippet,
  which receives `openNestedModal` to wire up nested-field triggers), plus
  optional `headerLeft` content for the trigger row's left side, and the two
  callbacks the shell needs to drive the nested-field modal without knowing
  how the form reads and writes a field's value (each form's own concern).

  Props:
  - nodeId: node the form is editing (relationships gate + modal)
  - fieldStats: { filled, total } for the trigger's "X/Y fields" badge —
    computed by the caller, since what counts as a "field" differs (task
    counts its 6 hardcoded core fields + user extensions; the generic form
    counts every visible schema field). The shell adds its promoted
    relationship fields on top.
  - hasFields: whether the caller has any fields of its own (a schema with
    zero fields and no promoted relationships shows no collapsible, only the
    Relationships entry point)
  - autoOpen: mirrors GenericSchemaForm's existing autoOpen behavior —
    starts open and focuses the first control once, for types whose header is
    read-only (title_template) and need the properties panel front and center
  - getFieldValue / onFieldChange: read/write a field by name, for the shared
    NestedPropertyModal instance
-->
<script lang="ts">
  import type { Snippet } from 'svelte';
  import { Collapsible } from 'bits-ui';
  import type { SchemaField } from '$lib/types/schema-node';
  import RelationshipViewerModal from '$lib/components/relationships/relationship-viewer-modal.svelte';
  import RelationshipField from '$lib/components/relationships/relationship-field.svelte';
  import NestedPropertyModal from './nested-property-modal.svelte';
  import { NodeRelationshipsState } from '$lib/services/node-relationships-state.svelte';
  import WaypointsIcon from '@lucide/svelte/icons/waypoints';

  let {
    nodeId,
    fieldStats,
    hasFields = true,
    autoOpen = false,
    getFieldValue,
    onFieldChange,
    headerLeft,
    fields
  }: {
    nodeId: string;
    fieldStats: { filled: number; total: number };
    hasFields?: boolean;
    autoOpen?: boolean;
    getFieldValue: (_fieldName: string) => unknown;
    onFieldChange: (_fieldName: string, _value: unknown) => void;
    headerLeft?: Snippet;
    fields: Snippet<[(_field: SchemaField) => void]>;
  } = $props();

  // The node's typed relationships, loaded once per node and split between
  // the two surfaces: single-valued groups render as fields at the end of the
  // grid, and the Relationships modal's entry point shows only when the modal
  // has something left once those have moved out.
  let showRelationships = $state(false);
  const relationships = new NodeRelationshipsState();
  $effect(() => relationships.load(nodeId));
  const promotedGroups = $derived(relationships.partitioned.promoted);

  // Promoted relationship fields count toward the badge like any other field.
  const stats = $derived({
    filled: fieldStats.filled + promotedGroups.filter((group) => group.rows.length > 0).length,
    total: fieldStats.total + promotedGroups.length
  });

  // Nested (object/array) field editor. One modal instance is reused; the
  // clicked field determines what it edits. `getFieldValue`/`onFieldChange`
  // are the composing form's own read/write for whatever storage shape it
  // uses — the shell never touches the store directly.
  let nestedModalField = $state<SchemaField | null>(null);
  let nestedModalOpen = $state(false);
  function openNestedModal(field: SchemaField) {
    nestedModalField = field;
    nestedModalOpen = true;
  }

  // Initial value only (IIFE avoids Svelte's state_referenced_locally warning) — after
  // mount isOpen is fully user-controlled via bind:open below.
  let isOpen = $state((() => autoOpen)());
  let formEl = $state<HTMLElement | null>(null);
  let autoFocusDone = false;

  $effect(() => {
    if (autoOpen && isOpen && !autoFocusDone) {
      autoFocusDone = true;
      // Delay to allow Collapsible animation to complete before querying DOM
      setTimeout(() => {
        const first = formEl?.querySelector<HTMLElement>('input, select, textarea');
        first?.focus();
      }, 150);
    }
  });
</script>

<div class="schema-form-wrapper">
  {#if hasFields || promotedGroups.length > 0}
    <Collapsible.Root bind:open={isOpen}>
      <Collapsible.Trigger
        class="flex w-full items-center justify-between py-3 font-medium transition-all hover:opacity-80"
      >
        <div class="flex items-center gap-3">
          {#if headerLeft}{@render headerLeft()}{/if}
        </div>

        <div class="flex items-center gap-2">
          <span class="text-sm text-muted-foreground">
            {stats.filled}/{stats.total} fields
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
        <div bind:this={formEl}>
          {#if hasFields}{@render fields(openNestedModal)}{/if}
          <!-- Promoted relationships follow the form's own fields as one group:
               scalar fields and relationships share no declaration order to
               interleave by. -->
          {#if promotedGroups.length > 0}
            <div class="grid grid-cols-2 gap-4" class:mt-4={hasFields}>
              {#each promotedGroups as group (group.key)}
                {@const fieldId = `relationship-${group.key}`}
                <div class="space-y-2">
                  <label for={fieldId} class="text-sm font-medium">{group.label}</label>
                  <RelationshipField
                    {nodeId}
                    {group}
                    {fieldId}
                    onChanged={() => relationships.reload()}
                  />
                </div>
              {/each}
            </div>
          {/if}
        </div>
      </Collapsible.Content>
    </Collapsible.Root>
  {/if}

  <!-- Relationships entry point, for everything not already a field above.
       Hidden when the modal would have nothing to show. -->
  {#if relationships.showModalTrigger}
    <button
      type="button"
      class="flex w-full items-center gap-2 py-3 text-sm font-medium text-muted-foreground transition-all hover:opacity-80"
      onclick={() => (showRelationships = true)}
    >
      <WaypointsIcon class="h-4 w-4" />
      <span>Relationships</span>
    </button>
  {/if}
</div>

<!-- Reload on close: an edit in the modal can empty what it had to show, and
     the trigger's gate must see that. -->
<RelationshipViewerModal
  bind:open={
    () => showRelationships,
    (open) => {
      showRelationships = open;
      if (!open) void relationships.reload();
    }
  }
  {nodeId}
/>

{#if nestedModalField}
  {@const nestedField = nestedModalField}
  <NestedPropertyModal
    bind:open={nestedModalOpen}
    field={nestedField}
    value={getFieldValue(nestedField.name)}
    onPersist={(newValue) => onFieldChange(nestedField.name, newValue)}
  />
{/if}

<style>
  .schema-form-wrapper {
    width: calc(100% + (var(--viewer-padding-horizontal) * 2));
    margin-left: calc(-1 * var(--viewer-padding-horizontal));
    padding: 0 var(--viewer-padding-horizontal);
    border-bottom: 1px solid hsl(var(--border));
  }
</style>
