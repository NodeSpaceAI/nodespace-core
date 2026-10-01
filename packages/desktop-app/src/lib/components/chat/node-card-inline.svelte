<!--
  NodeCardInline Component

  Renders a nodespace:// reference in an AI chat message as a plain hyperlink
  whose text is the node's title. It is an anchor tag, so the global click
  handler navigates to the node.

  The title is the node's live title, never the label the agent wrote for the
  link: a rename shows everywhere, and a stale or wrong label can't pass as the
  node's name. The label shows only while the node loads or when it can't be
  found.

  Colour and underline come from the chat message's link style in
  chat-markdown.svelte, which also styles the missing state.
-->

<script lang="ts">
  import { onMount } from 'svelte';
  import { v4 as uuidv4 } from 'uuid';
  import { createLogger } from '$lib/utils/logger';
  import { sharedNodeStore } from '$lib/services/shared-node-store.svelte';
  import { backendAdapter } from '$lib/services/backend-adapter';
  import { pinReachableNodes } from '$lib/utils/pin-node-reachability';
  import { pluginRegistry } from '$lib/plugins/plugin-registry';
  import type { Node } from '$lib/types/node';

  const log = createLogger('NodeCardInline');

  let { nodeId, displayText = '' }: { nodeId: string; displayText?: string } = $props();

  // The referenced node is an arbitrary cross-reference (nodespace:// URI in
  // chat content) resolved below via a one-shot mount fetch, never through
  // structureTree. Pin it explicitly for as long as this link displays it,
  // so it isn't evicted out from under a still-open chat (which would
  // otherwise silently revert `title` to the agent's label or the truncated id,
  // indistinguishable from the node having been deleted).
  const pinOwnerId = uuidv4();
  $effect(() => pinReachableNodes(pinOwnerId, [nodeId]));

  let node = $derived(sharedNodeStore.getNode(nodeId));
  // True once the on-mount fetch has settled; marks the link as missing below when
  // the node still isn't present. Written from the fetch callback, never from an effect.
  let fetchAttempted = $state(false);

  // Fetch from the backend on mount if the node isn't already in the store. Each link is
  // mounted imperatively with a fixed nodeId, so this is a one-shot per-link load - not a
  // reactive watch (ADR-049). Once fetched, `node` updates via the store read above.
  onMount(() => {
    if (sharedNodeStore.getNode(nodeId)) return;
    // ADR-053: capture the database generation so a switch mid-fetch drops this
    // read instead of writing the previous database's node into the now-active store.
    const epoch = sharedNodeStore.currentEpoch();
    backendAdapter.getNode(nodeId).then((fetched) => {
      if (fetched && sharedNodeStore.currentEpoch() === epoch) {
        sharedNodeStore.setNode(fetched, { type: 'database', reason: 'node-card-fetch' }, true);
      }
    }).catch((e) => {
      log.warn(`Failed to fetch node ${nodeId}:`, e);
    }).finally(() => {
      fetchAttempted = true;
    });
  });

  const MAX_TITLE_LENGTH = 120;

  /**
   * The resolved node's own title: the same value tabs and search show
   * (a computed title for title-template types, a plugin's own title for
   * dates), as one line without a header's leading `#` markers.
   *
   * Not `formatTabTitle`: that cuts a title to the width of a tab, and this
   * text sits inline in a message, where it wraps.
   */
  function liveTitle(resolved: Node): string {
    const firstLine = (pluginRegistry.getNodeTitle(resolved) ?? '').split('\n')[0].trim();
    return firstLine.replace(/^#{1,6}\s+/, '').slice(0, MAX_TITLE_LENGTH) || 'Untitled';
  }

  let title = $derived(node ? liveTitle(node) : displayText || nodeId.slice(0, 8));
  let missing = $derived(!node && fetchAttempted);
</script>

<a
  href="nodespace://{nodeId}"
  class="ns-node-card-inline"
  class:ns-node-card-inline--missing={missing}
  data-node-id={nodeId}>{title}</a
>
