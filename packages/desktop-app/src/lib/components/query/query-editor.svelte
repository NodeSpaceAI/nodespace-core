<!--
  QueryEditor - JSON textarea editor for QueryDefinition objects

  Allows users to create or edit a query by writing JSON directly.
  Validates that targetType exists and filters is an array before saving.
  Includes collapsible template examples for common query patterns.
-->

<script lang="ts">
  import type { QueryFilter, SortConfig } from '$lib/types/query';
  import { createLogger } from '$lib/utils/logger';

  const log = createLogger('QueryEditor');

  /**
   * A QueryDefinition is the subset of QueryNode fields that define the query
   * itself — stored as node.properties on a query node.
   */
  export interface QueryDefinition {
    targetType: string;
    filters: QueryFilter[];
    sorting?: SortConfig[];
    limit?: number;
  }

  const DEFAULT_QUERY: QueryDefinition = {
    targetType: 'task',
    filters: [],
    limit: 50,
  };

  const TEMPLATE_EXAMPLES: Array<{ label: string; definition: QueryDefinition }> = [
    {
      label: 'All incomplete tasks',
      definition: {
        targetType: 'task',
        filters: [
          {
            type: 'property',
            operator: 'in',
            property: 'status',
            value: ['open', 'in_progress'],
          },
        ],
        limit: 50,
      },
    },
    {
      label: 'Recent text nodes with keyword',
      definition: {
        targetType: 'text',
        filters: [
          {
            type: 'content',
            operator: 'contains',
            value: 'keyword',
          },
        ],
        sorting: [{ field: 'modifiedAt', direction: 'desc' }],
        limit: 25,
      },
    },
    {
      label: 'Tasks by priority',
      definition: {
        targetType: 'task',
        filters: [
          {
            type: 'property',
            operator: 'equals',
            property: 'priority',
            value: 'high',
          },
        ],
        sorting: [{ field: 'dueDate', direction: 'asc' }],
        limit: 50,
      },
    },
  ];

  let {
    query = null,
    onSave,
    onCancel,
  }: {
    query?: QueryDefinition | null;
    onSave: (_query: QueryDefinition) => void;
    onCancel?: () => void;
  } = $props();

  // Initialized empty; $effect sets the real value on first run and on prop changes.
  let jsonText = $state('');
  let errorMessage = $state<string | null>(null);

  // Sync textarea with the query prop (runs once on mount and on every prop change)
  $effect(() => {
    jsonText = JSON.stringify(query ?? DEFAULT_QUERY, null, 2);
  });

  function validateAndSave(): void {
    errorMessage = null;

    let parsed: unknown;
    try {
      parsed = JSON.parse(jsonText);
    } catch (e) {
      const message = e instanceof Error ? e.message : String(e);
      errorMessage = `Invalid JSON: ${message}`;
      log.warn('QueryEditor: invalid JSON', { error: message });
      return;
    }

    if (typeof parsed !== 'object' || parsed === null || Array.isArray(parsed)) {
      errorMessage = 'Query must be a JSON object.';
      return;
    }

    const candidate = parsed as Record<string, unknown>;

    if (!candidate.targetType || typeof candidate.targetType !== 'string') {
      errorMessage = 'Missing required field: targetType (must be a non-empty string).';
      return;
    }

    if (!Array.isArray(candidate.filters)) {
      errorMessage = 'Missing required field: filters (must be an array).';
      return;
    }

    const definition: QueryDefinition = {
      targetType: candidate.targetType,
      filters: candidate.filters as QueryFilter[],
    };

    if (candidate.sorting !== undefined) {
      if (!Array.isArray(candidate.sorting)) {
        errorMessage = 'Optional field sorting must be an array.';
        return;
      }
      definition.sorting = candidate.sorting as SortConfig[];
    }

    if (candidate.limit !== undefined) {
      if (typeof candidate.limit !== 'number') {
        errorMessage = 'Optional field limit must be a number.';
        return;
      }
      definition.limit = candidate.limit;
    }

    log.debug('QueryEditor: saving query', { targetType: definition.targetType });
    onSave(definition);
  }

  function handleCancel(): void {
    errorMessage = null;
    onCancel?.();
  }

  function applyTemplate(template: QueryDefinition): void {
    jsonText = JSON.stringify(template, null, 2);
    errorMessage = null;
  }
</script>

