<!--
  CustomEntityNode - Generic Component for User-Defined Entity Types

  This component provides a unified rendering solution for all custom entity
  types created through the schema system. Instead of creating individual
  components for each custom type (InvoiceNode, PersonNode, etc.), we use
  this single generic component that adapts based on the schema definition.

  ## Features

  - Wraps BaseNode for core editing functionality
  - Displays schema-defined properties (future: SchemaPropertyForm integration)
  - Works with any custom entity schema
  - Lazy loaded via plugin system
  - No special UI treatment needed (BaseNode handles all editing)

  ## Architecture

  ```
  CustomEntityNode (this) → BaseNode → TextareaController
        ↓
  SchemaPropertyForm (future)
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
-->

<script lang="ts">
  import BaseNode from './base-node.svelte';
  import type { NodeComponentProps } from '$lib/types/node-viewers';

  // Component props match NodeComponentProps interface
  let { nodeId, nodeType, content, children }: NodeComponentProps = $props();

  // Future enhancement: Load schema for this entity type
  // let schema = $state(null);
  //
  // $effect(async () => {
  //   try {
  //     schema = await schemaService.getSchema(nodeType);
  //   } catch (error) {
  //     console.error(`[CustomEntityNode] Failed to load schema for ${nodeType}:`, error);
  //   }
  // });
</script>

<!--
  For MVP, we simply wrap BaseNode. The custom entity behaves like a text node
  with the ability to have children. Future enhancements will add:

  - SchemaPropertyForm integration for field editing
  - Custom icons based on schema configuration
  - Special rendering for enum fields, arrays, etc.
-->
<BaseNode
  {nodeId}
  bind:nodeType
  bind:content
  {children}
/>
