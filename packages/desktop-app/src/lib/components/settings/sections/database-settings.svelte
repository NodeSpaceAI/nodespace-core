<script lang="ts">
    import { appSettings, loadSettings } from '$lib/stores/settings';
    import { invoke } from '@tauri-apps/api/core';
    import { createLogger } from '$lib/utils/logger';

    const log = createLogger('DatabaseSettings');
</script>

<div class="settings-section">
    <h2>Database</h2>

    <div class="setting-group">
        <span class="setting-label">Active Database Path</span>
        <div class="setting-value path-display">
            {$appSettings?.activeDatabasePath ?? 'Loading...'}
        </div>
    </div>

    <div class="setting-actions">
        <button class="btn btn-secondary" onclick={async () => {
            try {
                const result = await invoke<{ newPath: string; requiresRestart: boolean }>('select_new_database');
                if (result.requiresRestart) {
                    const confirmed = window.confirm(
                        `Database location changed to:\n${result.newPath}\n\nNodeSpace needs to restart to use the new database. Restart now?`
                    );
                    if (confirmed) {
                        await invoke('restart_app');
                    } else {
                        await loadSettings();
                    }
                }
            } catch (err) {
                if (err !== 'No folder selected') {
                    log.error('Database selection failed:', err);
                }
            }
        }}>
            Change Location...
        </button>

        <button class="btn btn-outline" onclick={async () => {
            try {
                const defaultPath = await invoke<string>('reset_database_to_default');
                const confirmed = window.confirm(
                    `Database will be reset to default location:\n${defaultPath}\n\nNodeSpace needs to restart. Restart now?`
                );
                if (confirmed) {
                    await invoke('restart_app');
                } else {
                    await loadSettings();
                }
            } catch (err) {
                log.error('Failed to reset database:', err);
            }
        }}>
            Reset to Default
        </button>
    </div>
</div>

<style>
    .settings-section { max-width: 600px; }
    h2 { font-size: 1.25rem; font-weight: 600; color: hsl(var(--foreground)); margin: 0 0 1.5rem 0; }
    .setting-group { margin-bottom: 1.5rem; }
    .setting-label { display: block; font-size: 0.875rem; font-weight: 500; color: hsl(var(--muted-foreground)); margin-bottom: 0.5rem; }
    .setting-value { font-size: 0.875rem; color: hsl(var(--foreground)); }
    .path-display { font-family: monospace; background: hsl(var(--muted)); padding: 0.5rem 0.75rem; border-radius: var(--radius); word-break: break-all; }
    .setting-actions { display: flex; gap: 0.75rem; margin-top: 1rem; }
    .btn { padding: 0.5rem 1rem; border-radius: var(--radius); font-size: 0.875rem; cursor: pointer; border: 1px solid transparent; }
    .btn-secondary { background: hsl(var(--secondary)); color: hsl(var(--secondary-foreground)); }
    .btn-secondary:hover { opacity: 0.9; }
    .btn-outline { background: transparent; border-color: hsl(var(--border)); color: hsl(var(--foreground)); }
    .btn-outline:hover { background: hsl(var(--muted) / 0.5); }
</style>
