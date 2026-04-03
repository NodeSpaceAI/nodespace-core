<script lang="ts">
  import { agentStore } from '$lib/stores/agent-store.svelte';

  let isOpen = $state(false);

  const agents = $derived(agentStore.agents);
  const selectedAgent = $derived(agentStore.selectedAgent);

  function toggleDropdown() {
    isOpen = !isOpen;
  }

  function selectAgent(agentId: string) {
    agentStore.selectAgent(agentId);
    isOpen = false;
  }

  function handleBlur(event: FocusEvent) {
    // Close dropdown if focus moves outside the component
    const target = event.relatedTarget as HTMLElement | null;
    if (target && !target.closest('.agent-selector')) return;
    // Small delay to allow click handlers to fire
    setTimeout(() => { isOpen = false; }, 150);
  }
</script>

<div class="agent-selector" role="combobox" aria-expanded={isOpen} aria-haspopup="listbox">
  <button
    class="agent-selector-trigger"
    onclick={toggleDropdown}
    onblur={handleBlur}
    aria-label="Select AI agent"
  >
    <span class="agent-selector-value">
      {#if selectedAgent}
        <span class="availability-dot" class:available={selectedAgent.available}></span>
        {selectedAgent.name}
      {:else}
        Select agent
      {/if}
    </span>
    <svg class="chevron" class:chevron-open={isOpen} viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" width="14" height="14">
      <polyline points="6 9 12 15 18 9" />
    </svg>
  </button>

  {#if isOpen}
    <div class="agent-dropdown" role="listbox">
      {#each agents as agent (agent.id)}
        <button
          class="agent-option"
          class:selected={agent.id === agentStore.selectedAgentId}
          onclick={() => selectAgent(agent.id)}
          role="option"
          aria-selected={agent.id === agentStore.selectedAgentId}
          disabled={!agent.available}
        >
          <span class="availability-dot" class:available={agent.available}></span>
          <span class="agent-option-name">{agent.name}</span>
          {#if !agent.available}
            <span class="agent-status-badge">Not available</span>
          {/if}
        </button>
      {/each}
      {#if agents.length === 0}
        <div class="agent-option-empty">No agents detected</div>
      {/if}
    </div>
  {/if}
</div>

<style>
  .agent-selector {
    position: relative;
  }

  .agent-selector-trigger {
    display: flex;
    align-items: center;
    gap: 0.375rem;
    padding: 0.375rem 0.625rem;
    border: 1px solid hsl(var(--border));
    border-radius: 0.375rem;
    background: hsl(var(--background));
    color: hsl(var(--foreground));
    cursor: pointer;
    font-size: 0.8125rem;
    min-width: 10rem;
    transition: border-color 0.15s;
  }

  .agent-selector-trigger:hover {
    border-color: hsl(var(--ring));
  }

  .agent-selector-value {
    flex: 1;
    display: flex;
    align-items: center;
    gap: 0.375rem;
    text-align: left;
  }

  .chevron {
    flex-shrink: 0;
    transition: transform 0.15s;
    color: hsl(var(--muted-foreground));
  }

  .chevron-open {
    transform: rotate(180deg);
  }

  .agent-dropdown {
    position: absolute;
    top: calc(100% + 4px);
    right: 0;
    min-width: 100%;
    background: hsl(var(--background));
    border: 1px solid hsl(var(--border));
    border-radius: 0.375rem;
    box-shadow: 0 4px 12px hsl(0 0% 0% / 0.1);
    z-index: 50;
    overflow: hidden;
  }

  .agent-option {
    display: flex;
    align-items: center;
    gap: 0.375rem;
    width: 100%;
    padding: 0.5rem 0.75rem;
    border: none;
    background: none;
    color: hsl(var(--foreground));
    cursor: pointer;
    font-size: 0.8125rem;
    text-align: left;
    transition: background 0.1s;
  }

  .agent-option:hover:not(:disabled) {
    background: hsl(var(--accent));
  }

  .agent-option:disabled {
    opacity: 0.5;
    cursor: not-allowed;
  }

  .agent-option.selected {
    background: hsl(var(--accent));
    font-weight: 500;
  }

  .agent-option-name {
    flex: 1;
  }

  .agent-option-empty {
    padding: 0.75rem;
    font-size: 0.8125rem;
    color: hsl(var(--muted-foreground));
    text-align: center;
  }

  .availability-dot {
    width: 8px;
    height: 8px;
    border-radius: 50%;
    background: hsl(var(--muted-foreground) / 0.3);
    flex-shrink: 0;
  }

  .availability-dot.available {
    background: hsl(142 76% 36%);
  }

  .agent-status-badge {
    font-size: 0.6875rem;
    color: hsl(var(--muted-foreground));
    background: hsl(var(--muted));
    padding: 0.0625rem 0.375rem;
    border-radius: 9999px;
  }
</style>
