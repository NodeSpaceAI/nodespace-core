<!-- Fixture Databases-header action. Renders a button and counts its mounts. -->
<script lang="ts">
  import { onMount } from 'svelte';
  import { countMount } from './flags.svelte';

  onMount(() => countMount('database-action'));
</script>

<button data-testid="test-database-action">Fixture action</button>
