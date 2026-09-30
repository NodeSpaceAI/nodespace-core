<!-- Second fixture chrome component, so two siblings in one slot are told apart. -->
<script lang="ts">
  import { onMount } from 'svelte';
  import { countMount } from './flags.svelte';

  onMount(() => countMount('chrome-secondary'));
</script>

<div data-testid="test-chrome-secondary"></div>
