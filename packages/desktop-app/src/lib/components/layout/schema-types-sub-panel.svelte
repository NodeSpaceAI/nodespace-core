<!--
  SchemaTypesSubPanel - Slide-out panel showing available schema types

  Displays when "Schema Types" is clicked in the navigation sidebar.
  Slides in from the left, adjacent to the sidebar.
  Lists built-in types first (currently only 'task'), then custom user types.
-->

<script lang="ts">
  import type { SchemaNode } from '$lib/types/schema-node';

  interface Props {
    open: boolean;
    schemas: SchemaNode[];
    onClose: () => void;
    onSchemaClick: (_schemaId: string) => void;
  }

  let { open, schemas, onClose, onSchemaClick }: Props = $props();

  // Filter and group schemas: built-in first (only 'task' for now), then custom
  let builtInSchemas = $derived(schemas.filter((s) => s.isCore && s.id === 'task'));
  let customSchemas = $derived(schemas.filter((s) => !s.isCore));
</script>

<div class="sub-panel" class:open>
  <div class="sub-panel-header">
    <span class="sub-panel-title">Schema Types</span>
    <button class="close-btn" onclick={onClose} aria-label="Close panel">
      <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
        <path d="M18 6L6 18M6 6l12 12" />
      </svg>
    </button>
  </div>

  <ul class="schema-list">
    {#if builtInSchemas.length > 0}
      <li class="section-label">Built-in</li>
    {/if}
    {#each builtInSchemas as schema (schema.id)}
      <li>
        <button class="schema-item" onclick={() => onSchemaClick(schema.id)}>
          <svg
            class="schema-icon"
            viewBox="0 0 24 24"
            fill="none"
            stroke="currentColor"
            stroke-width="2"
            stroke-linecap="round"
            stroke-linejoin="round"
          >
            <!-- 4-shape icon: triangle (top-left), square (top-right), square (bottom-left), diamond (bottom-right) -->
            <path d="M3 3 L9 3 L6 8 Z" />
            <rect x="11" y="3" width="6" height="6" />
            <rect x="3" y="13" width="6" height="6" />
            <path d="M14 13 L17 16 L14 19 L11 16 Z" />
          </svg>
          <span class="schema-name">{schema.content}</span>
        </button>
      </li>
    {/each}

    {#if customSchemas.length > 0}
      {#if builtInSchemas.length > 0}
        <li class="separator"></li>
      {/if}
      <li class="section-label">Custom</li>
    {/if}
    {#each customSchemas as schema (schema.id)}
      <li>
        <button class="schema-item" onclick={() => onSchemaClick(schema.id)}>
          <svg
            class="schema-icon"
            viewBox="0 0 24 24"
            fill="none"
            stroke="currentColor"
            stroke-width="2"
            stroke-linecap="round"
            stroke-linejoin="round"
          >
            <path d="M3 3 L9 3 L6 8 Z" />
            <rect x="11" y="3" width="6" height="6" />
            <rect x="3" y="13" width="6" height="6" />
            <path d="M14 13 L17 16 L14 19 L11 16 Z" />
          </svg>
          <span class="schema-name">{schema.content}</span>
        </button>
      </li>
    {/each}

    {#if builtInSchemas.length === 0 && customSchemas.length === 0}
      <li class="empty-state">No schema types defined</li>
    {/if}
  </ul>
</div>

<style>
  .sub-panel {
    position: absolute;
    left: var(--sidebar-width, 240px);
    top: 0;
    width: var(--sidebar-width, 240px);
    height: 100%;
    background: hsl(var(--sidebar-background));
    border-right: 1px solid hsl(var(--border));
    box-shadow: 2px 0 8px rgba(0, 0, 0, 0.1);
    transform: translateX(-100%);
    opacity: 0;
    transition:
      transform 250ms ease-out,
      opacity 250ms ease-out;
    z-index: 20;
    display: flex;
    flex-direction: column;
    pointer-events: none;
  }

  .sub-panel.open {
    transform: translateX(0);
    opacity: 1;
    pointer-events: auto;
  }

  .sub-panel-header {
    display: flex;
    justify-content: space-between;
    align-items: center;
    padding: 1rem;
    border-bottom: 1px solid hsl(var(--border));
    flex-shrink: 0;
  }

  .sub-panel-title {
    font-size: 0.875rem;
    font-weight: 600;
    color: hsl(var(--foreground));
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .close-btn {
    display: flex;
    align-items: center;
    justify-content: center;
    width: 24px;
    height: 24px;
    background: none;
    border: none;
    cursor: pointer;
    color: hsl(var(--muted-foreground));
    border-radius: 4px;
    transition:
      background-color 0.2s,
      color 0.2s;
    flex-shrink: 0;
  }

  .close-btn:hover {
    background: hsl(var(--border));
    color: hsl(var(--foreground));
  }

  .close-btn svg {
    width: 16px;
    height: 16px;
  }

  .schema-list {
    flex: 1;
    overflow-y: auto;
    padding: 0.5rem 0;
    margin: 0;
    list-style: none;
  }

  .section-label {
    padding: 0.25rem 1rem;
    font-size: 0.75rem;
    font-weight: 600;
    color: hsl(var(--muted-foreground));
    text-transform: uppercase;
    letter-spacing: 0.05em;
  }

  .separator {
    height: 1px;
    background: hsl(var(--border));
    margin: 0.25rem 1rem;
  }

  .schema-item {
    display: flex;
    align-items: center;
    gap: 0.5rem;
    width: 100%;
    padding: 0.5rem 1rem;
    background: none;
    border: none;
    cursor: pointer;
    text-align: left;
    color: hsl(var(--muted-foreground));
    font-size: 0.875rem;
    transition:
      background-color 0.2s,
      color 0.2s;
  }

  .schema-item:hover {
    background: hsl(var(--border));
    color: hsl(var(--foreground));
  }

  .schema-icon {
    width: 16px;
    height: 16px;
    flex-shrink: 0;
  }

  .schema-name {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .empty-state {
    padding: 1rem;
    text-align: center;
    color: hsl(var(--muted-foreground));
    font-size: 0.875rem;
    font-style: italic;
  }
</style>
