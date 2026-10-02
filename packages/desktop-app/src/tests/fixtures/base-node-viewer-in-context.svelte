<!-- BaseNodeViewer inside the NodeServiceContext it requires, for tests that mount a page. -->
<script lang="ts">
  import NodeServiceContext from '$lib/contexts/node-service-context.svelte';
  import BaseNodeViewer from '$lib/design/components/base-node-viewer.svelte';

  let { nodeId }: { nodeId: string } = $props();
</script>

<NodeServiceContext>
  <BaseNodeViewer {nodeId} />
</NodeServiceContext>