<div class="query-editor">
  <div class="editor-body">
    <label class="editor-label" for="query-json">Query Definition (JSON)</label>
    <textarea
      id="query-json"
      class="json-textarea"
      bind:value={jsonText}
      rows={14}
      spellcheck={false}
    ></textarea>

    {#if errorMessage}
      <p class="error-message" role="alert">{errorMessage}</p>
    {/if}

    <div class="editor-actions">
      <button class="btn-save" onclick={validateAndSave}>Save</button>
      {#if onCancel}
        <button class="btn-cancel" onclick={handleCancel}>Cancel</button>
      {/if}
    </div>
  </div>

  <details class="templates">
    <summary class="templates-summary">Query template examples</summary>
    <div class="templates-list">
      {#each TEMPLATE_EXAMPLES as template (template.label)}
        <div class="template-item">
          <span class="template-label">{template.label}</span>
          <button
            class="btn-use-template"
            onclick={() => applyTemplate(template.definition)}
          >
            Use
          </button>
        </div>
        <pre class="template-preview">{JSON.stringify(template.definition, null, 2)}</pre>
      {/each}
    </div>
  </details>
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

  .editor-label {
    font-size: 0.8125rem;
    font-weight: 500;
    color: hsl(var(--muted-foreground));
    letter-spacing: 0.02em;
  }

  .json-textarea {
    width: 100%;
    font-family: ui-monospace, SFMono-Regular, Menlo, Monaco, Consolas, monospace;
    font-size: 0.8125rem;
    line-height: 1.6;
    padding: 0.75rem;
    background: hsl(var(--muted));
    color: hsl(var(--foreground));
    border: 1px solid hsl(var(--border));
    border-radius: 0.375rem;
    resize: vertical;
    box-sizing: border-box;
    outline: none;
    transition: border-color 0.15s ease;
  }

  .json-textarea:focus {
    border-color: hsl(var(--primary));
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
    transition: opacity 0.15s ease;
  }

  .btn-save:hover {
    opacity: 0.9;
  }

  .btn-cancel {
    padding: 0.4375rem 1rem;
    font-size: 0.875rem;
    font-weight: 500;
    background: hsl(var(--secondary));
    color: hsl(var(--secondary-foreground));
    border: 1px solid hsl(var(--border));
    border-radius: 0.375rem;
    cursor: pointer;
    transition: background-color 0.15s ease;
  }

  .btn-cancel:hover {
    background: hsl(var(--muted));
  }

  .templates {
    border: 1px solid hsl(var(--border));
    border-radius: 0.375rem;
    overflow: hidden;
  }

  .templates-summary {
    padding: 0.5rem 0.75rem;
    font-size: 0.8125rem;
    font-weight: 500;
    color: hsl(var(--muted-foreground));
    cursor: pointer;
    background: hsl(var(--muted));
    user-select: none;
    list-style: none;
  }

  .templates-summary::-webkit-details-marker {
    display: none;
  }

  .templates-summary::before {
    content: '▶';
    display: inline-block;
    margin-right: 0.375rem;
    font-size: 0.625rem;
    transition: transform 0.15s ease;
  }

  details[open] .templates-summary::before {
    transform: rotate(90deg);
  }

  .templates-list {
    padding: 0.75rem;
    display: flex;
    flex-direction: column;
    gap: 0.75rem;
    background: hsl(var(--background));
  }

  .template-item {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 0.5rem;
  }

  .template-label {
    font-size: 0.8125rem;
    font-weight: 500;
    color: hsl(var(--foreground));
  }

  .btn-use-template {
    padding: 0.25rem 0.625rem;
    font-size: 0.75rem;
    background: hsl(var(--secondary));
    color: hsl(var(--secondary-foreground));
    border: 1px solid hsl(var(--border));
    border-radius: 0.25rem;
    cursor: pointer;
    transition: background-color 0.15s ease;
    flex-shrink: 0;
  }

  .btn-use-template:hover {
    background: hsl(var(--muted));
  }

  .template-preview {
    margin: 0;
    padding: 0.5rem 0.75rem;
    font-family: ui-monospace, SFMono-Regular, Menlo, Monaco, Consolas, monospace;
    font-size: 0.75rem;
    line-height: 1.5;
    color: hsl(var(--muted-foreground));
    background: hsl(var(--muted));
    border: 1px solid hsl(var(--border));
    border-radius: 0.25rem;
    overflow-x: auto;
    white-space: pre;
  }
</style>
