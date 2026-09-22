<!--
  Your identity (ADR-037) — read-only summary of the seeded local-user
  PersonNode's name/email, with a link that opens the node so
  PersonSchemaForm can edit it. PersonSchemaForm is the person lazy-load
  form registered in `core-plugins.ts`, and it already owns editing these
  fields: optimistic writes through `sharedNodeStore.updateNode`, the
  ADR-065 duplicate-email suggestion, the ADR-077 instant title preview,
  and the Relationships entry point. This card intentionally does not
  reimplement any of that — it only displays the current values and links
  out, so there is exactly one place identity gets edited.

  `get_local_identity` still backs this card's initial read and the
  backfill-target link's node id. `set_local_identity` is no longer called
  from here — it stays wired for the onboarding wizard's identity step,
  which has no node viewer to navigate to yet.

  The displayed name/email are NOT re-derived from `get_local_identity`
  after the initial load — they read the node reactively out of
  `sharedNodeStore` (same source PersonSchemaForm itself renders from),
  via `ensureNode` below. Settings and the person node's tab can both be
  open at once (opening the link splits into a second pane rather than
  navigating away), so a snapshot from one Tauri call taken at mount would
  otherwise go stale the moment the user edits through the just-opened
  PersonSchemaForm — this card would keep showing the pre-edit value
  indefinitely. Reading the shared store instead means an edit made in the
  other pane is reflected here in the same tick, matching every other
  settings section's live-update behavior.

  Lives in the Database settings section because the identity IS the
  database's owner (`has_role` edge to the DatabaseSettingsNode singleton) —
  the same person `seed_database_settings_if_needed` wires ownership to.
-->
<script lang="ts">
  import { getContext, onMount } from 'svelte';
  import { v4 as uuidv4 } from 'uuid';
  import { invoke } from '@tauri-apps/api/core';
  import { createLogger } from '$lib/utils/logger';
  import { Button } from '$lib/components/ui/button';
  import { Badge } from '$lib/components/ui/badge';
  import { Card, CardHeader, CardContent } from '$lib/components/ui/card';
  import { getNavigationService } from '$lib/services/navigation-service';
  import { DEFAULT_PANE_ID } from '$lib/stores/navigation.svelte';
  import { sharedNodeStore } from '$lib/services/shared-node-store.svelte';
  import { pinReachableNodes } from '$lib/utils/pin-node-reachability';

  const log = createLogger('IdentityCard');

  interface LocalIdentity {
    nodeId: string;
    firstName: string;
    lastName: string;
    email: string;
    isBlank: boolean;
  }

  let identity = $state<LocalIdentity | null>(null);
  // Distinct from `identity === null`, which also covers "still loading" —
  // this flags the load having actually failed, so the header badge and
  // the link don't sit on "Loading…" forever with no way out. The pre-PR
  // Save-button editor didn't have this failure mode: its inputs stayed
  // usable (blank) regardless of whether the initial GET succeeded, since
  // saving never depended on it. Editing now happens by navigating to the
  // node, which genuinely requires knowing its id, so a failed load here
  // gets a distinct error state plus a retry instead.
  let loadFailed = $state(false);

  async function loadIdentity() {
    loadFailed = false;
    try {
      identity = await invoke<LocalIdentity | null>('get_local_identity');
      if (identity) {
        // Hydrate (and subscribe) the node into the shared store so the
        // $derived reads below pick up later edits made elsewhere — see the
        // header comment.
        void sharedNodeStore.ensureNode(identity.nodeId);
      }
    } catch (err) {
      log.warn('Could not load local identity', err);
      loadFailed = true;
    }
  }

  onMount(loadIdentity);

  // The person node isn't a structureTree descendant of the Settings tab
  // (it belongs to whatever tab/pane its own document lives in), so
  // SharedNodeStore's automatic structureTree-based reachability can't see
  // this card as a reason to keep it cached — without an explicit pin it
  // could be evicted while still displayed here, indistinguishable from
  // the node having been deleted. Pin it for as long as this card knows
  // its id.
  const pinOwnerId = uuidv4();
  $effect(() => {
    if (!identity) return;
    return pinReachableNodes(pinOwnerId, [identity.nodeId]);
  });

  // Prefer the live node from the shared store once it's hydrated (reflects
  // edits made through PersonSchemaForm anywhere in the app); fall back to
  // the one-time `get_local_identity` snapshot until then, so the card has
  // something to show on the very first paint rather than flashing empty.
  const liveNode = $derived(identity ? sharedNodeStore.getNode(identity.nodeId) : undefined);
  const personProps = $derived(
    liveNode ? ((liveNode.properties?.['person'] as Record<string, unknown> | undefined) ?? {}) : null
  );
  const firstName = $derived(
    personProps ? ((personProps['first_name'] as string | undefined) ?? '') : (identity?.firstName ?? '')
  );
  const lastName = $derived(
    personProps ? ((personProps['last_name'] as string | undefined) ?? '') : (identity?.lastName ?? '')
  );
  const email = $derived(
    personProps ? ((personProps['email'] as string | undefined) ?? '') : (identity?.email ?? '')
  );
  const isBlank = $derived(!firstName && !lastName && !email);
  const fullName = $derived([firstName, lastName].filter(Boolean).join(' '));

  // Settings renders inside a pane's tab (`settings-pane.svelte` under a
  // `type: 'settings'` tab in pane-content.svelte), so the same `paneId`
  // context every node component reads (e.g. task-node.svelte) is set by
  // an ancestor here too — this is the pane the "Edit identity" link was
  // clicked from, not necessarily the currently-active one.
  const sourcePaneId = getContext<string>('paneId') ?? DEFAULT_PANE_ID;

  const editTitle = $derived(
    identity
      ? 'Open in the person editor (Cmd+Click for a new tab in this pane)'
      : 'Loading your identity…'
  );

  /** Open the local person node — same click-vs-modifier convention as
   *  task-node.svelte's "Open" button and node-row.svelte's openEntity:
   *  a plain click opens it in the other pane (splitting one if needed),
   *  Cmd/Ctrl+click opens a new tab in this pane instead. */
  function openIdentity(event: MouseEvent) {
    if (!identity) return;
    const navigationService = getNavigationService();
    if (event.metaKey || event.ctrlKey) {
      navigationService.navigateToNode(identity.nodeId, true, sourcePaneId);
    } else {
      navigationService.navigateToNodeInOtherPane(identity.nodeId, sourcePaneId);
    }
  }
