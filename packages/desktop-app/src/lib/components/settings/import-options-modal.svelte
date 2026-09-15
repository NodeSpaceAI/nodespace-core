<script lang="ts">
  import * as Dialog from '$lib/components/ui/dialog';
  import { Button } from '$lib/components/ui/button';
  import { Checkbox } from '$lib/components/ui/checkbox';
  import { Label } from '$lib/components/ui/label';
  import FolderOpenIcon from '@lucide/svelte/icons/folder-open';
  import LoaderIcon from '@lucide/svelte/icons/loader-circle';
  import CircleCheckIcon from '@lucide/svelte/icons/circle-check';
  import CircleAlertIcon from '@lucide/svelte/icons/circle-alert';
  import {
    importService,
    type BatchImportResult,
    type ImportProgressEvent,
  } from '$lib/services/import-service';
  import {
    defaultImportModalState,
    importOptionsFromModal,
    type ImportModalState,
  } from '$lib/services/import-options';
  import { collectionsData } from '$lib/stores/collections.svelte';
  import { createLogger } from '$lib/utils/logger';
  import { toError } from '$lib/types/errors';

  const log = createLogger('ImportOptionsModal');

  interface Props {
    open: boolean;
  }

  let { open = $bindable(false) }: Props = $props();

  type Phase = 'idle' | 'importing' | 'done' | 'error';

  let modalState = $state<ImportModalState>(defaultImportModalState());
  let phase = $state<Phase>('idle');
  let progress = $state<ImportProgressEvent | null>(null);
  let result = $state<BatchImportResult | null>(null);
  let errorMessage = $state<string | null>(null);

  let unsubscribeProgress: (() => void) | null = null;

  const canImport = $derived(modalState.folderPath.length > 0 && phase !== 'importing');
  // A finished import isn't necessarily a clean one: Phase-2 failures come back
  // as result.failed > 0 with success folded off, so don't show it as success.
  const importEmpty = $derived(!!result && result.total_files === 0);
  const importHadFailures = $derived(!!result && result.failed > 0);

  // Fresh, all-checked state every time the dialog opens.
  $effect(() => {
    if (open) {
      resetToDefaults();
    } else {
      teardownProgress();
    }
  });

  function resetToDefaults() {
    modalState = defaultImportModalState();
    phase = 'idle';
    progress = null;
    result = null;
    errorMessage = null;
  }

  function teardownProgress() {
    if (unsubscribeProgress) {
      unsubscribeProgress();
      unsubscribeProgress = null;
    }
  }

  async function chooseFolder() {
    const selected = await importService.selectFolder();
    // `null` = user cancelled the native picker; keep the current selection.
    if (selected) {
      modalState.folderPath = selected;
      // A new folder invalidates any prior run summary.
      if (phase !== 'importing') {
        phase = 'idle';
        result = null;
        errorMessage = null;
        progress = null;
      }
    }
  }

  async function runImport() {
    if (!canImport) return;

    const options = importOptionsFromModal(modalState);
    log.info('Running folder import', { folder: modalState.folderPath, options });

    phase = 'importing';
    progress = null;
    result = null;
    errorMessage = null;

    teardownProgress();
    // The listener only drives the live progress message; completion is decided
    // by the awaited result below, which the modal owns — so an import can never
    // hang waiting on a step-9 event that may not arrive.
    unsubscribeProgress = importService.onProgress((event) => {
      progress = event;
    });

    try {
      result = await importService.importDirectory(modalState.folderPath, options);
      // importDirectory resolves once the progress stream is drained, so treat
      // the returned result as completion (covers the empty-folder case too).
      teardownProgress();
      if (phase === 'importing') phase = 'done';
      // Refresh the collections sidebar so newly-routed/mirrored collections
      // (and their members) show up without a manual reload. Runs on any
      // completion (including a partial failure with result.failed > 0), not
      // only full success — files that did import still routed to collections.
      await collectionsData.loadCollections();
    } catch (error) {
      log.error('Folder import failed', error);
      errorMessage = toError(error).message;
      teardownProgress();
      phase = 'error';
    }
  }

  const optionRows = [
    {
      id: 'exclude-agent-files',
      label: 'Exclude agent/design files (CLAUDE.md, AGENTS.md, DESIGN.md)',
      help: 'Skip agent instruction and design files (matched by name, case-insensitive, in every sub-folder).',
      key: 'excludeAgentFiles',
    },
    {
      id: 'skip-hidden',
      label: 'Skip hidden files/folders',
      help: 'Ignore any path component starting with a dot, such as .git or .claude.',
      key: 'skipHidden',
    },
    {
      id: 'include-subfolders',
      label: 'Include sub-folders',
      help: 'Walk into nested folders. Turn off to import only the top-level files.',
      key: 'includeSubfolders',
    },
    {
      id: 'mirror-collections',
      label: 'Create collections mirroring sub-folders',
      help: 'Build a collection per sub-folder so the imported structure matches your directory tree.',
      key: 'mirrorCollections',
    },
  ] as const;
