<!--
  AIChatNodeViewer - Custom viewer for AI chat nodes
  
  Wraps BaseNode with AI-specific UI:
  - Chat bubble styling
  - Role indicators (user/assistant/system)
  - Token count display
  - Response regeneration controls
-->

<script lang="ts">
  import { createEventDispatcher } from 'svelte';
  import BaseNode from '$lib/design/components/base-node.svelte';
  import Icon from '$lib/design/icons/icon.svelte';
  import type { ViewerComponentProps } from './base-viewer.js';

  // Props following the new viewer interface
  let { 
    nodeId,
    content = '',
    autoFocus = false,
    nodeType = 'ai-chat',
    inheritHeaderLevel = 0,
    children = []
  }: ViewerComponentProps = $props();

  const dispatch = createEventDispatcher();

  // AI chat-specific state
  let role = $state(parseRole(content) || 'user');
  let model = $state(parseModel(content) || 'claude-3');
  let tokenCount = $state(parseTokenCount(content) || estimateTokens(content));
  let timestamp = $state(parseTimestamp(content) || new Date());
  
  const roleColors = {
    user: 'hsl(142 71% 45%)',
    assistant: 'hsl(221 83% 53%)',
    system: 'hsl(271 81% 56%)'
  };

  const roleIcons = {
    user: 'user',
    assistant: 'cpu',
    system: 'settings'
  };

  /**
   * Parse role from content
   */
  function parseRole(content: string): 'user' | 'assistant' | 'system' | null {
    const roleMatch = content.match(/role:(user|assistant|system)/i);
    return roleMatch ? roleMatch[1].toLowerCase() as 'user' | 'assistant' | 'system' : null;
  }

  /**
   * Parse model from content
   */
  function parseModel(content: string): string | null {
    const modelMatch = content.match(/model:([\w-]+)/i);
    return modelMatch ? modelMatch[1] : null;
  }

  /**
   * Parse token count from content
   */
  function parseTokenCount(content: string): number | null {
    const tokenMatch = content.match(/tokens:(\d+)/i);
    return tokenMatch ? parseInt(tokenMatch[1]) : null;
  }

  /**
   * Parse timestamp from content
   */
  function parseTimestamp(content: string): Date | null {
    const timestampMatch = content.match(/timestamp:(\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2})/);
    return timestampMatch ? new Date(timestampMatch[1]) : null;
  }

  /**
   * Estimate token count (rough approximation)
   */
  function estimateTokens(text: string): number {
    // Rough estimation: ~4 characters per token
    return Math.ceil(text.length / 4);
  }

  /**
   * Change role
   */
  function changeRole(newRole: 'user' | 'assistant' | 'system') {
    role = newRole;
    
    // Update content with new role
    let newContent = content;
    if (newContent.includes('role:')) {
      newContent = newContent.replace(/role:(user|assistant|system)/i, `role:${newRole}`);
    } else {
      newContent = `role:${newRole} ${newContent}`;
    }
    
    dispatch('contentChanged', { content: newContent });
  }

  /**
   * Regenerate response (for assistant messages)
   */
  function regenerateResponse() {
    // In a real implementation, this would trigger AI regeneration
    console.log('Regenerating AI response for node:', nodeId);
    // For demo, just add a marker
    const newContent = `${content} [regenerated]`;
    dispatch('contentChanged', { content: newContent });
  }

  /**
   * Forward event handlers to maintain BaseNodeViewer compatibility
   */
  function forwardEvent<T>(eventName: string) {
    return (event: CustomEvent<T>) => dispatch(eventName, event.detail);
  }

  // Update token count when content changes
  $effect(() => {
    tokenCount = parseTokenCount(content) || estimateTokens(content);
  });
</script>

