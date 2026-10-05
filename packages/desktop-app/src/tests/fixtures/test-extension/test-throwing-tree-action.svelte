<!-- Fixture tree-item action that throws while rendering. Takes the action's props. -->
<script lang="ts">
  import type { TreeItemActionProps } from '@nodespace/extension-api';

  let { nodeId }: TreeItemActionProps = $props();

  throw new Error('test-throwing-tree-action: render failed');
</script>

<span data-testid="test-throwing-tree-action" data-node-id={nodeId}></span>
