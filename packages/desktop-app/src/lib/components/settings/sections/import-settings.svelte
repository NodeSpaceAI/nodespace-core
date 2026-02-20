<div class="settings-section">
    <h2>Import Sources</h2>
    <p class="description">Configure external sources for importing content into NodeSpace.</p>

    <div class="source-item">
        <span class="source-name">Local Folder Import</span>
        <span class="source-status available">Available</span>
    </div>

    <div class="source-item">
        <span class="source-name">Notion</span>
        <span class="source-status coming-soon">Coming Soon</span>
    </div>

    <div class="source-item">
        <span class="source-name">Confluence</span>
        <span class="source-status coming-soon">Coming Soon</span>
    </div>
</div>

<style>
    .settings-section { max-width: 600px; }
    h2 { font-size: 1.25rem; font-weight: 600; color: hsl(var(--foreground)); margin: 0 0 1rem 0; }
    .description { font-size: 0.875rem; color: hsl(var(--muted-foreground)); margin-bottom: 1.5rem; }
    .source-item { display: flex; justify-content: space-between; align-items: center; padding: 0.75rem 0; border-bottom: 1px solid hsl(var(--border)); }
    .source-name { font-size: 0.875rem; color: hsl(var(--foreground)); }
    .source-status { font-size: 0.75rem; padding: 0.25rem 0.5rem; border-radius: var(--radius); }
    .source-status.available { background: hsl(var(--success) / 0.1); color: hsl(var(--success)); }
    .source-status.coming-soon { background: hsl(var(--muted)); color: hsl(var(--muted-foreground)); }
</style>
