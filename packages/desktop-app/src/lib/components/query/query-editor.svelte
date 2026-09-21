<!--
  QueryEditor - structured filter builder for a QueryDefinition

  Replaces the raw-JSON textarea with property/operator/value rows built from the
  target type's schema fields:
  - `targetType` is inherited from the view context and is never an input here.
  - Property is chosen from the schema's declared fields (no free-typed names).
  - Operator is narrowed to the selected field's type.
  - Value renders a typed control (enum select, number, boolean, text). `exists`
    takes no value; `in` accepts a comma-separated list.

  Emits a validated QueryDefinition on save; targetType/sorting/limit from the
  incoming query are preserved untouched.
-->

<script lang="ts">
  import type { QueryDefinition } from '$lib/types/query';
  import type { SchemaField } from '$lib/types/schema-node';
  import { labelForField } from '$lib/utils/schema-field-label';
  import {
    type FilterRow,
    type Operator,
    OPERATOR_LABELS,
    buildDefinition,
    enumOptions,
    initialValueForField,
    operatorsForType,
    partitionFilters,
  } from './query-editor-model';
  import { untrack } from 'svelte';
  import { createLogger } from '$lib/utils/logger';
  import { toError } from '$lib/types/errors';

  const log = createLogger('QueryEditor');

  let {
    query = null,
    fields = [],
    targetType,
    onSave,
    onCancel,
    onPreview,
  }: {
    query?: QueryDefinition | null;
    /** The target type's schema fields — drive the property/operator/value controls. */
    fields?: SchemaField[];
    /** Inherited from the view context; preserved, never edited here. */
    targetType: string;
    onSave: (_query: QueryDefinition) => void;
    onCancel?: () => void;
    /** Optional callback to get the live matching-node count for a definition. */
    onPreview?: (_query: QueryDefinition) => Promise<number>;
  } = $props();

  function fieldByName(name: string): SchemaField | undefined {
    return fields.find((f) => f.name === name);
  }

  function operatorsFor(field: SchemaField | undefined): Operator[] {
    return operatorsForType(field?.type);
  }

  // Capture once at init (ADR-049 — no prop→state $effect syncing). Rows are the
  // editable property filters; `preservedFilters` are content/relationship/
  // metadata filters (and property filters on fields the schema no longer
  // declares) that the builder can't represent — carried through untouched on
  // save so re-saving never drops them.
  const seeded = untrack(() => partitionFilters(query, fields));
  let rows = $state<FilterRow[]>(seeded.rows);
  const preservedFilters = seeded.preserved;
  let errorMessage = $state<string | null>(null);
  let previewCount = $state<number | null>(null);
  let previewLoading = $state(false);

  const canAdd = $derived(fields.length > 0);

  function addRow(): void {
    const first = fields[0];
    if (!first) return;
    rows = [
      ...rows,
      { property: first.name, operator: operatorsFor(first)[0], value: initialValueForField(first) },
    ];
    previewCount = null;
  }

  function removeRow(i: number): void {
    rows = rows.filter((_, idx) => idx !== i);
    previewCount = null;
  }

  /** Keep the operator valid when the property (and thus its type) changes. */
  function onPropertyChange(i: number, property: string): void {
    const field = fieldByName(property);
    const allowed = operatorsFor(field);
    const current = rows[i].operator;
    rows[i] = {
      property,
      operator: allowed.includes(current) ? current : allowed[0],
      value: initialValueForField(field),
    };
    previewCount = null;
  }

  /** Build a validated definition from the rows, preserving inherited fields. */
  function build(): QueryDefinition | null {
    const result = buildDefinition(rows, fields, {
      targetType: query?.targetType ?? targetType,
      sorting: query?.sorting,
      limit: query?.limit,
      preserved: preservedFilters,
    });
    if (!result.ok) {
      errorMessage = result.error;
      return null;
    }
    errorMessage = null;
    return result.definition;
  }

  function handleSave(): void {
    const def = build();
    if (!def) return;
    log.debug('QueryEditor: saving structured query', { targetType: def.targetType, filters: def.filters.length });
    onSave(def);
  }

  function handleCancel(): void {
    errorMessage = null;
    onCancel?.();
  }

  async function handlePreview(): Promise<void> {
    if (!onPreview) return;
    const def = build();
    if (!def) return;
    previewLoading = true;
    try {
      previewCount = await onPreview(def);
    } catch (e) {
      const message = toError(e).message;
      log.warn('QueryEditor: preview failed', { error: message });
      previewCount = null;
      errorMessage = `Preview failed: ${message}`;
    } finally {
      previewLoading = false;
    }
  }
