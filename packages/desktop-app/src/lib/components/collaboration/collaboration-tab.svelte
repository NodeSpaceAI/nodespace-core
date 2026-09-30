<!--
  Collaboration tab.

  The variant switch behind the single `collaboration` tab contribution: the
  live `CollaborationView` once sync is enabled for the database (`relogin` /
  `connected`), the locked placeholder while it is not (`sign-in` / `consent`).
  `CollaborationView` keeps its own internal `proSync.tier` gate and all its
  logic; `CollaborationLocked` is unchanged.

  `{#key variant}` remounts the content on every variant change, as the former
  per-variant tab contributions did.
-->
<script lang="ts">
  import CollaborationView from '$lib/components/collaboration/collaboration-view.svelte';
  import CollaborationLocked from '$lib/components/collaboration/collaboration-locked.svelte';
  import { resolveProSyncVariant } from '$lib/plugins/pro-sync-variant.svelte';

  let { nodeId }: { nodeId: string } = $props();

  const variant = $derived(resolveProSyncVariant());
  const live = $derived(variant === 'relogin' || variant === 'connected');
</script>

{#key variant}
  {#if live}
    <CollaborationView collectionId={nodeId} />
  {:else}
    <CollaborationLocked {nodeId} />
  {/if}
{/key}
