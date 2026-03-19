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
  - "Open" button to view entity in dedicated pane (like task nodes)
  - Read-only title display when schema has title_template

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
  import { createEventDispatcher, getContext, onDestroy } from 'svelte';
  import BaseNode from './base-node.svelte';
  import { backendAdapter } from '$lib/services/backend-adapter';
  import { getNavigationService } from '$lib/services/navigation-service';
  import { DEFAULT_PANE_ID } from '$lib/stores/navigation';
  import { structureTree } from '$lib/stores/reactive-structure-tree.svelte';
  import { sharedNodeStore } from '$lib/services/shared-node-store.svelte';
  import type { NodeComponentProps } from '$lib/types/node-viewers';
  import { type SchemaNode, isSchemaNode } from '$lib/types/schema-node';
  import { createLogger } from '$lib/utils/logger';

  const log = createLogger('CustomEntityNode');

  // Get paneId from context (set by PaneContent)
  const sourcePaneId = getContext<string>('paneId') ?? DEFAULT_PANE_ID;

  // Component props match NodeComponentProps interface
  let {
    nodeId,
    nodeType,
    content: propsContent = '',
    children: propsChildren = []
  }: NodeComponentProps = $props();

  const dispatch = createEventDispatcher();

  // Use sharedNodeStore for cross-pane reactivity
  let sharedNode = $derived(sharedNodeStore.getNode(nodeId));
  let childIds = $derived(structureTree.getChildren(nodeId));
  let content = $derived(sharedNode?.content ?? propsContent);
  let children = $derived(childIds ?? propsChildren);

  // Schema state — loaded for titleTemplate and entity name display
  let schema = $state<SchemaNode | null>(null);

  // Load schema for this entity type
  $effect(() => {
    async function loadSchema() {
      if (!nodeType) return;
      try {
        const schemaNode = await backendAdapter.getSchema(nodeType);
        if (isSchemaNode(schemaNode)) {
          schema = schemaNode;
        }
      } catch (error) {
        log.error(`Failed to load schema for ${nodeType}:`, error);
      }
    }
    loadSchema();
  });

  // title_template support: when the schema has a template, content is read-only
  // and the title is derived from schema properties via compute_title()
  const hasTitleTemplate = $derived(schema?.titleTemplate != null);
  // The computed title is stored in the node's title field (computed by backend on property update)
  // We show it as a read-only placeholder when present; otherwise show raw template as hint
  const nodeTitle = $derived(sharedNode?.title ?? '');
  const displayContent = $derived.by(() => {
    if (!hasTitleTemplate) return null; // null = use normal editable content
    // Only show computed title if it contains at least one alphanumeric character
    // (guards against empty interpolations like "()" when no properties are filled yet)
    if (nodeTitle && /\w/.test(nodeTitle)) return nodeTitle;
    // Title not yet meaningful — show raw template as placeholder hint
    return schema!.titleTemplate!;
  });

  // Track typing state for open button visibility (like task-node)
  let isTyping = $state(false);
  let typingTimer: ReturnType<typeof setTimeout> | undefined;

  function handleTypingStart() {
    isTyping = true;
    if (typingTimer) clearTimeout(typingTimer);
    typingTimer = setTimeout(() => {
      isTyping = false;
    }, 1000);
  }

  function handleMouseMove() {
    if (isTyping) {
      if (typingTimer) clearTimeout(typingTimer);
      isTyping = false;
    }
  }

  onDestroy(() => {
    if (typingTimer) clearTimeout(typingTimer);
  });

  /**
   * Handle open button click to navigate to entity viewer
   * Follows the same pattern as task-node.svelte
   */
  async function handleOpenClick(event: MouseEvent) {
    event.preventDefault();
    event.stopPropagation();

    const navigationService = getNavigationService();

    if (event.metaKey || event.ctrlKey) {
      // Cmd+Click: Open in new tab in source pane
      await navigationService.navigateToNode(nodeId, true, sourcePaneId);
    } else {
      // Regular click: Open in dedicated viewer pane (other pane)
      await navigationService.navigateToNodeInOtherPane(nodeId, sourcePaneId);
    }
  }

  function forwardEvent<T>(eventName: string) {
    return (event: CustomEvent<T>) => dispatch(eventName, event.detail);
  }
</script>

<div
  class="custom-entity-node"
  class:typing={isTyping}
  data-entity-type={nodeType}
  onmousemove={handleMouseMove}
  role="group"
  aria-label="Custom entity node"
>
  <!-- Base Content Editing -->
  <!-- When schema has title_template, pass a read-only snippet for view mode;
       otherwise BaseNode uses its default editable view. -->
  <BaseNode
    {nodeId}
    bind:nodeType
    bind:content
    {children}
    readonly={hasTitleTemplate}
    customViewContent={hasTitleTemplate && displayContent != null ? titleTemplateSnippet : undefined}
    on:createNewNode={forwardEvent('createNewNode')}
    on:contentChanged={(e) => {
      handleTypingStart();
      dispatch('contentChanged', e.detail);
    }}
    on:indentNode={forwardEvent('indentNode')}
    on:outdentNode={forwardEvent('outdentNode')}
    on:navigateArrow={forwardEvent('navigateArrow')}
    on:combineWithPrevious={forwardEvent('combineWithPrevious')}
    on:deleteNode={forwardEvent('deleteNode')}
    on:focus={forwardEvent('focus')}
    on:blur={forwardEvent('blur')}
    on:nodeReferenceSelected={forwardEvent('nodeReferenceSelected')}
    on:slashCommandSelected={forwardEvent('slashCommandSelected')}
    on:nodeTypeChanged={forwardEvent('nodeTypeChanged')}
  />

  <!-- Open button (appears on hover, like task-node) -->
  <button
    class="entity-open-button"
    onclick={handleOpenClick}
    type="button"
    aria-label="Open entity in dedicated viewer pane (Cmd+Click for new tab in same pane)"
    title="Open in viewer"
  >
    open
  </button>
</div>

{#snippet titleTemplateSnippet()}
  <span
    class="title-template-display"
    class:title-template-placeholder={!nodeTitle || !/\w/.test(nodeTitle)}
  >{displayContent}</span>
{/snippet}

<style>
  .custom-entity-node {
    position: relative;
  }

  /* title_template display styles */
  .title-template-display {
    display: inline;
  }

  .title-template-placeholder {
    color: hsl(var(--muted-foreground));
    font-style: italic;
    opacity: 0.7;
  }

  /* Open button (top-right, appears on hover) - matches task-node pattern */
  .entity-open-button {
    position: absolute;
    top: 0.25rem;
    right: 0.25rem;
    background: hsl(var(--background));
    border: 1px solid hsl(var(--border));
    color: hsl(var(--foreground));
    padding: 0.25rem 0.5rem;
    border-radius: 0.25rem;
    font-size: 0.75rem;
    cursor: pointer;
    opacity: 0;
    transition: opacity 0.2s ease;
    text-transform: lowercase;
    z-index: 5;
  }

  /* Show button on hover, but hide while actively typing */
  .custom-entity-node:hover:not(.typing) .entity-open-button {
    opacity: 1;
  }

  .entity-open-button:hover {
    background: hsl(var(--muted));
  }
</style>
