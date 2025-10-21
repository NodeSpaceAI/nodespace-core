<!--
  TaskNode - Wraps BaseNode with task-specific functionality

  Responsibilities:
  - Manages task state (pending/inProgress/completed)
  - Provides task-specific icon management
  - Handles task state changes via icon clicks
  - Forwards all other events to BaseNode

  Integration:
  - Uses icon registry for proper TaskIcon rendering
  - Maintains compatibility with BaseNode API
  - Works seamlessly in node tree structure
-->

<script lang="ts">
  import { createEventDispatcher } from 'svelte';
  import BaseNode from './base-node.svelte';
  import type { NodeState } from '$lib/design/icons/registry';

  // Props using Svelte 5 runes mode - same interface as BaseNode
  let {
    nodeId,
    nodeType = 'task',
    autoFocus = false,
    content = '',
    children = [],
    metadata = {}
  }: {
    nodeId: string;
    nodeType?: string;
    autoFocus?: boolean;
    content?: string;
    children?: string[];
    metadata?: Record<string, unknown>;
  } = $props();

  const dispatch = createEventDispatcher();

  // REFACTOR (Issue #316): Removed $effect for prop sync, will use bind:content instead
  // Replaced $effect with $derived.by() for task state detection

  // Task-specific state management using $derived.by() for reactive computation
  // When content has task syntax, derive from content; otherwise use metadata
  let taskState = $derived.by(() => {
    const hasTaskSyntax = /^\s*-?\s*\[(x|X|~|o|\s)\]/i.test(content.trim());
    return hasTaskSyntax ? parseTaskState(content) : (metadata.taskState as NodeState) || 'pending';
  });

  // TaskNodes use default single-line editing
  const editableConfig = {};

  // Create reactive metadata object
  let taskMetadata = $derived({ taskState });

  /**
   * Parse task state from content
   * - Looks for markdown task syntax: [ ], [x], [~]
   * - Returns appropriate NodeState
   */
  function parseTaskState(content: string): NodeState {
    const trimmed = content.trim();

    // Check for completed task: [x] or [X]
    if (/^\s*-?\s*\[x\]/i.test(trimmed)) {
      return 'completed';
    }

    // Check for in-progress task: [~] or [o]
    if (/^\s*-?\s*\[~|o\]/i.test(trimmed)) {
      return 'inProgress';
    }

    // Check for pending task: [ ] or empty
    if (/^\s*-?\s*\[\s*\]/i.test(trimmed) || trimmed === '') {
      return 'pending';
    }

    // Default to pending for task nodes
    return 'pending';
  }

  /**
   * Clean content by removing task syntax shortcut markers
   */
  function cleanContentForDisplay(content: string): string {
    return content.replace(/^\s*-?\s*\[[x~o\s]*\]\s*/i, '').trim();
  }

  /**
   * Update task state and sync with node manager
   *
   * REFACTOR (Issue #316): Removed direct taskState assignment since it's now $derived
   * State updates flow through content/metadata changes, and taskState derives reactively
   *
   * WHY THIS FUNCTION STILL EXISTS:
   * Even though taskState is now a derived value (computed automatically from content/metadata),
   * we still need this function to:
   * 1. Handle icon click events (user cycling through states)
   * 2. Dispatch events to parent components (contentChanged, taskStateChanged)
   * 3. Clean content when converting from other node types
   *
   * The function doesn't SET taskState directly anymore - instead it updates the underlying
   * content/metadata, and taskState automatically recomputes via $derived.by()
   */
  function updateTaskState(newState: NodeState) {
    // Note: taskState is now $derived, so it will update automatically
    // when we update the content or metadata below

    // Clean the content if it has any shortcut syntax from node conversion
    const cleanedContent = cleanContentForDisplay(content);
    if (cleanedContent !== content) {
      content = cleanedContent;
    }

    // Dispatch standard contentChanged event to update node manager
    dispatch('contentChanged', { content: content });

    // Also dispatch specialized taskStateChanged event with the new state
    dispatch('taskStateChanged', {
      nodeId,
      state: newState,
      content: content
    });
  }

  /**
   * Handle icon click to cycle through task states
   */
  function handleIconClick(event: CustomEvent) {
    event.preventDefault();
    event.stopPropagation();

    // Cycle through states: pending -> inProgress -> completed -> pending
    let nextState: NodeState;
    switch (taskState) {
      case 'pending':
        nextState = 'inProgress';
        break;
      case 'inProgress':
        nextState = 'completed';
        break;
      case 'completed':
        nextState = 'pending';
        break;
      default:
        nextState = 'pending';
    }

    updateTaskState(nextState);
  }

  /**
   * REFACTOR (Issue #316): Removed $effect - taskState now derives automatically via $derived.by()
   * No need for manual synchronization - state updates reactively from content/metadata changes
   */

  /**
   * Forward all other events to parent components
   */
  function forwardEvent<T>(eventName: string) {
    return (event: CustomEvent<T>) => dispatch(eventName, event.detail);
  }
</script>

<!-- Wrap BaseNode with task-specific styling -->
<!-- REFACTOR (Issue #316): Using bind:content and on:contentChanged instead of internalContent and handleContentChange -->
<div class="task-node-wrapper" class:task-completed={taskState === 'completed'}>
  <BaseNode
    {nodeId}
    {nodeType}
    {autoFocus}
    bind:content
    {children}
    {editableConfig}
    metadata={taskMetadata}
    on:iconClick={handleIconClick}
    on:createNewNode={forwardEvent('createNewNode')}
    on:contentChanged
    on:indentNode={forwardEvent('indentNode')}
    on:outdentNode={forwardEvent('outdentNode')}
    on:navigateArrow={forwardEvent('navigateArrow')}
    on:combineWithPrevious={forwardEvent('combineWithPrevious')}
    on:deleteNode={forwardEvent('deleteNode')}
    on:focus={forwardEvent('focus')}
    on:blur={forwardEvent('blur')}
    on:nodeReferenceSelected={forwardEvent('nodeReferenceSelected')}
    on:slashCommandSelected={forwardEvent('slashCommandSelected')}
    on:nodeTypeChanged={forwardEvent('nodeTypeChanged')}
  />
</div>

<style>
  /* Completed task styling following design system */
  .task-completed {
    text-decoration: line-through;
  }

  /* Apply completed styling to the content specifically */
  .task-completed :global(.node__content) {
    text-decoration: line-through;
    opacity: 0.7;
  }
</style>
