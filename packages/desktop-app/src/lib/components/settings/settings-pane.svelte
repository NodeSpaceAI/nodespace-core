<script lang="ts">
    import { onMount } from 'svelte';
    import { loadSettings, settingsStore } from '$lib/stores/settings.svelte';
    import { labsFlags } from '$lib/stores/labs-flags.svelte';
    import SettingsSidebar from './settings-sidebar.svelte';
    import DatabaseSettings from './sections/database-settings.svelte';
    import AccountSettings from './sections/account-settings.svelte';
    import DisplaySettings from './sections/display-settings.svelte';
    import ImportSettings from './sections/import-settings.svelte';
    import DiagnosticsSettings from './sections/diagnostics-settings.svelte';
    import ModelManager from './model-manager.svelte';
    import IntegrationsSettings from './sections/integrations-settings.svelte';
    import MethodologySettings from './sections/methodology-settings.svelte';
    import LabsSettings from './sections/labs-settings.svelte';

    const initial = settingsStore.initialCategory;
    settingsStore.initialCategory = null;

    let activeCategory = $state(initial ?? 'database');

    onMount(() => {
        loadSettings();
    });

    // Defense in depth: settings-sidebar.svelte already hides the "Account"
    // tab while the Labs "Team synchronization" flag is off, but that only
    // stops a *click*. Fall back to Database if this view is ever reached
    // while off some other way (e.g. a future `settingsStore.initialCategory`
    // caller, or the flag being turned off elsewhere) — no "NodeSpace Pro"
    // surface may render while the flag is off, tab-click or not.
    $effect(() => {
        if (activeCategory === 'account' && !labsFlags.syncEnabled) {
            activeCategory = 'database';
        }
    });
</script>

<div class="settings-container">
    <SettingsSidebar {activeCategory} onCategoryChange={(cat) => activeCategory = cat} />
    <div class="settings-content">
        {#if activeCategory === 'database'}
            <DatabaseSettings />
        {:else if activeCategory === 'account'}
            <AccountSettings onNavigateToDatabase={() => activeCategory = 'database'} />
        {:else if activeCategory === 'display'}
            <DisplaySettings />
        {:else if activeCategory === 'import'}
            <ImportSettings />
        {:else if activeCategory === 'ai-models'}
            <ModelManager />
        {:else if activeCategory === 'integrations'}
            <IntegrationsSettings />
        {:else if activeCategory === 'methodology'}
            <MethodologySettings />
        {:else if activeCategory === 'labs'}
            <LabsSettings />
        {:else if activeCategory === 'about'}
            <DiagnosticsSettings />
        {/if}
    </div>
</div>

<style>
    .settings-container {
        display: flex;
        height: 100%;
        background: hsl(var(--background));
    }

    .settings-content {
        flex: 1;
        padding: 2rem;
        overflow-y: auto;
    }
</style>
