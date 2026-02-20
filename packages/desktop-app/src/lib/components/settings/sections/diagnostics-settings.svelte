<script lang="ts">
    import { appSettings } from '$lib/stores/settings';
</script>

<div class="settings-section">
    <h2>About NodeSpace</h2>

    <div class="info-grid">
        <div class="info-item">
            <span class="info-label">Version</span>
            <span class="info-value">Development Build</span>
        </div>

        <div class="info-item">
            <span class="info-label">Database Path</span>
            <span class="info-value mono">{$appSettings?.activeDatabasePath ?? 'Unknown'}</span>
        </div>

        <div class="info-item">
            <span class="info-label">Saved Database Path</span>
            <span class="info-value mono">{$appSettings?.savedDatabasePath ?? 'Default'}</span>
        </div>
    </div>
</div>

<style>
    .settings-section { max-width: 600px; }
    h2 { font-size: 1.25rem; font-weight: 600; color: hsl(var(--foreground)); margin: 0 0 1.5rem 0; }
    .info-grid { display: flex; flex-direction: column; gap: 1rem; }
    .info-item { display: flex; flex-direction: column; gap: 0.25rem; }
    .info-label { font-size: 0.75rem; font-weight: 500; text-transform: uppercase; letter-spacing: 0.05em; color: hsl(var(--muted-foreground)); }
    .info-value { font-size: 0.875rem; color: hsl(var(--foreground)); }
    .info-value.mono { font-family: monospace; word-break: break-all; }
</style>
