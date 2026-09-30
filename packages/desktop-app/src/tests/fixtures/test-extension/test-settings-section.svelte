<!-- Fixture settings section. Renders a heading and a button that navigates to the Database category. -->
<script lang="ts">
  import { onMount } from 'svelte';
  import { countMount } from './flags.svelte';

  let { navigate }: { navigate: (_sectionId: string) => void } = $props();

  onMount(() => countMount('settings-section'));
</script>

<div data-testid="test-settings-section">
  <h2>Test settings section</h2>
  <button onclick={() => navigate('database')}>Go to Database</button>
</div>
