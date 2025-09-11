<!--
  BaseNode - Abstract Core Component (Internal Use Only)
  
  IMPORTANT: This component should NOT be used directly in application code.
  It serves as the abstract foundation for all node viewers and should only 
  be consumed by concrete node viewer implementations like TextNodeViewer.
  
  Core Features:
  - ContentEditableController integration for markdown syntax handling
  - Dual-representation with focus/blur state management
  - Universal keyboard shortcuts (Enter, Backspace, Cmd+B/I, etc.)
  - Node reference autocomplete system (@-trigger)
  - Base styling and layout for all node types
  
  Architecture:
  - Abstract Base: Provides core functionality but should not be instantiated directly
  - Concrete Viewers: TextNodeViewer, TaskNodeViewer, etc. wrap this component
  - Plugin System: Registered viewers extend this base with type-specific logic
-->

<script lang="ts">
  import { createEventDispatcher, onDestroy } from 'svelte';
  import Icon, { type NodeType } from '$lib/design/icons';

  import {
    ContentEditableController,
    type ContentEditableEvents,
    type ContentEditableConfig
  } from './contentEditableController.js';
  import { NodeAutocomplete, type NodeResult } from '$lib/components/ui/node-autocomplete';
  import type { TriggerContext } from '$lib/services/nodeReferenceService';
  import { getNodeServices } from '$lib/contexts/node-service-context.svelte';

  // Props (Svelte 5 runes syntax) - nodeReferenceService removed
  let {
    nodeId,
    nodeType = 'text',
    autoFocus = false,
    content = $bindable(''),
    headerLevel = 0,
    children = [],
    editableConfig = {}
  }: {
    nodeId: string;
    nodeType?: string;
    autoFocus?: boolean;
    content?: string;
    headerLevel?: number;
    children?: unknown[];
    editableConfig?: ContentEditableConfig;
  } = $props();

  // Get services from context
  const services = getNodeServices();
  const nodeReferenceService = services?.nodeReferenceService || null;

  // DOM element and controller - Svelte bind:this assignment
  let contentEditableElement: HTMLDivElement | undefined = undefined;
  let controller: ContentEditableController | null = null;

  // Autocomplete modal state
  let showAutocomplete = $state(false);
  let autocompletePosition = $state({ x: 0, y: 0 });
  let currentQuery = $state('');
  let autocompleteResults = $state<NodeResult[]>([]);
  let autocompleteLoading = $state(false);

  // Generate mock autocomplete results based on query
  function generateMockResults(query: string): NodeResult[] {
    const mockNodes: NodeResult[] = [
      {
        id: 'mock-node-1',
        title: 'Welcome to NodeSpace',
        type: 'text',
        subtitle: 'Getting started with your knowledge management system',
        metadata: '2 days ago'
      },
      {
        id: 'mock-node-2',
        title: 'Project Notes',
        type: 'document',
        subtitle: 'Comprehensive project documentation and meeting notes',
        metadata: '1 week ago'
      },
      {
        id: 'mock-node-3',
        title: 'Task List',
        type: 'task',
        subtitle: 'Important tasks and action items for the current sprint',
        metadata: '3 days ago'
      },
      {
        id: 'mock-node-4',
        title: 'AI Research Chat',
        type: 'ai-chat',
        subtitle: 'Conversation about machine learning and AI development',
        metadata: '5 days ago'
      },
      {
        id: 'mock-node-5',
        title: 'User Research Findings',
        type: 'user',
        subtitle: 'Key insights from user interviews and usability testing',
        metadata: '1 week ago'
      },
      {
        id: 'mock-node-6',
        title: 'Search Query Examples',
        type: 'query',
        subtitle: 'Commonly used search patterns and filters',
        metadata: '4 days ago'
      }
    ];

    if (!query.trim()) {
      return mockNodes.slice(0, 4); // Show top 4 when no query
    }

    // Filter results based on query
    return mockNodes.filter(
      (node) =>
        node.title.toLowerCase().includes(query.toLowerCase()) ||
        (node.subtitle && node.subtitle.toLowerCase().includes(query.toLowerCase()))
    );
  }

  // Reactive effect to update autocomplete results when query changes
  $effect(() => {
    if (showAutocomplete && nodeReferenceService) {
      autocompleteResults = generateMockResults(currentQuery);
    }
  });

  // Autocomplete event handlers
  function handleAutocompleteSelect(event: CustomEvent<NodeResult>) {
    const result = event.detail;

    if (controller) {
      // Insert node reference in markdown link format: [nodeTitle](nodespace://nodeId)
      // This replaces the @ trigger text with the proper reference format
      controller.insertNodeReference(result.id, result.title);
    }

    // Hide autocomplete and clear state
    showAutocomplete = false;
    currentQuery = '';
    autocompleteResults = [];

    // Emit event for parent components to handle if needed
    dispatch('nodeReferenceSelected', {
      nodeId: result.id,
      nodeTitle: result.title
    });
  }

  // Event dispatcher
  const dispatch = createEventDispatcher<{
    contentChanged: { content: string };
    headerLevelChanged: { level: number };
    focus: void;
    blur: void;
    createNewNode: {
      afterNodeId: string;
      nodeType: string;
      currentContent?: string;
      newContent?: string;
    };
    indentNode: { nodeId: string };
    outdentNode: { nodeId: string };
    navigateArrow: { nodeId: string; direction: 'up' | 'down'; columnHint: number };
    combineWithPrevious: { nodeId: string; currentContent: string };
    deleteNode: { nodeId: string };
    nodeReferenceSelected: { nodeId: string; nodeTitle: string };
  }>();

  // Controller event handlers
  const controllerEvents: ContentEditableEvents = {
    contentChanged: (content: string) => dispatch('contentChanged', { content }),
    headerLevelChanged: (level: number) => dispatch('headerLevelChanged', { level }),
    focus: () => dispatch('focus'),
    blur: () => {
      // Hide autocomplete modal when losing focus
      // Delay hiding autocomplete to prevent it from being hidden during DOM manipulation
      // The DOM updates from setLiveFormattedContent can cause brief focus loss
      setTimeout(() => {
        // Only hide if autocomplete is still showing and we're not actively typing
        if (showAutocomplete && controller && !controller.isProcessingInput()) {
          showAutocomplete = false;
        }
      }, 50); // Small delay to let input processing complete
      dispatch('blur');
    },
    createNewNode: (data) => dispatch('createNewNode', data),
    indentNode: (data) => dispatch('indentNode', data),
    outdentNode: (data) => dispatch('outdentNode', data),
    navigateArrow: (data) => dispatch('navigateArrow', data),
    combineWithPrevious: (data) => dispatch('combineWithPrevious', data),
    deleteNode: (data) => dispatch('deleteNode', data),
    // @ Trigger System Events
    triggerDetected: (data: {
      triggerContext: TriggerContext;
      cursorPosition: { x: number; y: number };
    }) => {
      if (nodeReferenceService) {
        currentQuery = data.triggerContext.query;
        autocompletePosition = data.cursorPosition;
        autocompleteResults = generateMockResults(currentQuery);
        showAutocomplete = true;
      }
    },
    triggerHidden: () => {
      showAutocomplete = false;
      currentQuery = '';
      autocompleteResults = [];
    },
    nodeReferenceSelected: (data: { nodeId: string; nodeTitle: string }) => {
      // Forward the event for potential parent component handling
      dispatch('nodeReferenceSelected', data);
    }
  };

  // Initialize controller when element is available (Svelte 5 $effect)
  // Note: Must explicitly access contentEditableElement to track dependency
  $effect(() => {
    const element = contentEditableElement; // Force dependency tracking
    if (element && !controller) {
      controller = new ContentEditableController(element, nodeId, controllerEvents, editableConfig);
      controller.initialize(content, autoFocus);
    }
  });

  // Update content when prop changes
  $effect(() => {
    if (controller && content !== undefined) {
      controller.updateContent(content);
    }
  });

  // Focus programmatically when autoFocus changes
  $effect(() => {
    if (controller && autoFocus) {
      controller.focus();
    }
  });

  onDestroy(() => {
    if (controller) {
      controller.destroy();
    }
  });

  // ============================================================================
  // Autocomplete Modal Event Handlers
  // ============================================================================

  function handleAutocompleteClose(): void {
    showAutocomplete = false;
    currentQuery = '';

    // Return focus to the content editable element
    if (controller) {
      controller.focus();
    }
  }

  // ============================================================================
  // Utility Functions
  // ============================================================================

  // Compute CSS classes
  const containerClasses = $derived(
    [
      'node',
      `node--${nodeType}`,
      headerLevel > 0 && `node--h${headerLevel}`,
      children.length > 0 && 'node--has-children'
    ]
      .filter(Boolean)
      .join(' ')
  );

  // Convert string nodeType to typed NodeType for new icon system
  const typedNodeType = $derived(() => {
    // Ensure nodeType matches our NodeType union
    if (['text', 'document', 'task', 'ai-chat', 'ai_chat', 'user', 'entity', 'query'].includes(nodeType)) {
      return nodeType as NodeType;
    }
    // Fallback to text for unrecognized types
    return 'text' as NodeType;
  });
  const nodeColor = $derived(`hsl(var(--node-${nodeType}, var(--node-text)))`);
  
  // Note: Semantic classes handled internally by Icon component
