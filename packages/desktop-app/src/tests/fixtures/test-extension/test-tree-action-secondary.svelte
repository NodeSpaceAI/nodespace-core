<!-- Second fixture tree-item action, so two actions on one item are told apart. -->
<script lang="ts">
  import { onMount } from 'svelte';
  import type { TreeItemActionProps } from '@nodespace/extension-api';
  import { countMount } from './flags.svelte';

  let { nodeId }: TreeItemActionProps = $props();

  onMount(() => countMount('tree-action-secondary'));
</script>

<span data-testid="test-tree-action-secondary" data-node-id={nodeId}></span>
