<!--
  ChromeSlotOutlet — renders every active contribution to one app-shell chrome
  slot, in priority order.

  Each contribution gets its own {@link ExtensionOutlet}, keyed by the
  contribution key (`<extension id>/<contribution id>`), so one contribution
  appearing, disappearing or failing never remounts or takes down its siblings.
  The accessor is called in the template, so the list re-evaluates when the
  state read by the contributions' `when()` predicates changes.

  The prop is `name`, not `slot`, because Svelte reserves the `slot` attribute.
-->
<script lang="ts">
  import type { ChromeSlot } from './ui-extensions';
  import { getActiveChromeContributions } from './ui-extensions.svelte';
  import ExtensionOutlet from './extension-outlet.svelte';

  let { name }: { name: ChromeSlot } = $props();
</script>

{#each getActiveChromeContributions(name) as c (c.key)}
  <ExtensionOutlet load={c.load} />
{/each}
