<!-- Fixture viewer-tab component. Renders the host node's id and counts its mounts. -->
<script lang="ts">
  import { onMount } from 'svelte';
  import { countMount } from './flags.svelte';

  let { nodeId }: { nodeId: string } = $props();

  onMount(() => countMount('viewer-tab'));
</script>

<div data-testid="test-viewer-tab" data-node-id={nodeId}>{nodeId}</div>
