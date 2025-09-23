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
    editableConfig = { allowMultiline: true }
  }: {
    nodeId: string;
    nodeType?: string;
    autoFocus?: boolean;
    content?: string;
    headerLevel?: number;
    children?: string[];
    editableConfig?: object;
  } = $props();

  const dispatch = createEventDispatcher();

  // Task-specific state management
  let taskState = $state<NodeState>(parseTaskState(content));

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
    if (/^\s*-?\s*\[~o\]/i.test(trimmed)) {
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
   * Update task state and content when state changes
   */
  function updateTaskState(newState: NodeState) {
    taskState = newState;

    // Update content to reflect new state
    let newContent = content;

    // Remove existing task syntax
    newContent = newContent.replace(/^\s*-?\s*\[[x~o\s]*\]\s*/i, '');

    // Add new task syntax based on state
    switch (newState) {
      case 'completed':
        newContent = `- [x] ${newContent}`;
        break;
      case 'inProgress':
        newContent = `- [~] ${newContent}`;
        break;
      case 'pending':
      default:
        newContent = `- [ ] ${newContent}`;
        break;
    }

    // Dispatch content change
    dispatch('contentChanged', { content: newContent.trim() });
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
   * Update task state when content changes externally
   */
  $effect(() => {
    const newState = parseTaskState(content);
    if (newState !== taskState) {
      taskState = newState;
    }
  });

  /**
   * Forward all other events to parent components
   */
  function forwardEvent<T>(eventName: string) {
    return (event: CustomEvent<T>) => dispatch(eventName, event.detail);
  }
</script>

<!-- Wrap BaseNode with task-specific metadata -->
<BaseNode
  {nodeId}
  {nodeType}
  {autoFocus}
  {content}
  {headerLevel}
  {children}
  {editableConfig}
  metadata={{ taskState }}
  on:iconClick={handleIconClick}
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
  on:slashCommandSelected={forwardEvent('slashCommandSelected')}
/>