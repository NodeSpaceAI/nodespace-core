<!--
  Fixture `app-shell-modal` content: shows the item a tree-item action opened,
  and closes by clearing it.
-->
<script lang="ts">
  import { testExtensionFlags } from './flags.svelte';

  const item = $derived(testExtensionFlags.openedTreeItem);
</script>

{#if item}
  <div
    role="dialog"
    aria-label="Test item"
    data-testid="test-item-modal"
    data-node-id={item.nodeId}
    data-node-type={item.nodeType}
  >
    {item.nodeId}
    <button type="button" onclick={() => (testExtensionFlags.openedTreeItem = null)}>Close</button>
  </div>
{/if}
