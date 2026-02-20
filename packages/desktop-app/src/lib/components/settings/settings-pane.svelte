<script lang="ts">
    import { onMount } from 'svelte';
    import { loadSettings } from '$lib/stores/settings';
    import SettingsSidebar from './settings-sidebar.svelte';
    import DatabaseSettings from './sections/database-settings.svelte';
    import DisplaySettings from './sections/display-settings.svelte';
    import ImportSettings from './sections/import-settings.svelte';
    import DiagnosticsSettings from './sections/diagnostics-settings.svelte';

    let activeCategory = $state('database');

    onMount(() => {
        loadSettings();
    });
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
