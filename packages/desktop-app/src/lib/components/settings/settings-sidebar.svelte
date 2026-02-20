<script lang="ts">
    interface Props {
        activeCategory: string;
        onCategoryChange: (_category: string) => void;
    }

    let { activeCategory, onCategoryChange }: Props = $props();

    const categories = [
        { id: 'database', label: 'Database' },
        { id: 'display', label: 'Display' },
        { id: 'import', label: 'Import Sources' },
        { id: 'about', label: 'About' },
    ];
</script>

<nav class="settings-sidebar">
    <h2 class="sidebar-title">Settings</h2>
    {#each categories as category}
        <button
            class="sidebar-item"
            class:active={activeCategory === category.id}
            onclick={() => onCategoryChange(category.id)}
        >
            {category.label}
        </button>
    {/each}
</nav>

<style>
    .settings-sidebar {
        width: 200px;
        min-width: 200px;
        border-right: 1px solid hsl(var(--border));
        padding: 1rem 0;
        background: hsl(var(--muted) / 0.3);
    }

    .sidebar-title {
        font-size: 0.75rem;
        font-weight: 600;
        text-transform: uppercase;
        letter-spacing: 0.05em;
        color: hsl(var(--muted-foreground));
        padding: 0.5rem 1rem;
        margin: 0;
    }

    .sidebar-item {
        display: block;
        width: 100%;
        text-align: left;
        padding: 0.5rem 1rem;
        border: none;
        background: none;
        color: hsl(var(--foreground));
        font-size: 0.875rem;
        cursor: pointer;
        border-radius: 0;
    }

    .sidebar-item:hover {
        background: hsl(var(--muted) / 0.5);
    }

    .sidebar-item.active {
        background: hsl(var(--primary) / 0.1);
        color: hsl(var(--primary));
        font-weight: 500;
    }
</style>
