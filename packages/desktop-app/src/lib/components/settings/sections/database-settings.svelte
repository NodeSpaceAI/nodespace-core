<script lang="ts">
  import { onMount } from 'svelte';
  import { open as openDialog } from '@tauri-apps/plugin-dialog';
  import * as Dialog from '$lib/components/ui/dialog';
  import { Button } from '$lib/components/ui/button';
  import DatabaseNameDialog from '$lib/components/layout/database-name-dialog.svelte';
  import AddSyncedDatabaseDialog from './add-synced-database-dialog.svelte';
  import IdentityCard from './identity-card.svelte';
  import { databaseStore, type DatabaseInfo } from '$lib/stores/database.svelte';
  import { proSync } from '$lib/stores/pro-sync.svelte';
  import { createLogger } from '$lib/utils/logger';

  const log = createLogger('DatabaseSettings');

  const databases = $derived(databaseStore.databases);
  const activeDatabaseId = $derived(databaseStore.activeDatabaseId);

  let newDialogOpen = $state(false);
  let addSyncedDialogOpen = $state(false);
  let renameDialogOpen = $state(false);
  let renameTarget = $state<DatabaseInfo | null>(null);
  let removeDialogOpen = $state(false);
  let removeTarget = $state<DatabaseInfo | null>(null);

  onMount(() => {
    databaseStore.load();
  });

  function statusLabel(status: string): string {
    switch (status) {
      case 'open':
        return 'Open';
      case 'closed':
        return 'Closed';
      case 'missing':
        return 'File missing';
      default:
        return 'Unknown';
    }
  }

  /** Friendly display name for a tenant schema, e.g. "tenant_demo" → "Demo". */
  function tenantLabel(schema: string): string {
    const bare = schema.replace(/^tenant_/, '').replace(/_/g, ' ').trim();
    return bare.length ? bare.charAt(0).toUpperCase() + bare.slice(1) : schema;
  }

  async function createDatabase(name: string) {
    const entry = await databaseStore.create(name);
    if (entry) {
      await databaseStore.switchTo(entry.id);
    }
  }

  async function openExisting() {
    try {
      const selected = await openDialog({
        directory: false,
        multiple: false,
        title: 'Open an existing NodeSpace database'
      });
      if (typeof selected !== 'string') return;
      const entry = await databaseStore.register(selected);
      if (entry) {
        await databaseStore.switchTo(entry.id);
      }
    } catch (err) {
      log.error('Failed to open existing database', err);
    }
  }

  function startRename(db: DatabaseInfo) {
    renameTarget = db;
    renameDialogOpen = true;
  }

  async function confirmRename(name: string) {
    if (renameTarget) {
      await databaseStore.rename(renameTarget.id, name);
    }
  }

  function startRemove(db: DatabaseInfo) {
    removeTarget = db;
    removeDialogOpen = true;
  }

  async function confirmRemove() {
    if (removeTarget) {
      await databaseStore.remove(removeTarget.id);
    }
    removeDialogOpen = false;
    removeTarget = null;
  }

  function setDefault(db: DatabaseInfo) {
    databaseStore.setDefault(db.id);
  }
</script>