</script>

<div class={containerClasses} data-node-id={nodeId}>
  <!-- Node indicator -->
  <div class="node__indicator">
    {#if children.length > 0}
      <!-- Text Node with Children (Ring Effect) -->
      <div class="node-icon">
        <div class="task-ring"></div>
        <div class="text-circle"></div>
      </div>
    {:else}
      <!-- Regular Text Node -->
      <div class="node-icon">
        <div class="text-circle"></div>
      </div>
    {/if}
  </div>

  <!-- Content area -->
  <div
    bind:this={contentEditableElement}
    contenteditable="true"
    class="node__content"
    id="contenteditable-{nodeId}"
    role="textbox"
    tabindex="0"
  ></div>
</div>

<!-- Professional Node Autocomplete Component -->
{#if nodeReferenceService}
  <NodeAutocomplete
    position={autocompletePosition}
    query={currentQuery}
    results={autocompleteResults}
    loading={autocompleteLoading}
    visible={showAutocomplete}
    on:select={handleAutocompleteSelect}
    on:close={handleAutocompleteClose}
  />
{/if}

<style>
  .node {
    position: relative;
    padding: 0.25rem;
    padding-left: calc(var(--circle-offset, 26px) + var(--circle-diameter, 20px) + var(--circle-text-gap, 8px)); /* Dynamic: circle position + circle width + gap */
    width: 100%;
    /* 
      Circle positioning system using CSS-first dynamic calculation
      
      MATHEMATICAL FOUNDATION:
      The circle needs to be positioned at the visual center of the first line of text.
      
      Formula: containerPaddingPx + (lineHeightPx / 2)
      - containerPaddingPx = 0.25rem (top padding of .node container)  
      - lineHeightPx = font-size × line-height multiplier
      
      DERIVATION:
      1. Text baseline starts at container top + padding (0.25rem)
      2. Line height creates vertical space around text
      3. Visual center is at baseline + (line-height / 2)
      4. Therefore: top = 0.25rem + (line-height-px / 2)
      
      BROWSER COMPATIBILITY:
      - CSS custom properties: IE 11+ (95%+ browser support)
      - CSS calc(): IE 9+ (98%+ browser support) 
      - Graceful fallback to 1.05rem for older browsers
    */

    /* Default values for normal text (1rem × 1.6 = 1.6rem line-height) */
    --line-height: 1.6;
    --font-size: 1rem;
    --line-height-px: calc(var(--font-size) * var(--line-height));
  }

  .node__indicator {
    position: absolute;
    left: var(--circle-offset, 26px); /* Dynamic circle positioning */
    /* Fallback positioning for older browsers that don't support CSS custom properties */
    top: 1.05rem; /* Approximates normal text center: 0.25rem padding + (1rem * 1.6 / 2) */
    /* CSS-first dynamic positioning using proven formula: containerPaddingPx + (lineHeightPx / 2) */
    top: calc(0.25rem + (var(--line-height-px) / 2));
    transform: translate(-50%, -50%); /* Center icon on coordinates */
    width: var(--circle-diameter, 20px); /* Dynamic circle size */
    height: var(--circle-diameter, 20px); /* Dynamic circle size */
    display: flex;
    align-items: center;
    justify-content: center;
    /* No transition - position updates should be instant */
  }
  
  /* Design system semantic classes for icon containers */
  .node-icon,
  .task-icon,
  .ai-icon {
    /* Classes applied dynamically based on node type */
    /* Base styling inherited from .node__indicator */
    position: relative;
  }

  .node__content {
    flex: 1;
    min-height: 1.25rem;
    padding: 0;
    border: none;
    background: transparent;
    font-family: inherit;
    font-size: 1rem;
    line-height: 1.6;
    color: hsl(var(--foreground));
    outline: none;
    white-space: pre-wrap;
    word-wrap: break-word;
  }

  .node__content:empty {
    /* Ensure empty nodes maintain their height and are clickable */
    min-height: 1.25rem;
    cursor: text;
  }

  /* Header styling with CSS variables for dynamic positioning */

  /* Shared header class - eliminates code duplication */
  .node--h1,
  .node--h2,
  .node--h3,
  .node--h4,
  .node--h5,
  .node--h6 {
    /* Calculate line-height in pixels for positioning formula */
    --line-height-px: calc(var(--font-size) * var(--line-height));
  }

  /* Shared header content styling */
  .node--h1 .node__content,
  .node--h2 .node__content,
  .node--h3 .node__content,
  .node--h4 .node__content,
  .node--h5 .node__content,
  .node--h6 .node__content {
    font-size: var(--font-size);
    font-weight: bold;
    line-height: var(--line-height);
  }

  /* Header-specific typography variables */
  .node--h1 {
    --font-size: 2rem;
    --line-height: 1.2;
  }

  .node--h2 {
    --font-size: 1.5rem;
    --line-height: 1.3;
  }

  .node--h3 {
    --font-size: 1.25rem;
    --line-height: 1.4;
  }

  .node--h4 {
    --font-size: 1.125rem;
    --line-height: 1.4;
  }

  .node--h5 {
    --font-size: 1rem;
    --line-height: 1.4;
  }

  .node--h6 {
    --font-size: 0.875rem;
    --line-height: 1.4;
  }

  /* Header empty state - ensure proper height */
  .node--h1 .node__content:empty,
  .node--h2 .node__content:empty,
  .node--h3 .node__content:empty,
  .node--h4 .node__content:empty,
  .node--h5 .node__content:empty,
  .node--h6 .node__content:empty {
    min-height: 1.5rem;
  }

  /* Markdown formatting styles */
  :global(.markdown-syntax) {
    /* Wrapper for consistent double-click selection behavior */
    display: inline;
    word-break: keep-all;
  }

  :global(.markdown-bold) {
    font-weight: 700;
    /* Make bold more visible in monospace */
    text-shadow: 0.5px 0 0 currentColor;
  }

  :global(.markdown-italic) {
    font-style: italic;
  }

  :global(.markdown-underline) {
    text-decoration: underline;
  }

  /* Node type extension colors */
  :global(.node) {
    --node-text: 142 71% 45%;
    --node-task: 25 95% 53%;
    --node-ai-chat: 221 83% 53%;
    --node-entity: 271 81% 56%;
    --node-query: 330 81% 60%;
  }
</style>
