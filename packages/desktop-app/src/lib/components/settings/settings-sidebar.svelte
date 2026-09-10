<script lang="ts">
    import { cn } from '$lib/utils';

    interface Props {
        activeCategory: string;
        onCategoryChange: (_category: string) => void;
    }

    let { activeCategory, onCategoryChange }: Props = $props();

    const categories = [
        { id: 'database', label: 'Database' },
        { id: 'display', label: 'Display' },
        { id: 'ai-models', label: 'AI Models' },
        { id: 'import', label: 'Import Sources' },
        { id: 'integrations', label: 'Integrations' },
        { id: 'labs', label: 'Labs' },
        { id: 'about', label: 'About' },
    ];
</script>

<nav class="border-border bg-muted/30 min-w-[200px] w-[200px] border-r py-4">
    <h2 class="text-muted-foreground px-4 py-2 text-xs font-semibold uppercase tracking-widest">Settings</h2>
    {#each categories as category}
        <button
            class={cn(
                'block w-full cursor-pointer border-none bg-transparent px-4 py-2 text-left text-sm',
                activeCategory === category.id
                    ? 'text-primary bg-primary/10 font-medium'
                    : 'text-foreground hover:bg-muted/50'
            )}
            onclick={() => onCategoryChange(category.id)}
        >
            {category.label}
        </button>
    {/each}
</nav>
