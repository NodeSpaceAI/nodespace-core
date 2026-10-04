<script lang="ts">
    import { onMount } from 'svelte';
    import { loadSettings, settingsStore } from '$lib/stores/settings.svelte';
    import ExtensionOutlet from '$lib/plugins/extension-outlet.svelte';
    import SettingsSidebar from './settings-sidebar.svelte';
    import { findSettingsSection, isSettingsCategoryVisible } from './settings-categories';
    import DatabaseSettings from './sections/database-settings.svelte';
    import DisplaySettings from './sections/display-settings.svelte';
    import ImportSettings from './sections/import-settings.svelte';
    import DiagnosticsSettings from './sections/diagnostics-settings.svelte';
    import ModelManager from './model-manager.svelte';
    import IntegrationsSettings from './sections/integrations-settings.svelte';
    import LabsSettings from './sections/labs-settings.svelte';

    const initial = settingsStore.initialCategory;
    settingsStore.initialCategory = null;

    let activeCategory = $state(initial ?? 'database');

    onMount(() => {
        loadSettings();
    });

    // The sidebar only stops a *click* on a hidden category. Fall back to Database
    // whenever the active one is not listed: a Labs flag that is off, a section
    // whose `when()` turned false while open, or an id nothing registered
    // (a `settingsStore.initialCategory` caller, or `navigate()` from a section).
    // No hidden surface may render, tab-click or not.
    $effect(() => {
        if (!isSettingsCategoryVisible(activeCategory)) {
            activeCategory = 'database';
        }
    });

    const activeSection = $derived(findSettingsSection(activeCategory));

    function navigate(category: string) {
        activeCategory = category;
    }
</script>

<div class="settings-container">
    <SettingsSidebar {activeCategory} onCategoryChange={(cat) => activeCategory = cat} />
    <div class="settings-content">
        {#if activeCategory === 'database'}
            <DatabaseSettings />
        {:else if activeCategory === 'display'}
            <DisplaySettings />
        {:else if activeCategory === 'import'}
            <ImportSettings />
        {:else if activeCategory === 'ai-models'}
            <ModelManager />
        {:else if activeCategory === 'integrations'}
            <IntegrationsSettings />
        {:else if activeCategory === 'labs'}
            <LabsSettings />
        {:else if activeCategory === 'about'}
            <DiagnosticsSettings />
        {:else if activeSection}
            {#key activeSection.key}
                <ExtensionOutlet load={activeSection.load} props={{ navigate }} />
            {/key}
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
