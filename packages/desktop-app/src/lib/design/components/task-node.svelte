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
    headerLevel = 0,
    children = [],
    editableConfig = { allowMultiline: true },
    metadata = {}
  }: {
    nodeId: string;
    nodeType?: string;
    autoFocus?: boolean;
    content?: string;
    headerLevel?: number;
    children?: string[];
    editableConfig?: object;
    metadata?: Record<string, unknown>;
  } = $props();

  const dispatch = createEventDispatcher();

  // Internal reactive state - sync with content prop changes like TextNodeViewer
  let internalContent = $state(content);

  // Sync internalContent when content prop changes externally (e.g., from pattern conversions)
  $effect(() => {
    internalContent = content;
  });

  // Task-specific state management - prioritize metadata over content parsing
  let taskState = $state<NodeState>((metadata.taskState as NodeState) || parseTaskState(content));

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
   */
  function updateTaskState(newState: NodeState) {
    taskState = newState;

    // Clean the content if it has any shortcut syntax from node conversion
    const cleanedContent = cleanContentForDisplay(content);
    if (cleanedContent !== content) {
      content = cleanedContent;
    }

    // Dispatch standard contentChanged event to update node manager
    dispatch('contentChanged', { content: content });

    // Also dispatch specialized taskStateChanged event
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
   * Update task state when content changes externally (only if content has task syntax)
   */
  $effect(() => {
    // Only override state if content actually contains task syntax
    if (/^\s*-?\s*\[(x|X|~|o|\s)\]/i.test(content.trim())) {
      const newState = parseTaskState(content);
      if (newState !== taskState) {
        taskState = newState;
      }
    }
  });

  /**
   * Handle content changes and sync with parent
   */
  function handleContentChange(event: CustomEvent<{ content: string }>) {
    const newContent = event.detail.content;
    internalContent = newContent;
    dispatch('contentChanged', { content: newContent });
  }

  /**
   * Forward all other events to parent components
   */
  function forwardEvent<T>(eventName: string) {
    return (event: CustomEvent<T>) => dispatch(eventName, event.detail);
  }
</script>

<!-- Wrap BaseNode with task-specific styling -->
<div class="task-node-wrapper" class:task-completed={taskState === 'completed'}>
  <BaseNode
    {nodeId}
    {nodeType}
    {autoFocus}
    bind:content={internalContent}
    {headerLevel}
    {children}
    {editableConfig}
    metadata={taskMetadata}
    on:iconClick={handleIconClick}
    on:createNewNode={forwardEvent('createNewNode')}
    on:contentChanged={handleContentChange}
    on:headerLevelChanged={forwardEvent('headerLevelChanged')}
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
    opacity: 0.7;
  }

  /* Apply completed styling to the content specifically */
  .task-completed :global(.node-content),
  .task-completed :global(.content-editable) {
    text-decoration: line-through;
    opacity: 0.7;
  }
</style>
