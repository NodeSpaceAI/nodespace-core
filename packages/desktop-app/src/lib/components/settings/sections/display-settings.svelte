<script lang="ts">
    import { appSettings, updateDisplaySetting } from '$lib/stores/settings';
    import { setTheme } from '$lib/design/theme';
    import type { Theme } from '$lib/design/tokens';
</script>

<div class="settings-section">
    <h2>Display</h2>

    <div class="setting-group">
        <label class="setting-label" for="theme-select">Theme</label>
        <select
            id="theme-select"
            class="setting-select"
            value={$appSettings?.display?.theme ?? 'system'}
            onchange={async (e) => {
                const value = e.currentTarget.value;
                await updateDisplaySetting('theme', value);
                setTheme(value as Theme);
            }}
        >
            <option value="system">System</option>
            <option value="light">Light</option>
            <option value="dark">Dark</option>
        </select>
    </div>

    <div class="setting-group">
        <label class="setting-toggle">
            <input
                type="checkbox"
                checked={$appSettings?.display?.renderMarkdown ?? false}
                onchange={async (e) => {
                    await updateDisplaySetting('renderMarkdown', e.currentTarget.checked);
                }}
            />
            <span>Render Markdown in node content</span>
        </label>
    </div>
</div>

<style>
    .settings-section { max-width: 600px; }
    h2 { font-size: 1.25rem; font-weight: 600; color: hsl(var(--foreground)); margin: 0 0 1.5rem 0; }
    .setting-group { margin-bottom: 1.5rem; }
    .setting-label { display: block; font-size: 0.875rem; font-weight: 500; color: hsl(var(--muted-foreground)); margin-bottom: 0.5rem; }
    .setting-select { padding: 0.5rem 0.75rem; border: 1px solid hsl(var(--border)); border-radius: var(--radius); background: hsl(var(--background)); color: hsl(var(--foreground)); font-size: 0.875rem; min-width: 200px; }
    .setting-toggle { display: flex; align-items: center; gap: 0.75rem; font-size: 0.875rem; color: hsl(var(--foreground)); cursor: pointer; }
    .setting-toggle input[type="checkbox"] { width: 1rem; height: 1rem; cursor: pointer; }
</style>
