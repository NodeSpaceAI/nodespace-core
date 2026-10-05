<!--
  Fixture tree-item action: a button that opens the fixture's item modal for its
  item, the way an extension's action opens its own `app-shell-modal`
  contribution. Counts its mounts.
-->
<script lang="ts">
  import { onMount } from 'svelte';
  import type { TreeItemActionProps } from '@nodespace/extension-api';
  import { countMount, testExtensionFlags } from './flags.svelte';

  let { nodeId, nodeType }: TreeItemActionProps = $props();

  onMount(() => countMount('tree-action'));
</script>

<button
  type="button"
  data-testid="test-tree-action"
  data-node-id={nodeId}
  data-node-type={nodeType}
  aria-label="Open item {nodeId}"
  onclick={() => (testExtensionFlags.openedTreeItem = { nodeId, nodeType })}
></button>
