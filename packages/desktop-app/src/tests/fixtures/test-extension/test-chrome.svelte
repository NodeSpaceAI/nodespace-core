<!-- Fixture chrome component. Renders a marker and counts its mounts. -->
<script lang="ts">
  import { onMount } from 'svelte';
  import { countMount } from './flags.svelte';

  onMount(() => countMount('chrome'));
</script>

<div data-testid="test-chrome"></div>
