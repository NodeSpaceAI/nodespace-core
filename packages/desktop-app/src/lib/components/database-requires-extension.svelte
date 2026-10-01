<!--
  Refusal view for a database that requires an extension this build does not
  support (ADR-083 §2).

  Shown in place of the workspace while the daemon refuses the active
  database. It renders the refusal message and the download link's label and
  URL verbatim from the REQUIRES_EXTENSION payload, which the app binary
  renders, so the app says exactly what the CLI and the tray say. Its way out
  is to switch to another registered database or to create a new one.

  Unlike the incompatible-database banner, it never offers to move, reset or
  otherwise modify the file: the database is intact, and another app opens it.
-->
<script lang="ts">
  import { Button } from '$lib/components/ui/button';
  import DatabaseNameDialog from '$lib/components/layout/database-name-dialog.svelte';
  import { databaseStore } from '$lib/stores/database.svelte';
  import type { RequiresExtensionPayload } from '$lib/types/requires-extension';
  import { openUrl } from '$lib/utils/external-links';
  import { createLogger } from '$lib/utils/logger';

  const log = createLogger('DatabaseRequiresExtension');

  interface Props {
    /** The refusal of the active database, from the REQUIRES_EXTENSION error. */
    refusal: RequiresExtensionPayload;
  }

  let { refusal }: Props = $props();

  const refusedDatabase = $derived(databaseStore.activeDatabase);
  // A database the listing marks refused too would only bring the user back here.
  const otherDatabases = $derived(
    databaseStore.databases.filter(
      (db) => db.id !== databaseStore.activeDatabaseId && db.status !== 'requires_extension'
    )
  );

  let createDialogOpen = $state(false);

  function download(): void {
    openUrl(refusal.downloadUrl).catch((err: unknown) => {
      log.error('Failed to open the download link', err);
    });
  }

  async function createDatabase(name: string): Promise<void> {
    const entry = await databaseStore.create(name);
    if (entry) {
      await databaseStore.switchTo(entry.id);
    }
  }
</script>

<section
  class="flex h-full w-full items-center justify-center overflow-auto p-8"
  aria-labelledby="database-refusal-message"
>
  <div class="flex w-full max-w-[520px] flex-col gap-6">
    <div class="flex flex-col gap-2">
      <h1 id="database-refusal-message" class="text-foreground text-xl font-semibold">
        {refusal.message}
      </h1>
      {#if refusedDatabase}
        <p class="text-muted-foreground text-sm">
          <span class="text-foreground font-medium">{refusedDatabase.name}</span>
          <span class="mt-1 block break-all font-mono text-xs">{refusedDatabase.path}</span>
        </p>
      {/if}
      <div class="mt-2">
        <Button onclick={download}>{refusal.downloadLabel}</Button>
      </div>
    </div>

    <div class="border-border flex flex-col gap-3 border-t pt-6">
      <p class="text-muted-foreground text-sm">
        Or keep working in another database on this machine.
      </p>

      {#if databaseStore.error}
        <div
          class="border-destructive/40 bg-destructive/10 text-destructive rounded-[var(--radius)] border px-3 py-2 text-sm"
          role="alert"
        >
          {databaseStore.error}
        </div>
      {/if}

      {#if otherDatabases.length > 0}
        <ul class="flex flex-col gap-2" aria-label="Other databases">
          {#each otherDatabases as db (db.id)}
            <li
              class="border-border bg-muted/40 flex items-center justify-between gap-4 rounded-[var(--radius)] border p-3"
            >
              <span class="text-foreground min-w-0 truncate font-medium">{db.name}</span>
              <Button
                variant="outline"
                size="sm"
                aria-label={`Open ${db.name}`}
                onclick={() => databaseStore.switchTo(db.id)}
              >
                Open
              </Button>
            </li>
          {/each}
        </ul>
      {/if}

      <div>
        <Button variant="outline" size="sm" onclick={() => (createDialogOpen = true)}>
          New database…
        </Button>
      </div>
    </div>
  </div>
</section>

<DatabaseNameDialog
  bind:open={createDialogOpen}
  title="New Database"
  description="Create a new local database. It opens immediately once created."
  label="Name"
  confirmLabel="Create"
  placeholder="e.g. Work"
  onConfirm={createDatabase}
/>
