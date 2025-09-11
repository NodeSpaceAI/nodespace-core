<!--
  TaskNodeViewer - Custom viewer for task nodes
  
  Wraps BaseNode with task-specific UI:
  - Checkbox for task completion
  - Priority indicators
  - Due date display
  - Task-specific styling
-->

<script lang="ts">
  import { createEventDispatcher } from 'svelte';
  import BaseNode from '$lib/design/components/base-node.svelte';
  import Icon from '$lib/design/icons/icon.svelte';
  import type { ViewerComponentProps } from './baseViewer.js';

  // Props following the new viewer interface
  let {
    nodeId,
    content = '',
    autoFocus = false,
    nodeType = 'task',
    inheritHeaderLevel = 0,
    children = []
  }: ViewerComponentProps = $props();

  const dispatch = createEventDispatcher();

  // Task-specific state
  let isCompleted = $state(parseCompletionStatus(content));
  let priority = $state(parsePriority(content) || 'medium');
  let dueDate = $state(parseDueDate(content));

  const priorityColors = {
    high: 'hsl(0 84% 60%)',
    medium: 'hsl(38 92% 50%)',
    low: 'hsl(142 71% 45%)'
  };

  /**
   * Parse task completion status from content
   */
  function parseCompletionStatus(content: string): boolean {
    // Look for markdown-style task checkboxes
    return /^\s*-\s*\[x\]/i.test(content);
  }

  /**
   * Parse priority from content
   */
  function parsePriority(content: string): 'high' | 'medium' | 'low' | null {
    const priorityMatch = content.match(/priority:(high|medium|low)/i);
    return priorityMatch ? (priorityMatch[1].toLowerCase() as 'high' | 'medium' | 'low') : null;
  }

  /**
   * Parse due date from content
   */
  function parseDueDate(content: string): Date | null {
    const dueDateMatch = content.match(/due:(\d{4}-\d{2}-\d{2})/);
    return dueDateMatch ? new Date(dueDateMatch[1]) : null;
  }

  /**
   * Toggle task completion
   */
  function toggleCompletion() {
    isCompleted = !isCompleted;

    // Update content with new completion status
    let newContent = content;
    if (isCompleted) {
      // Mark as completed
      newContent = newContent.replace(/^\s*-\s*\[\s*\]/, '- [x]');
      if (!newContent.match(/^\s*-\s*\[/)) {
        newContent = `- [x] ${newContent}`;
      }
    } else {
      // Mark as incomplete
      newContent = newContent.replace(/^\s*-\s*\[x\]/i, '- [ ]');
    }

    dispatch('contentChanged', { content: newContent });
  }

  /**
   * Update priority
   */
  function updatePriority(newPriority: 'high' | 'medium' | 'low') {
    priority = newPriority;

    // Update content with new priority
    let newContent = content;
    if (newContent.includes('priority:')) {
      newContent = newContent.replace(/priority:(high|medium|low)/i, `priority:${newPriority}`);
    } else {
      newContent = `${newContent} priority:${newPriority}`;
    }

    dispatch('contentChanged', { content: newContent });
  }

  /**
   * Forward event handlers to maintain BaseNodeViewer compatibility
   */
  function forwardEvent<T>(eventName: string) {
    return (event: CustomEvent<T>) => dispatch(eventName, event.detail);
  }
</script>

<div class="task-node-viewer">
  <!-- Task Header with Controls -->
  <div class="task-header">
    <div class="task-controls">
      <!-- Completion Checkbox -->
      <button
        class="task-checkbox"
        class:completed={isCompleted}
        onclick={toggleCompletion}
        aria-label={isCompleted ? 'Mark as incomplete' : 'Mark as complete'}
        type="button"
      >
        {#if isCompleted}
          <Icon name="taskComplete" size={12} />
        {/if}
      </button>

      <!-- Priority Indicator -->
      <div class="priority-indicator">
        <span
          class="priority-dot"
          style="background-color: {priorityColors[priority]}"
          title="Priority: {priority}"
        ></span>
      </div>

      <!-- Due Date (if exists) -->
      {#if dueDate}
        <div class="due-date">
          <Icon name="calendar" size={12} />
          <span class="due-text">
            {dueDate.toLocaleDateString('en-US', { month: 'short', day: 'numeric' })}
          </span>
        </div>
      {/if}
    </div>

    <!-- Priority Selector -->
    <div class="priority-selector">
      {#each (['high', 'medium', 'low'] as const) as p}
        <button
          class="priority-btn"
          class:active={priority === p}
          onclick={() => updatePriority(p)}
          style="border-color: {priorityColors[p]}"
          type="button"
        >
          {p}
        </button>
      {/each}
    </div>
  </div>

  <!-- Wrapped BaseNode for content editing -->
  <div class="task-content" class:completed={isCompleted}>
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
  .task-node-viewer {
    display: flex;
    flex-direction: column;
    gap: 0.5rem;
  }

  .task-header {
    display: flex;
    align-items: center;
    justify-content: space-between;
    padding: 0.5rem;
    border: 1px solid hsl(var(--border));
    border-radius: 0.375rem;
    background: hsl(var(--muted) / 0.2);
  }

  .task-controls {
    display: flex;
    align-items: center;
    gap: 0.75rem;
  }

  .task-checkbox {
    width: 20px;
    height: 20px;
    border: 2px solid hsl(var(--border));
    border-radius: 0.25rem;
    background: hsl(var(--background));
    cursor: pointer;
    display: flex;
    align-items: center;
    justify-content: center;
    transition: all 0.15s ease;
    padding: 0;
  }

  .task-checkbox:hover {
    border-color: hsl(var(--primary));
  }

  .task-checkbox.completed {
    background: hsl(var(--primary));
    border-color: hsl(var(--primary));
    color: hsl(var(--primary-foreground));
  }

  .priority-indicator {
    display: flex;
    align-items: center;
  }

  .priority-dot {
    width: 8px;
    height: 8px;
    border-radius: 50%;
  }

  .due-date {
    display: flex;
    align-items: center;
    gap: 0.25rem;
    font-size: 0.75rem;
    color: hsl(var(--muted-foreground));
  }

  .due-text {
    font-weight: 500;
  }

  .priority-selector {
    display: flex;
    gap: 0.25rem;
  }

  .priority-btn {
    padding: 0.25rem 0.5rem;
    border: 1px solid;
    border-radius: 0.25rem;
    background: hsl(var(--background));
    color: hsl(var(--foreground));
    font-size: 0.75rem;
    cursor: pointer;
    transition: all 0.15s ease;
    text-transform: capitalize;
  }

  .priority-btn:hover {
    background: hsl(var(--muted));
  }

  .priority-btn.active {
    background: currentColor;
    color: white;
  }

  .task-content {
    transition: opacity 0.15s ease;
  }

  .task-content.completed {
    opacity: 0.6;
  }

  .task-content.completed :global(.node__content) {
    text-decoration: line-through;
  }
</style>
