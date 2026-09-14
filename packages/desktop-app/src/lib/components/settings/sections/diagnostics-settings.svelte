<script lang="ts">
    import { onMount } from 'svelte';
    import { invoke } from '@tauri-apps/api/core';
    import { settingsStore } from '$lib/stores/settings.svelte';
    import { Button } from '$lib/components/ui/button';
    import { createLogger } from '$lib/utils/logger';

    const log = createLogger('DiagnosticsSettings');

    // `windows_autorun_present` is always false on macOS/Linux (and on a
    // fresh Windows install before the daemon has ever registered the HKCU
    // Run entry), so this row only renders where there's actually something
    // to clean up.
    let autorunPresent = $state(false);
    let removingAutorun = $state(false);
    let autorunError = $state<string | null>(null);

    onMount(async () => {
        try {
            autorunPresent = await invoke<boolean>('windows_autorun_present');
        } catch (err) {
            log.error('Failed to check Windows autorun status', err);
        }
    });

    async function removeAutorun() {
        removingAutorun = true;
        autorunError = null;
        try {
            await invoke('remove_windows_autorun');
            autorunPresent = false;
        } catch (err) {
            log.error('Failed to remove Windows autorun entry', err);
            autorunError = 'Failed to remove the startup entry. Try again.';
        } finally {
            removingAutorun = false;
        }
    }
</script>

<div class="max-w-[600px]">
    <h2 class="text-foreground mb-6 text-xl font-semibold">About NodeSpace</h2>

    <div class="flex flex-col gap-4">
        <div class="flex flex-col gap-1">
            <span class="text-muted-foreground text-xs font-medium uppercase tracking-widest">Version</span>
            <span class="text-foreground text-sm">Development Build</span>
        </div>

        <div class="flex flex-col gap-1">
            <span class="text-muted-foreground text-xs font-medium uppercase tracking-widest">Database Path</span>
            <span class="text-foreground break-all font-mono text-sm">{settingsStore.appSettings?.activeDatabasePath ?? 'Unknown'}</span>
        </div>

        {#if autorunPresent}
            <div class="flex flex-col gap-1">
                <span class="text-muted-foreground text-xs font-medium uppercase tracking-widest">Startup</span>
                <div class="flex items-center gap-3">
                    <span class="text-foreground text-sm">NodeSpace starts automatically when you sign in to Windows.</span>
                    <Button variant="outline" size="sm" disabled={removingAutorun} onclick={removeAutorun}>
                        {removingAutorun ? 'Removing…' : 'Remove'}
                    </Button>
                </div>
                {#if autorunError}
                    <span class="text-destructive text-xs">{autorunError}</span>
                {/if}
            </div>
        {/if}
    </div>
</div>
