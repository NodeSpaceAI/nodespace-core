<!--
  ReplaceableSlotOutlet — hosts a replaceable slot (ADR-082 §3.2): a place where
  core renders default content that one extension contribution can replace.

  Core's default renders only while no contribution is registered for the slot.
  Once one is registered, the default never renders again, even while that
  contribution is hidden, its `when()` throws, its `load()` rejects or its
  component throws: the place stays empty instead (ADR-082 §3.4), so a failing
  extension never brings core's default back. At most one contribution renders,
  the visible one with the highest priority, then the first registered. It
  mounts through {@link ExtensionOutlet}, keyed by its contribution key, which
  isolates and logs a failed load or a throwing component.

  The prop is `name`, not `slot`, because Svelte reserves the `slot` attribute.
-->
<script lang="ts">
  import type { Snippet } from 'svelte';
  import type { ReplaceableSlot } from './ui-extensions';
  import { getReplaceableSlot } from './ui-extensions.svelte';
  import ExtensionOutlet from './extension-outlet.svelte';

  let { name, defaultContent }: { name: ReplaceableSlot; defaultContent: Snippet } = $props();

  const slot = $derived(getReplaceableSlot(name));
  // Read through their own derivations: `getReplaceableSlot` returns a new object
  // on every pass, which would make the outlet re-run `load()` and remount the
  // contribution whenever any state a `when()` reads changes. `active` is the
  // registry's own entry, so it compares equal until the chosen contribution
  // itself changes.
  const registered = $derived(slot.registered);
  const active = $derived(slot.active);
</script>

{#if !registered}
  {@render defaultContent()}
{:else if active}
  {#key active.key}
    <ExtensionOutlet load={active.load} />
  {/key}
{/if}
