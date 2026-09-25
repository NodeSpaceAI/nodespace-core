<!--
  TypedFormShell — the chrome shared by every schema-driven property form.

  Owns everything that GenericSchemaForm and TaskSchemaForm used to each
  implement on their own:
  - the Collapsible shell + trigger row (X/Y-fields badge, chevron)
  - the Relationships entry point, gated on the node's type actually having a
    typed relationship (outbound declared on its schema, or inbound declared
    by another schema targeting it) — resolved once per nodeId via
    loadNodeRelationshipsView and reused by whichever form renders
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
    counts every visible schema field)
  - hasFields: whether to render the Collapsible at all (a schema with zero
    fields shows no collapsible, only the Relationships entry point)
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
  import { createLogger } from '$lib/utils/logger';
  import RelationshipViewerModal from '$lib/components/relationships/relationship-viewer-modal.svelte';
  import NestedPropertyModal from './nested-property-modal.svelte';
  import { loadNodeRelationshipsView } from '$lib/services/relationship-viewer-service';
  import WaypointsIcon from '@lucide/svelte/icons/waypoints';

  const log = createLogger('TypedFormShell');

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

  // Relationships viewer entry point. Gated on whether this
  // node's type actually has any typed relationship — otherwise it opens
  // only to say "no typed relationships". The viewer's own load resolves
  // both sides (outbound declared on this schema + inbound declared by
  // another schema targeting this type) into one group per relationship, so
  // it runs once per node and gates on whether any group exists. Default
  // hidden; fail-open on a query error so a transient failure never hides a
  // real feature.
  let showRelationships = $state(false);
  let hasRelationships = $state(false);
  let relCheckedFor = '';
  $effect(() => {
    const id = nodeId;
    if (relCheckedFor === id) return;
    relCheckedFor = id;
    hasRelationships = false;
    loadNodeRelationshipsView(id)
      .then((view) => {
        if (nodeId === id) hasRelationships = view.groups.length > 0;
      })
      .catch((err) => {
        log.error('Failed to check relationships for the trigger gate', err);
        if (nodeId === id) hasRelationships = true;
      });
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
  {#if hasFields}
    <Collapsible.Root bind:open={isOpen}>
      <Collapsible.Trigger
        class="flex w-full items-center justify-between py-3 font-medium transition-all hover:opacity-80"
      >
        <div class="flex items-center gap-3">
          {#if headerLeft}{@render headerLeft()}{/if}
        </div>

        <div class="flex items-center gap-2">
          <span class="text-sm text-muted-foreground">
            {fieldStats.filled}/{fieldStats.total} fields
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
          {@render fields(openNestedModal)}
        </div>
      </Collapsible.Content>
    </Collapsible.Root>
  {/if}

  <!-- Relationships entry point (read-only viewer). Gated on the
       type actually having typed relationships (outbound declared or inbound). -->
  {#if hasRelationships}
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

<RelationshipViewerModal bind:open={showRelationships} {nodeId} />

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
