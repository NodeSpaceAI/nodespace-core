<!-- Fixture `collaboration.entry` component. Renders a marker and counts its mounts. -->
<script lang="ts">
  import { onMount } from 'svelte';
  import { countMount } from './flags.svelte';

  onMount(() => countMount('collaboration-entry'));
</script>

<div data-testid="test-collaboration-entry"></div>
