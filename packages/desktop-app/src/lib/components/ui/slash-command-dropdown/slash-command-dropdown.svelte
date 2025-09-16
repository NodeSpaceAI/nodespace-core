<script lang="ts">
  import { cn } from '$lib/utils';
  import Icon, { type NodeType } from '$lib/design/icons';
  import { createEventDispatcher, onMount } from 'svelte';
  import type { SlashCommand } from '$lib/services/slashCommandService';

  // Props
  export let position: { x: number; y: number } = { x: 0, y: 0 };
  export let query: string = '';
  export let commands: SlashCommand[] = [];
  export let loading: boolean = false;
  export let visible: boolean = false;

  // State
  let selectedIndex: number = 0;
  let containerRef: HTMLElement | undefined = undefined;
  let itemRefs: HTMLElement[] = [];

  const dispatch = createEventDispatcher<{
    select: SlashCommand;
    close: void;
  }>();

  // Smart positioning to avoid viewport edges
  function getSmartPosition(pos: { x: number; y: number }) {
    const padding = 16;
    const maxWidth = 360;
    const maxHeight = 300;

    let { x, y } = pos;

    // Adjust horizontal position
    if (x + maxWidth > window.innerWidth - padding) {
      x = window.innerWidth - maxWidth - padding;
    }
    if (x < padding) {
      x = padding;
    }

    // Adjust vertical position (show above cursor if not enough space below)
    if (y + maxHeight > window.innerHeight - padding) {
      y = y - maxHeight - 20; // Show above cursor
    } else {
      y = y + 20; // Show below cursor
    }

    return { x, y };
  }

  // Smart position calculation
  $: smartPosition = getSmartPosition(position);

  function scrollToSelected() {
    const selectedItem = itemRefs[selectedIndex];
    if (selectedItem && containerRef) {
      selectedItem.scrollIntoView({
        block: 'nearest',
        behavior: 'smooth'
      });
    }
  }

  // Keyboard navigation
  function handleKeydown(event: KeyboardEvent) {
    if (!visible || commands.length === 0) return;

    switch (event.key) {
      case 'ArrowDown':
        event.preventDefault();
        selectedIndex = Math.min(selectedIndex + 1, commands.length - 1);
        scrollToSelected();
        break;
      case 'ArrowUp':
        event.preventDefault();
        selectedIndex = Math.max(selectedIndex - 1, 0);
        scrollToSelected();
        break;
      case 'Enter':
        event.preventDefault();
        if (commands[selectedIndex]) {
          selectCommand(commands[selectedIndex]);
        }
        break;
      case 'Escape':
        event.preventDefault();
        dispatch('close');
        break;
    }
  }

  function selectCommand(command: SlashCommand) {
    selectedIndex = commands.indexOf(command);
    dispatch('select', command);
  }

  // Reset selection when commands change
  $: if (commands) {
    selectedIndex = 0;
  }

  // Bind keyboard events
  onMount(() => {
    document.addEventListener('keydown', handleKeydown);
    return () => {
      document.removeEventListener('keydown', handleKeydown);
    };
  });
</script>

