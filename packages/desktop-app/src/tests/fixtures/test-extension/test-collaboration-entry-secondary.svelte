<!-- Second fixture `collaboration.entry` component, with its own marker and mount count. -->
<script lang="ts">
  import { onMount } from 'svelte';
  import { countMount } from './flags.svelte';

  onMount(() => countMount('collaboration-entry-secondary'));
</script>

<div data-testid="test-collaboration-entry-secondary"></div>
