<!-- Fixture per-database row content. Renders `row:<databaseId>` and counts its mounts. -->
<script lang="ts">
  import { onMount } from 'svelte';
  import { countMount } from './flags.svelte';

  let { databaseId }: { databaseId: string } = $props();

  onMount(() => countMount('database-row'));
</script>

<div data-testid="test-database-row" data-database-id={databaseId}>row:{databaseId}</div>
