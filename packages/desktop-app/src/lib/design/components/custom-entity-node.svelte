<!--
  CustomEntityNode - Generic Component for User-Defined Entity Types

  This component provides a unified rendering solution for all custom entity
  types created through the schema system. Instead of creating individual
  components for each custom type (InvoiceNode, PersonNode, etc.), we use
  this single generic component that adapts based on the schema definition.

  ## Features

  - Wraps BaseNode for core editing functionality
  - Loads and displays schema-defined properties via SchemaPropertyForm
  - Works with any custom entity schema
  - Lazy loaded via plugin system
  - Visual distinction via left border with custom entity color
  - Error handling with helpful fallbacks for missing schemas
  - Custom icon support from schema metadata

  ## Architecture

  ```
  CustomEntityNode (this) → BaseNode → TextareaController
        ↓
  SchemaPropertyForm (displays schema-driven properties)
  ```

  ## Usage

  This component is registered automatically by schema-plugin-loader.ts
  when custom entities are created. You never instantiate it directly.

  ```typescript
  // User creates "invoice" schema via AI
  await schemaService.createSchema({ id: 'invoice', fields: [...] });

  // Plugin auto-registered by schema-plugin-loader
  // Component lazy-loaded when "/invoice" is used
  ```

  @see packages/desktop-app/src/lib/plugins/schema-plugin-loader.ts - Auto-registration
  @see packages/desktop-app/src/lib/design/components/base-node.svelte - Core editing
  @see packages/desktop-app/src/lib/components/property-forms/schema-property-form.svelte - Property form
-->

<script lang="ts">
  import BaseNode from './base-node.svelte';
  import SchemaPropertyForm from '$lib/components/property-forms/schema-property-form.svelte';
  import { backendAdapter } from '$lib/services/backend-adapter';
  import type { NodeComponentProps } from '$lib/types/node-viewers';
  import { type SchemaNode, isSchemaNode } from '$lib/types/schema-node';
  import { createLogger } from '$lib/utils/logger';

  const log = createLogger('CustomEntityNode');

  // Component props match NodeComponentProps interface
  let { nodeId, nodeType, content, children }: NodeComponentProps = $props();

  // Schema state
  let schema = $state<SchemaNode | null>(null);
  let schemaError = $state<string | null>(null);
  let isLoadingSchema = $state(true);

  // Load schema for this entity type
  $effect(() => {
    async function loadSchema() {
      if (!nodeType) return; // Guard against undefined nodeType

      isLoadingSchema = true;
      schemaError = null;

      try {
        const schemaNode = await backendAdapter.getSchema(nodeType);
        if (isSchemaNode(schemaNode)) {
          schema = schemaNode;
        } else {
          schemaError = `Invalid schema for type: ${nodeType}`;
          schema = null;
        }
      } catch (error) {
        log.error(`Failed to load schema for ${nodeType}:`, error);
        schemaError = error instanceof Error ? error.message : 'Failed to load schema';
        schema = null;
      } finally {
        isLoadingSchema = false;
      }
    }

    loadSchema();
  });

  // Get schema description directly from typed field
  const schemaDescription = $derived(schema?.description ?? '');

  // Get custom icon from schema metadata (if available)
  // Note: Custom icon support would require extending SchemaNode with metadata
  // For now, we use emoji in the schema description (e.g., "💰 Invoice")
  const customIcon = $derived(extractIconFromDescription(schemaDescription));
  const entityName = $derived(schemaDescription || nodeType);
  const showEntityHeader = $derived(true); // Always show entity header for custom entities

  // Extract emoji icon from description if present (e.g., "💰 Invoice" → "💰")
  function extractIconFromDescription(description: string): string | null {
    if (!description) return null;
    // Match emoji at the start of the description
    const emojiMatch = description.match(/^([\p{Emoji}])\s/u);
    return emojiMatch ? emojiMatch[1] : null;
  }
</script>

<div class="custom-entity-node" data-entity-type={nodeType}>
  <!-- Entity Header (shows entity type name and optional icon) -->
  {#if showEntityHeader && schema && !isLoadingSchema && entityName}
    <div class="entity-header">
      {#if customIcon}
        <span class="entity-icon">{customIcon}</span>
      {/if}
      <span class="entity-type-name">{entityName}</span>
    </div>
  {/if}

  <!-- Base Content Editing -->
  <BaseNode
    {nodeId}
    bind:nodeType
    bind:content
    {children}
  />

  <!-- Schema Properties (if schema exists and loaded) -->
  {#if isLoadingSchema}
    <div class="schema-loading">
      <span class="text-sm text-muted-foreground">Loading properties...</span>
    </div>
  {:else if schemaError}
    <div class="schema-error">
      <span class="text-sm text-destructive">
        ⚠️ Schema not found for "{nodeType}".
      </span>
    </div>
  {:else if schema && nodeType}
    <SchemaPropertyForm {nodeId} {nodeType} />
  {/if}
</div>

<style>
  .custom-entity-node {
    border-left: 3px solid var(--custom-entity-accent, #6366f1);
    padding-left: 0.75rem;
  }

  .entity-header {
    display: flex;
    align-items: center;
    gap: 0.5rem;
    margin-bottom: 0.5rem;
    padding: 0.25rem 0.5rem;
    background: var(--muted);
    border-radius: 0.25rem;
    font-size: 0.875rem;
    font-weight: 500;
  }

  .entity-icon {
    font-size: 1rem;
  }

  .entity-type-name {
    color: var(--muted-foreground);
    text-transform: uppercase;
    letter-spacing: 0.05em;
    font-size: 0.75rem;
  }

  .schema-loading,
  .schema-error {
    padding: 0.5rem;
    text-align: center;
  }

  .schema-error {
    background: var(--destructive-foreground);
    border-radius: 0.25rem;
    margin-top: 0.5rem;
  }
</style>