<div class="max-w-[720px]">
  <IdentityCard />

  <div class="mb-2 flex items-center justify-between">
    <h2 class="text-foreground text-xl font-semibold">Databases</h2>
    <div class="flex gap-2">
      <Button variant="outline" size="sm" onclick={() => (newDialogOpen = true)}>New</Button>
      <Button variant="outline" size="sm" onclick={openExisting}>Open existing…</Button>
      {#if proSync.isPro}
        <Button variant="outline" size="sm" onclick={() => (addSyncedDialogOpen = true)}>
          Add synced database…
        </Button>
      {/if}
    </div>
  </div>
  <p class="text-muted-foreground mb-6 text-sm leading-relaxed">
    A <span class="text-foreground font-medium">database</span> is a local store on this
    machine — <span class="text-foreground font-medium">not</span> a tenant. Each database
    can sync to one cloud <span class="text-foreground font-medium">tenant</span> (its
    workspace) or stay local-only. Opening a database also switches which tenant syncs.
  </p>

  {#if databaseStore.error}
    <div
      class="border-destructive/40 bg-destructive/10 text-destructive mb-4 rounded-[var(--radius)] border px-3 py-2 text-sm"
      role="alert"
    >
      {databaseStore.error}
    </div>
  {/if}

  <div class="flex flex-col gap-2">
    {#each databases as db (db.id)}
      <div
        class="border-border bg-muted/40 flex items-start justify-between gap-4 rounded-[var(--radius)] border p-3"
      >
        <div class="min-w-0 flex-1">
          <div class="flex items-center gap-2">
            <span class="text-foreground truncate font-medium">{db.name}</span>
            {#if db.isDefault}
              <span
                class="text-muted-foreground bg-muted rounded px-1.5 py-0.5 text-xs"
                title="The daemon's default database"
              >
                default
              </span>
            {/if}
            {#if db.id === activeDatabaseId}
              <span
                class="text-primary bg-primary/10 rounded px-1.5 py-0.5 text-xs"
                title="Currently viewing"
              >
                active
              </span>
            {/if}
          </div>
          <div class="text-muted-foreground mt-1 break-all font-mono text-xs">{db.path}</div>
          <div class="text-muted-foreground mt-1 text-xs">{statusLabel(db.status)}</div>
          <div class="mt-1.5 text-xs">
            {#if db.boundTenantSchema}
              <span class="text-primary bg-primary/10 rounded px-1.5 py-0.5 font-medium">
                Syncs to tenant · {tenantLabel(db.boundTenantSchema)}
              </span>
              <span class="text-muted-foreground/60 ml-1 font-mono">{db.boundTenantSchema}</span>
            {:else}
              <span class="text-muted-foreground bg-muted rounded px-1.5 py-0.5">
                Local only · not synced
              </span>
            {/if}
          </div>
        </div>

        <div class="flex shrink-0 flex-col gap-1.5">
          {#if db.id !== activeDatabaseId}
            <Button variant="default" size="sm" onclick={() => databaseStore.switchTo(db.id)}>
              Open
            </Button>
          {/if}
          {#if !db.isDefault}
            <Button variant="ghost" size="sm" onclick={() => setDefault(db)}>Set as default</Button>
          {/if}
          <Button variant="ghost" size="sm" onclick={() => startRename(db)}>Rename</Button>
          <Button variant="ghost" size="sm" onclick={() => startRemove(db)}>Remove</Button>
        </div>
      </div>
    {/each}

    {#if databases.length === 0 && !databaseStore.loading}
      <div class="text-muted-foreground text-sm">No databases registered.</div>
    {/if}
  </div>
</div>

<DatabaseNameDialog
  bind:open={newDialogOpen}
  title="New Database"
  description="Create a new local database. It opens immediately once created."
  label="Name"
  confirmLabel="Create"
  placeholder="e.g. Work"
  onConfirm={createDatabase}
/>

<AddSyncedDatabaseDialog bind:open={addSyncedDialogOpen} />

<DatabaseNameDialog
  bind:open={renameDialogOpen}
  title="Rename Database"
  label="Name"
  confirmLabel="Rename"
  initialValue={renameTarget?.name ?? ''}
  onConfirm={confirmRename}
/>

<Dialog.Root bind:open={removeDialogOpen}>
  <Dialog.Content class="sm:max-w-md">
    <Dialog.Header>
      <Dialog.Title>Remove database</Dialog.Title>
      <Dialog.Description>
        Remove <strong>{removeTarget?.name}</strong> from NodeSpace? This only unregisters it — the
        database file on disk is NOT deleted, and you can re-add it later with “Open existing…”.
      </Dialog.Description>
    </Dialog.Header>
    <Dialog.Footer>
      <Button variant="ghost" onclick={() => (removeDialogOpen = false)}>Cancel</Button>
      <Button variant="destructive" onclick={confirmRemove}>Remove</Button>
    </Dialog.Footer>
  </Dialog.Content>
</Dialog.Root>
