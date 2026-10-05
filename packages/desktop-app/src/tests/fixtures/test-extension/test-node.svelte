<!-- Fixture node component for the fixture's node type. Renders the node's id and content. -->
<script lang="ts">
  import { onMount } from 'svelte';
  import type { NodeComponentProps } from '@nodespace/extension-api';
  import { countMount } from './flags.svelte';

  let { nodeId, content = '' }: NodeComponentProps = $props();

  onMount(() => countMount('node'));
</script>

<div data-testid="test-node" data-node-id={nodeId}>{content}</div>
