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
  import { createEventDispatcher, getContext } from 'svelte';
  import BaseNode from './base-node.svelte';
  import type { NodeState } from '$lib/design/icons/registry';
  import { getNavigationService } from '$lib/services/navigation-service';
  import { DEFAULT_PANE_ID } from '$lib/stores/navigation';
  import { nodeData } from '$lib/stores/reactive-node-data.svelte';
  import { structureTree } from '$lib/stores/reactive-structure-tree.svelte';

  // Get paneId from context (set by PaneContent) - identifies which pane this node is in
  const sourcePaneId = getContext<string>('paneId') ?? DEFAULT_PANE_ID;

  // Props using Svelte 5 runes mode - same interface as BaseNode
  let {
    nodeId,
    nodeType: propsNodeType = 'task',
    autoFocus = false,
    content: propsContent = '',
    children: propsChildren = [],
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

  // Use reactive stores directly instead of relying on props
  // Components query the stores for current data and re-render automatically when data changes
  let node = $derived(nodeData.getNode(nodeId));
  let childIds = $derived(structureTree.getChildren(nodeId));

  // Derive props from stores with fallback to passed props for backward compatibility
  let content = $derived(node?.content ?? propsContent);
  let nodeType = $derived(node?.nodeType ?? propsNodeType);
  let children = $derived(childIds ?? propsChildren);

  // Track if user is actively typing (hide button during typing)
  let isTyping = $state(false);
  let typingTimer: ReturnType<typeof setTimeout> | undefined;

  function handleTypingStart() {
    isTyping = true;
    // Clear existing timer
    if (typingTimer) clearTimeout(typingTimer);
    // Hide button for 1 second after last keypress
    typingTimer = setTimeout(() => {
      isTyping = false;
    }, 1000);
  }

  // Reset typing state on mouse movement
  function handleMouseMove() {
    if (isTyping) {
      if (typingTimer) clearTimeout(typingTimer);
      isTyping = false;
    }
  }

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
   * Handle open button click to navigate to task viewer
   */
  async function handleOpenClick(event: MouseEvent) {
    event.preventDefault();
    event.stopPropagation();

    const navigationService = getNavigationService();

    // Regular click: Open in dedicated viewer pane (other pane)
    // Modifier keys provide alternative behaviors
    if (event.metaKey || event.ctrlKey) {
      // Cmd+Click: Open in new tab in source pane (where button was clicked)
      await navigationService.navigateToNode(nodeId, true, sourcePaneId);
    } else {
      // Regular click: Open in dedicated viewer pane (creates new pane if needed)
      // Pass sourcePaneId so it opens in the OTHER pane, not based on focus
      await navigationService.navigateToNodeInOtherPane(nodeId, sourcePaneId);
    }
  }

  /**
   * Forward all other events to parent components
   */
  function forwardEvent<T>(eventName: string) {
    return (event: CustomEvent<T>) => dispatch(eventName, event.detail);
  }
</script>

<!-- Wrap BaseNode with task-specific styling -->
<!-- REFACTOR (Issue #316): Using bind:content and on:contentChanged instead of internalContent and handleContentChange -->
<div
  class="task-node-wrapper"
  class:task-completed={taskState === 'completed'}
  class:typing={isTyping}
  onmousemove={handleMouseMove}
  role="group"
  aria-label="Task node"
>
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
    on:contentChanged={(e) => {
      handleTypingStart(); // Track typing to hide Open button
      dispatch('contentChanged', e.detail);
    }}
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

  <!-- Open button (appears on hover) -->
  <button
    class="task-open-button"
    onclick={handleOpenClick}
    type="button"
    aria-label="Open task in dedicated viewer pane (Cmd+Click for new tab in same pane)"
    title="Open task in viewer"
  >
    open
  </button>
</div>

<style>
  /* Task node wrapper - position relative for absolute button positioning */
  .task-node-wrapper {
    position: relative;
    /* width: 100% handled by parent .node-content-wrapper flex child rule */
  }

  /* Completed task styling following design system */
  .task-completed {
    text-decoration: line-through;
  }

  /* Apply completed styling to the content specifically */
  .task-completed :global(.node__content) {
    text-decoration: line-through;
    opacity: 0.7;
  }

  /* Open button (top-right, appears on hover) - matches code block copy button */
  .task-open-button {
    position: absolute;
    top: 0.25rem;
    right: 0.25rem;
    background: hsl(var(--background));
    border: 1px solid hsl(var(--border));
    color: hsl(var(--foreground));
    padding: 0.25rem 0.5rem;
    border-radius: 0.25rem;
    font-size: 0.75rem;
    cursor: pointer;
    opacity: 0;
    transition: opacity 0.2s ease;
    text-transform: lowercase;
    z-index: 5; /* Below popovers (1001) but above node content */
  }

  /* Show button on hover, but hide while actively typing */
  .task-node-wrapper:hover:not(.typing) .task-open-button {
    opacity: 1;
  }

  /* Hover state for better feedback */
  .task-open-button:hover {
    background: hsl(var(--muted));
  }
</style>
