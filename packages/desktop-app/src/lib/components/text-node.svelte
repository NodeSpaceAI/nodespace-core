<!--
  TextNode Component - Legacy Compatibility Wrapper
  
  DEPRECATED: This component is now a compatibility wrapper around TextNodeViewer.
  New code should use the plugin system and TextNodeViewer directly.
  
  This wrapper preserves the original TextNode API while internally using
  the new plugin architecture for consistency and the smart multiline behavior.
-->

<script lang="ts">
  import TextNodeViewer from '$lib/components/viewers/text-node-viewer.svelte';

  // Props - maintain original TextNode API for backward compatibility
  export let nodeId: string;
  export let autoFocus: boolean = false;
  export let content: string = '';
  export let inheritHeaderLevel: number = 0;
  export let children: unknown[] = [];

  // This wrapper directly forwards events from TextNodeViewer
</script>

<!-- Direct passthrough to TextNodeViewer -->
<TextNodeViewer
  {nodeId}
  {autoFocus}
  {content}
  nodeType="text"
  {inheritHeaderLevel}
  {children}
  on:createNewNode
  on:contentChanged
  on:indentNode
  on:outdentNode
  on:navigateArrow
  on:combineWithPrevious
  on:deleteNode
  on:focus
  on:blur
/>