</script>

<div class="query-editor">
  <div class="editor-body">
    <div class="filter-header">
      <span class="editor-label">Filters</span>
      <span class="target-chip" title="The type this view targets (inherited)">{targetType || query?.targetType || '—'}</span>
    </div>

    {#if rows.length === 0}
      <p class="empty-hint">No filters — this view shows every {targetType || 'node'}. Add a filter to narrow it.</p>
    {/if}

    {#each rows as row, i (i)}
      {@const field = fieldByName(row.property)}
      <div class="filter-row">
        <select
          class="row-control property"
          value={row.property}
          onchange={(e) => onPropertyChange(i, (e.currentTarget as { value: string }).value)}
          aria-label="Filter property"
        >
          {#each fields as f (f.name)}
            <option value={f.name}>{labelForField(f)}</option>
          {/each}
        </select>

        <select class="row-control operator" bind:value={row.operator} aria-label="Filter operator">
          {#each operatorsFor(field) as op (op)}
            <option value={op}>{OPERATOR_LABELS[op]}</option>
          {/each}
        </select>

        {#if row.operator === 'exists'}
          <span class="row-control value-placeholder">(no value)</span>
        {:else if field?.type === 'enum' && row.operator !== 'in'}
          <select class="row-control value" bind:value={row.value} aria-label="Filter value">
            <option value="" disabled>Choose…</option>
            {#each enumOptions(field) as v (v.value)}
              <option value={v.value}>{v.label}</option>
            {/each}
          </select>
        {:else if field?.type === 'boolean'}
          <select class="row-control value" bind:value={row.value} aria-label="Filter value">
            <option value="true">true</option>
            <option value="false">false</option>
          </select>
        {:else}
          <input
            class="row-control value"
            type={field?.type === 'number' && row.operator !== 'in' ? 'number' : 'text'}
            bind:value={row.value}
            placeholder={row.operator === 'in' ? 'a, b, c' : 'value'}
            aria-label="Filter value"
          />
        {/if}

        <button class="btn-remove" onclick={() => removeRow(i)} aria-label="Remove filter" title="Remove filter">✕</button>
      </div>
    {/each}

    <button class="btn-add" onclick={addRow} disabled={!canAdd}>+ Add filter</button>

    {#if preservedFilters.length > 0}
      <p class="preserved-hint">
        {preservedFilters.length} advanced {preservedFilters.length === 1 ? 'filter' : 'filters'}
        on this query {preservedFilters.length === 1 ? 'is' : 'are'} kept but not editable here.
      </p>
    {/if}

    {#if errorMessage}
      <p class="error-message" role="alert">{errorMessage}</p>
    {/if}

    <div class="editor-actions">
      <button class="btn-save" onclick={handleSave}>Save</button>
      {#if onPreview}
        <button class="btn-preview" onclick={handlePreview} disabled={previewLoading}>
          {#if previewLoading}
            Previewing...
          {:else if previewCount !== null}
            <!-- No "+" suffix: the count comes from a COUNT(*) that has no row
                 cap to saturate against, so the number is the exact total
                 however large it is. -->
            Preview ({previewCount} {previewCount === 1 ? 'result' : 'results'})
          {:else}
            Preview
          {/if}
        </button>
      {/if}
      {#if onCancel}
        <button class="btn-cancel" onclick={handleCancel}>Cancel</button>
      {/if}
    </div>
  </div>
</div>

<style>
  .query-editor {
    display: flex;
    flex-direction: column;
    gap: 1rem;
    padding: 1rem;
    background: hsl(var(--background));
    border: 1px solid hsl(var(--border));
    border-radius: 0.5rem;
  }

  .editor-body {
    display: flex;
    flex-direction: column;
    gap: 0.5rem;
  }

  .filter-header {
    display: flex;
    align-items: center;
    gap: 0.5rem;
    margin-bottom: 0.25rem;
  }

  .editor-label {
    font-size: 0.8125rem;
    font-weight: 500;
    color: hsl(var(--muted-foreground));
    letter-spacing: 0.02em;
  }

  .target-chip {
    font-size: 0.6875rem;
    font-weight: 500;
    padding: 0.125rem 0.5rem;
    border-radius: 999px;
    background: hsl(var(--muted));
    color: hsl(var(--muted-foreground));
    border: 1px solid hsl(var(--border));
  }

  .empty-hint {
    margin: 0 0 0.25rem;
    font-size: 0.8125rem;
    color: hsl(var(--muted-foreground));
  }

  .preserved-hint {
    margin: 0;
    font-size: 0.75rem;
    color: hsl(var(--muted-foreground));
    font-style: italic;
  }

  .filter-row {
    display: flex;
    align-items: center;
    gap: 0.5rem;
  }

  .row-control {
    font-size: 0.8125rem;
    padding: 0.375rem 0.5rem;
    background: hsl(var(--background));
    color: hsl(var(--foreground));
    border: 1px solid hsl(var(--border));
    border-radius: 0.375rem;
    outline: none;
  }

  .row-control:focus {
    border-color: hsl(var(--primary));
  }

  .row-control.property {
    flex: 0 0 34%;
  }

  .row-control.operator {
    flex: 0 0 22%;
  }

  .row-control.value {
    flex: 1 1 auto;
    min-width: 0;
  }

  .value-placeholder {
    flex: 1 1 auto;
    font-size: 0.8125rem;
    color: hsl(var(--muted-foreground));
    font-style: italic;
  }

  .btn-remove {
    flex: 0 0 auto;
    width: 1.75rem;
    height: 1.75rem;
    display: inline-flex;
    align-items: center;
    justify-content: center;
    font-size: 0.75rem;
    background: transparent;
    color: hsl(var(--muted-foreground));
    border: 1px solid hsl(var(--border));
    border-radius: 0.375rem;
    cursor: pointer;
  }

  .btn-remove:hover {
    color: hsl(var(--destructive));
    border-color: hsl(var(--destructive) / 0.5);
  }

  .btn-add {
    align-self: flex-start;
    padding: 0.375rem 0.75rem;
    font-size: 0.8125rem;
    font-weight: 500;
    background: transparent;
    color: hsl(var(--primary));
    border: 1px dashed hsl(var(--border));
    border-radius: 0.375rem;
    cursor: pointer;
  }

  .btn-add:hover:not(:disabled) {
    background: hsl(var(--muted));
  }

  .btn-add:disabled {
    opacity: 0.5;
    cursor: not-allowed;
  }

  .error-message {
    margin: 0;
    font-size: 0.8125rem;
    color: hsl(var(--destructive));
    padding: 0.5rem 0.75rem;
    background: hsl(var(--destructive) / 0.1);
    border: 1px solid hsl(var(--destructive) / 0.3);
    border-radius: 0.375rem;
  }

  .editor-actions {
    display: flex;
    gap: 0.5rem;
    margin-top: 0.25rem;
  }

  .btn-save {
    padding: 0.4375rem 1rem;
    font-size: 0.875rem;
    font-weight: 500;
    background: hsl(var(--primary));
    color: hsl(var(--primary-foreground));
    border: none;
    border-radius: 0.375rem;
    cursor: pointer;
  }

  .btn-save:hover {
    background: hsl(var(--primary-hover));
  }

  .btn-preview,
  .btn-cancel {
    padding: 0.4375rem 1rem;
    font-size: 0.875rem;
    font-weight: 500;
    background: hsl(var(--secondary));
    color: hsl(var(--secondary-foreground));
    border: 1px solid hsl(var(--border));
    border-radius: 0.375rem;
    cursor: pointer;
  }

  .btn-preview:hover:not(:disabled),
  .btn-cancel:hover {
    background: hsl(var(--muted));
  }

  .btn-preview:disabled {
    opacity: 0.6;
    cursor: not-allowed;
  }
</style>
