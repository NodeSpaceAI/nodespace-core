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

  `get_local_identity` still backs this card's read-only summary and the
  backfill-target link. `set_local_identity` is no longer called from
  here — it stays wired for the onboarding wizard's identity step, which
  has no node viewer to navigate to yet.

  Lives in the Database settings section because the identity IS the
  database's owner (`has_role` edge to the DatabaseSettingsNode singleton) —
  the same person `seed_database_settings_if_needed` wires ownership to.
-->
<script lang="ts">
  import { getContext, onMount } from 'svelte';
  import { invoke } from '@tauri-apps/api/core';
  import { createLogger } from '$lib/utils/logger';
  import { Button } from '$lib/components/ui/button';
  import { Badge } from '$lib/components/ui/badge';
  import { Card, CardHeader, CardContent } from '$lib/components/ui/card';
  import { getNavigationService } from '$lib/services/navigation-service';
  import { DEFAULT_PANE_ID } from '$lib/stores/navigation.svelte';

  const log = createLogger('IdentityCard');

  interface LocalIdentity {
    nodeId: string;
    firstName: string;
    lastName: string;
    email: string;
    isBlank: boolean;
  }

  let identity = $state<LocalIdentity | null>(null);

  async function loadIdentity() {
    try {
      identity = await invoke<LocalIdentity | null>('get_local_identity');
    } catch (err) {
      log.warn('Could not load local identity', err);
    }
  }

  onMount(loadIdentity);

  const isBlank = $derived(identity?.isBlank ?? false);
  const fullName = $derived(
    identity ? [identity.firstName, identity.lastName].filter(Boolean).join(' ') : ''
  );

  // Settings renders inside a pane's tab (`settings-pane.svelte` under a
  // `type: 'settings'` tab in pane-content.svelte), so the same `paneId`
  // context every node component reads (e.g. task-node.svelte) is set by
  // an ancestor here too — this is the pane the "Edit identity" link was
  // clicked from, not necessarily the currently-active one.
  const sourcePaneId = getContext<string>('paneId') ?? DEFAULT_PANE_ID;

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
        <Badge variant="secondary">Loading…</Badge>
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
        <span class="font-medium">{fullName || identity.email}</span>
        {#if fullName && identity.email}
          <span class="text-muted-foreground"> · {identity.email}</span>
        {/if}
      </p>
    {:else if identity}
      <p class="text-muted-foreground mb-4 text-sm leading-relaxed">
        Name and email haven't been set yet.
      </p>
    {/if}
    <Button
      size="sm"
      variant="outline"
      disabled={!identity}
      onclick={openIdentity}
      title="Open in the person editor (Cmd+Click for a new tab in this pane)"
    >
      Edit identity
    </Button>
  </CardContent>
</Card>
