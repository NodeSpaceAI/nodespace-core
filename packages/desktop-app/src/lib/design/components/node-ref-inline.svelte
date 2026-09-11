<!--
  NodeRefInline Component

  Renders a bare `[[node-id]]` wikilink (a valid UUID or date id) that arrived
  from outside the editor — imported docs, agent-written content, hand-edited
  text — as a clickable node reference showing the target node's title.

  Emits the same `<a href="nodespace://{id}" class="ns-noderef ns-noderef-valid">`
  markup the other node references use, so the document-level click handler in
  app-shell.svelte (handleLinkClick) navigates it exactly like every other
  nodespace:// link — no dedicated click path.

  Render-only: it never mutates stored content. Only valid ids ever reach this
  component (the splitter gates on id validity), so an absent node here means the
  target genuinely does not exist; we fall back to the literal `[[id]]` text.
-->

<script lang="ts">
  import { onMount } from 'svelte';
  import { v4 as uuidv4 } from 'uuid';
  import { createLogger } from '$lib/utils/logger';
  import { sharedNodeStore } from '$lib/services/shared-node-store.svelte';
  import { pinReachableNodes } from '$lib/utils/pin-node-reachability';

  const log = createLogger('NodeRefInline');

  let { id }: { id: string } = $props();

  // The referenced node is resolved via ensureNode() below, never through
  // structureTree — a [[wikilink]] routinely points at a node with no
  // structural relationship to whatever document it's rendered inside.
  // SharedNodeStore's structureTree-walk eviction check can't see it as
  // reachable on its own, so pin it explicitly for as long as this
  // reference displays it: otherwise an eviction mid-display would leave
  // `resolved`/`node` stale (this component never re-checks after its
  // one-shot mount resolution), permanently rendering "Unknown node" for a
  // node that still exists.
  const pinOwnerId = uuidv4();
  $effect(() => pinReachableNodes(pinOwnerId, [id]));

  // Reactive read of the store (SvelteMap): the title updates once the node is
  // present, whether it was already cached or loaded by the on-mount fetch.
  let node = $derived(sharedNodeStore.getNode(id));

  // Set once the on-mount resolution has settled; drives the not-found fallback.
  // Written only from the fetch callback, never from an effect.
  let resolved = $state(false);

  // One-shot load on mount if the node isn't already cached, or is cached but
  // possibly stale from a daemon reconnect (a WatchNodes outage can silently
  // drop the update that would have kept it current — see
  // SharedNodeStore.isPossiblyStale). ensureNode is cache-first and
  // synthesizes virtual date nodes, so valid date ids always resolve; a UUID
  // with no backing row resolves to undefined (not found).
  onMount(() => {
    if (sharedNodeStore.getNode(id) && !sharedNodeStore.isPossiblyStale(id)) {
      resolved = true;
      return;
    }
    sharedNodeStore
      .ensureNode(id)
      .catch((error) => {
        log.warn(`Failed to resolve node reference ${id}:`, error);
      })
      .finally(() => {
        resolved = true;
      });
  });

  // Prefer the indexed title, then the first content line, then the raw id.
  let title = $derived(
    node?.title?.trim() || node?.content?.split('\n')[0]?.trim() || id
  );

  // Node genuinely absent after the fetch settled → render the literal token.
  let notFound = $derived(resolved && !node);
</script>

{#if notFound}
  <span class="ns-noderef-missing" title="Unknown node: {id}">[[{id}]]</span>
{:else}
  <a href="nodespace://{id}" class="ns-noderef ns-noderef-valid" data-node-id={id}>{title}</a>
{/if}

<style>
  /* Muted, non-link styling for a wikilink whose target does not exist. */
  .ns-noderef-missing {
    color: hsl(var(--muted-foreground));
    text-decoration: underline dotted;
    text-underline-offset: 2px;
    cursor: default;
  }
</style>