</script>

<Card class="mb-4 gap-0 rounded-lg py-0">
  <CardHeader class="p-5 pb-4">
    <div class="mb-1.5 flex items-center gap-2.5">
      <span class="text-foreground text-[0.9375rem] font-semibold">Your identity</span>
      {#if identity === null}
        {#if loadFailed}
          <Badge variant="destructive">Failed to load</Badge>
        {:else}
          <Badge variant="secondary">Loading…</Badge>
        {/if}
      {:else if isBlank}
        <Badge class="border-amber-500/25 bg-amber-500/10 text-amber-700">Not set</Badge>
      {:else}
        <Badge class="border-green-500/25 bg-green-500/10 text-green-700">Set</Badge>
      {/if}
    </div>
    <p class="text-muted-foreground m-0 text-sm leading-relaxed">
      Identifies you as the owner of this database and the default assignee for new tasks.
    </p>
  </CardHeader>
  <CardContent class="px-5 pb-5">
    {#if identity && !isBlank}
      <p class="text-foreground mb-4 text-sm leading-relaxed">
        <span class="font-medium">{fullName || email}</span>
        {#if fullName && email}
          <span class="text-muted-foreground"> · {email}</span>
        {/if}
      </p>
    {:else if identity}
      <p class="text-muted-foreground mb-4 text-sm leading-relaxed">
        Name and email haven't been set yet.
      </p>
    {:else if loadFailed}
      <p class="text-destructive mb-4 text-sm leading-relaxed">
        Could not load your identity.
      </p>
    {/if}
    {#if loadFailed}
      <Button size="sm" variant="outline" onclick={loadIdentity}>Retry</Button>
    {:else}
      <Button size="sm" variant="outline" disabled={!identity} onclick={openIdentity} title={editTitle}>
        Edit identity
      </Button>
    {/if}
  </CardContent>
</Card>