<div class="ai-chat-node-viewer">
  <!-- Chat Header -->
  <div class="chat-header" style="border-left-color: {roleColors[role]}">
    <div class="chat-info">
      <!-- Role Indicator -->
      <div class="role-indicator" style="background-color: {roleColors[role]}">
        <Icon name={roleIcons[role]} size={12} color="white" />
        <span class="role-text">{role}</span>
      </div>

      <!-- Model Info (for assistant messages) -->
      {#if role === 'assistant'}
        <div class="model-info">
          <Icon name="cpu" size={12} />
          <span class="model-text">{model}</span>
        </div>
      {/if}

      <!-- Token Count -->
      <div class="token-count">
        <span class="token-text">{tokenCount} tokens</span>
      </div>

      <!-- Timestamp -->
      <div class="timestamp">
        <span class="timestamp-text">
          {timestamp.toLocaleTimeString('en-US', { 
            hour: '2-digit', 
            minute: '2-digit' 
          })}
        </span>
      </div>
    </div>

    <div class="chat-actions">
      <!-- Role Selector -->
      <div class="role-selector">
        {#each ['user', 'assistant', 'system'] as r}
          <button 
            class="role-btn"
            class:active={role === r}
            onclick={() => changeRole(r)}
            style="border-color: {roleColors[r]}"
            type="button"
            title="Change to {r}"
          >
            <Icon name={roleIcons[r]} size={10} />
          </button>
        {/each}
      </div>

      <!-- Regenerate Button (for assistant messages) -->
      {#if role === 'assistant'}
        <button 
          class="regenerate-btn"
          onclick={regenerateResponse}
          title="Regenerate response"
          type="button"
        >
          <Icon name="refresh" size={12} />
        </button>
      {/if}
    </div>
  </div>

  <!-- Chat Content -->
  <div class="chat-content" class:chat-bubble={role !== 'system'}>
    <BaseNode
      {nodeId}
      {nodeType}
      {autoFocus}
      {content}
      headerLevel={inheritHeaderLevel}
      {children}
      editableConfig={{ allowMultiline: true }}
      on:createNewNode={forwardEvent('createNewNode')}
      on:contentChanged={forwardEvent('contentChanged')}
      on:headerLevelChanged={forwardEvent('headerLevelChanged')}
      on:indentNode={forwardEvent('indentNode')}
      on:outdentNode={forwardEvent('outdentNode')}
      on:navigateArrow={forwardEvent('navigateArrow')}
      on:combineWithPrevious={forwardEvent('combineWithPrevious')}
      on:deleteNode={forwardEvent('deleteNode')}
      on:focus={forwardEvent('focus')}
      on:blur={forwardEvent('blur')}
      on:nodeReferenceSelected={forwardEvent('nodeReferenceSelected')}
    />
  </div>
</div>

<style>
  .ai-chat-node-viewer {
    display: flex;
    flex-direction: column;
    gap: 0.5rem;
  }

  .chat-header {
    display: flex;
    align-items: center;
    justify-content: space-between;
    padding: 0.5rem;
    border: 1px solid hsl(var(--border));
    border-left-width: 4px;
    border-radius: 0.375rem;
    background: hsl(var(--muted) / 0.2);
  }

  .chat-info {
    display: flex;
    align-items: center;
    gap: 0.75rem;
  }

  .role-indicator {
    display: flex;
    align-items: center;
    gap: 0.25rem;
    padding: 0.25rem 0.5rem;
    border-radius: 0.25rem;
    color: white;
    font-size: 0.75rem;
    font-weight: 500;
  }

  .role-text {
    text-transform: capitalize;
  }

  .model-info {
    display: flex;
    align-items: center;
    gap: 0.25rem;
    font-size: 0.75rem;
    color: hsl(var(--muted-foreground));
  }

  .model-text {
    font-family: monospace;
  }

  .token-count {
    font-size: 0.75rem;
    color: hsl(var(--muted-foreground));
  }

  .timestamp {
    font-size: 0.75rem;
    color: hsl(var(--muted-foreground));
  }

  .chat-actions {
    display: flex;
    align-items: center;
    gap: 0.5rem;
  }

  .role-selector {
    display: flex;
    gap: 0.125rem;
  }

  .role-btn {
    width: 24px;
    height: 24px;
    border: 1px solid;
    border-radius: 0.25rem;
    background: hsl(var(--background));
    cursor: pointer;
    display: flex;
    align-items: center;
    justify-content: center;
    transition: all 0.15s ease;
    padding: 0;
  }

  .role-btn:hover {
    background: hsl(var(--muted));
  }

  .role-btn.active {
    background: currentColor;
    color: white;
  }

  .regenerate-btn {
    width: 24px;
    height: 24px;
    border: 1px solid hsl(var(--border));
    border-radius: 0.25rem;
    background: hsl(var(--background));
    color: hsl(var(--muted-foreground));
    cursor: pointer;
    display: flex;
    align-items: center;
    justify-content: center;
    transition: all 0.15s ease;
    padding: 0;
  }

  .regenerate-btn:hover {
    background: hsl(var(--muted));
    color: hsl(var(--foreground));
  }

  .chat-content {
    position: relative;
  }

  .chat-content.chat-bubble :global(.node) {
    background: hsl(var(--muted) / 0.1);
    border-radius: 0.5rem;
    padding: 0.75rem;
  }

  .chat-content.chat-bubble :global(.node__content) {
    line-height: 1.5;
  }
</style>