</script>

<Dialog.Root bind:open>
  <Dialog.Content class="sm:max-w-lg">
    <Dialog.Header>
      <Dialog.Title>Import Folder</Dialog.Title>
      <Dialog.Description>
        Choose a folder of Markdown files and how it should be imported.
      </Dialog.Description>
    </Dialog.Header>

    <div class="grid gap-2">
      <span class="text-muted-foreground text-sm font-medium">Folder</span>
      <div class="flex items-center gap-2">
        <Button
          variant="outline"
          onclick={chooseFolder}
          disabled={phase === 'importing'}
          class="shrink-0"
        >
          <FolderOpenIcon />
          Choose Folder…
        </Button>
        <span
          class="text-muted-foreground truncate text-sm"
          title={modalState.folderPath || undefined}
        >
          {modalState.folderPath || 'No folder selected'}
        </span>
      </div>
    </div>

    <div class="grid gap-3">
      {#each optionRows as option (option.id)}
        <div class="flex items-start gap-3">
          <Checkbox
            id={option.id}
            bind:checked={modalState[option.key]}
            disabled={phase === 'importing'}
            class="mt-0.5"
          />
          <div class="grid gap-1 leading-none">
            <Label for={option.id} class="cursor-pointer">{option.label}</Label>
            <p class="text-muted-foreground text-xs">{option.help}</p>
          </div>
        </div>
      {/each}
    </div>

    {#if phase === 'importing'}
      <div class="text-muted-foreground flex items-center gap-2 text-sm">
        <LoaderIcon class="size-4 animate-spin" />
        <span>{progress?.message ?? 'Preparing import…'}</span>
      </div>
    {:else if phase === 'done' && result && importEmpty}
      <div class="border-border bg-muted/40 flex items-start gap-2 rounded-md border p-3 text-sm">
        <CircleAlertIcon class="text-muted-foreground mt-0.5 size-4 shrink-0" />
        <span class="text-muted-foreground">No Markdown files found in that folder.</span>
      </div>
    {:else if phase === 'done' && result}
      <div
        class="grid gap-1 rounded-md border p-3 text-sm {importHadFailures
          ? 'border-amber-500/30 bg-amber-500/10'
          : 'border-border bg-muted/40'}"
      >
        <div class="text-foreground flex items-center gap-2 font-medium">
          {#if importHadFailures}
            <CircleAlertIcon class="size-4 text-amber-600" />
            <span>Imported with issues</span>
          {:else}
            <CircleCheckIcon class="size-4 text-green-600" />
            <span>Import complete</span>
          {/if}
        </div>
        <div class="text-muted-foreground">
          {result.successful} of {result.total_files} file{result.total_files === 1 ? '' : 's'} imported{result.failed
            ? `, ${result.failed} failed`
            : ''}.
        </div>
      </div>
    {:else if phase === 'error'}
      <div
        class="border-destructive/30 bg-destructive/10 text-destructive flex items-start gap-2 rounded-md border p-3 text-sm"
      >
        <CircleAlertIcon class="mt-0.5 size-4 shrink-0" />
        <span>{errorMessage ?? 'Import failed.'}</span>
      </div>
    {/if}

    <Dialog.Footer>
      <Button variant="ghost" onclick={() => (open = false)} disabled={phase === 'importing'}>
        {phase === 'done' ? 'Close' : 'Cancel'}
      </Button>
      <Button variant="default" disabled={!canImport} onclick={runImport}>
        {#if phase === 'importing'}
          <LoaderIcon class="size-4 animate-spin" />
          Importing…
        {:else if phase === 'done'}
          Import Again
        {:else}
          Import
        {/if}
      </Button>
    </Dialog.Footer>
  </Dialog.Content>
</Dialog.Root>