{#if visible}
  <div
    bind:this={containerRef}
    class={cn(
      // Professional popover styling using design system tokens
      'fixed z-[9999] bg-popover text-popover-foreground',
      'border border-border rounded-lg shadow-lg',
      'min-w-[340px] max-w-[400px] max-h-[320px]',
      'overflow-hidden',
      // Smooth animations matching design system
      'animate-in fade-in-0 zoom-in-95 duration-200'
    )}
    style="left: {smartPosition.x}px; top: {smartPosition.y}px;"
    role="listbox"
    aria-label="Slash command palette"
  >
    <!-- Professional header with command context -->
    <div class="px-4 py-3 border-b border-border bg-muted/30">
      <div class="flex items-center gap-2.5 text-xs">
        <div class="flex items-center gap-2">
          <div class="w-4 h-4 rounded-sm bg-primary/10 flex items-center justify-center">
            <span class="text-[10px] font-bold text-primary">/</span>
          </div>
          <span class="font-semibold text-foreground">Quick Commands</span>
        </div>
        {#if query}
          <div class="flex items-center gap-1.5 text-muted-foreground">
            <span>•</span>
            <code class="bg-muted/80 px-2 py-0.5 rounded-md text-[10px] font-mono border"
              >{query}</code
            >
          </div>
        {/if}
      </div>
    </div>

    <!-- Commands container -->
    <div class="overflow-y-auto max-h-[240px] py-1">
      {#if loading}
        <div class="flex items-center gap-3 px-4 py-5 text-sm text-muted-foreground">
          <div
            class="animate-spin h-4 w-4 border-2 border-primary/30 border-r-primary rounded-full"
          ></div>
          <span>Loading commands...</span>
        </div>
      {:else if commands.length === 0}
        <div class="px-4 py-8 text-center text-muted-foreground">
          {#if query}
            <div class="text-sm mb-2 font-medium">No commands found matching</div>
            <code class="bg-muted/80 px-2.5 py-1 rounded-md text-xs border">{query}</code>
          {:else}
            <div class="text-sm font-medium mb-1">No commands available</div>
            <div class="text-xs">Try typing after the / character</div>
          {/if}
        </div>
      {:else}
        {#each commands as command, index}
          <button
            bind:this={itemRefs[index]}
            class={cn(
              'w-full flex items-center gap-3 px-4 py-3 text-left transition-colors',
              'hover:bg-muted/50 focus:bg-muted/50 focus:outline-none',
              selectedIndex === index && 'bg-muted/70'
            )}
            role="option"
            aria-selected={selectedIndex === index}
            on:click={() => selectCommand(command)}
            on:mouseover={() => (selectedIndex = index)}
          >
            <!-- Command Icon - matching design system exactly -->
            <div class="flex-shrink-0 w-5 h-5 flex items-center justify-center">
              {#if command.id === 'text'}
                <div class="node-icon">
                  <div class="text-circle"></div>
                </div>
              {:else if command.id === 'header1'}
                <div class="ai-icon">
                  <div class="h1-square-alt"></div>
                </div>
              {:else if command.id === 'header2'}
                <div class="ai-icon">
                  <div class="h2-square-alt"></div>
                </div>
              {:else if command.id === 'header3'}
                <div class="ai-icon">
                  <div class="h3-square-alt"></div>
                </div>
              {:else if command.id === 'task'}
                <div class="task-icon">
                  <div class="task-circle task-circle-completed"></div>
                </div>
              {:else if command.id === 'ai-chat'}
                <div class="ai-icon">
                  <div class="ai-square-alt"></div>
                </div>
              {:else}
                <div class="node-icon">
                  <div class="text-circle"></div>
                </div>
              {/if}
            </div>

            <!-- Command Details -->
            <div class="flex-1 min-w-0">
              <div class="flex items-center justify-between">
                <div class="font-medium text-sm text-foreground truncate">
                  {command.name}
                </div>
                {#if command.shortcut}
                  <code class="ml-2 bg-muted/80 px-2 py-0.5 rounded text-[10px] font-mono border flex-shrink-0">
                    {command.shortcut}
                  </code>
                {/if}
              </div>
            </div>
          </button>
        {/each}
      {/if}
    </div>

    <!-- Footer with navigation hints -->
    <div class="px-4 py-2 border-t border-border bg-muted/20">
      <div class="flex items-center justify-between text-[10px] text-muted-foreground">
        <div class="flex items-center gap-3">
          <div class="flex items-center gap-1">
            <code class="bg-muted/80 px-1.5 py-0.5 rounded border">↑↓</code>
            <span>Navigate</span>
          </div>
          <div class="flex items-center gap-1">
            <code class="bg-muted/80 px-1.5 py-0.5 rounded border">Enter</code>
            <span>Select</span>
          </div>
        </div>
        <div class="flex items-center gap-1">
          <code class="bg-muted/80 px-1.5 py-0.5 rounded border">Esc</code>
          <span>Close</span>
        </div>
      </div>
    </div>
  </div>
{/if}

<style>
  /* Node Icons - matching design system exactly */
  .node-icon {
    width: 20px;
    height: 20px;
    display: flex;
    align-items: center;
    justify-content: center;
  }

  .text-circle {
    width: 8px;
    height: 8px;
    border-radius: 50%;
    background: hsl(200 40% 45%); /* Blue-gray for text nodes */
  }

  .task-icon {
    width: 20px;
    height: 20px;
    display: flex;
    align-items: center;
    justify-content: center;
  }

  .task-circle {
    width: 8px;
    height: 8px;
    border-radius: 50%;
    background: hsl(200 40% 45%);
  }

  .task-circle-completed {
    background: hsl(200 40% 45%);
  }

  .ai-icon {
    width: 20px;
    height: 20px;
    display: flex;
    align-items: center;
    justify-content: center;
  }

  .h1-square-alt,
  .h2-square-alt,
  .h3-square-alt,
  .ai-square-alt {
    width: 8px;
    height: 8px;
    background: hsl(200 40% 45%);
    border-radius: 1px;
  }
</style>