<!--
  Clean BaseNode for ContentProcessor Integration
  
  Built specifically for dual-representation with focus/blur markdown syntax
  visibility. Uses ContentEditableController to eliminate reactive conflicts.
-->

<script lang="ts">
  import { createEventDispatcher, onDestroy } from 'svelte';
  import Icon, { type IconName } from '$lib/design/icons';
  import {
    ContentEditableController,
    type ContentEditableEvents
  } from './ContentEditableController.js';
  import MockTextElement from './MockTextElement.svelte';
  import { findCharacterFromClickFast, isClickWithinTextBounds } from './CursorPositioning.js';

  // Props (Svelte 5 runes syntax)
  let {
    nodeId,
    nodeType = 'text',
    autoFocus = false,
    content = '',
    headerLevel = 0,
    children = []
  }: {
    nodeId: string;
    nodeType?: string;
    autoFocus?: boolean;
    content?: string;
    headerLevel?: number;
    children?: unknown[];
  } = $props();

  // DOM element and controller
  let contentEditableElement: HTMLDivElement;
  let controller: ContentEditableController | null = null;

  // Mock text element for cursor positioning
  let mockTextElement: MockTextElement;

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
  }>();

  // Controller event handlers
  const controllerEvents: ContentEditableEvents = {
    contentChanged: (content: string) => dispatch('contentChanged', { content }),
    headerLevelChanged: (level: number) => dispatch('headerLevelChanged', { level }),
    focus: () => dispatch('focus'),
    blur: () => dispatch('blur'),
    createNewNode: (data) => dispatch('createNewNode', data),
    indentNode: (data) => dispatch('indentNode', data),
    outdentNode: (data) => dispatch('outdentNode', data),
    navigateArrow: (data) => dispatch('navigateArrow', data),
    combineWithPrevious: (data) => dispatch('combineWithPrevious', data),
    deleteNode: (data) => dispatch('deleteNode', data)
  };

  // Initialize controller when element is available (Svelte 5 $effect)
  // Note: Must explicitly access contentEditableElement to track dependency
  $effect(() => {
    const element = contentEditableElement; // Force dependency tracking
    if (element && !controller) {
      controller = new ContentEditableController(element, nodeId, controllerEvents);
      controller.initialize(content, autoFocus);
      
      // Add click event listener for cursor positioning
      element.addEventListener('click', handleClick);
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

  // Update mock element width when content element dimensions change
  let mockElementWidth = $state(0);
  $effect(() => {
    if (contentEditableElement) {
      const updateWidth = () => {
        mockElementWidth = contentEditableElement.offsetWidth;
      };
      
      // Update immediately
      updateWidth();
      
      // Set up resize observer for dynamic updates
      const resizeObserver = new ResizeObserver(updateWidth);
      resizeObserver.observe(contentEditableElement);
      
      return () => {
        resizeObserver.disconnect();
      };
    }
  });

  onDestroy(() => {
    if (controller) {
      controller.destroy();
    }
    if (contentEditableElement) {
      contentEditableElement.removeEventListener('click', handleClick);
    }
  });

  // Click handler for cursor positioning
  function handleClick(event: MouseEvent) {
    if (!contentEditableElement || !controller || !mockTextElement) {
      return;
    }

    // Get the mock element
    const mockElement = mockTextElement.getElement();
    if (!mockElement) {
      return;
    }

    // Get click coordinates
    const clickX = event.clientX;
    const clickY = event.clientY;

    // Get editable element bounds
    const editableRect = contentEditableElement.getBoundingClientRect();

    // Check if click is within reasonable bounds
    if (!isClickWithinTextBounds(clickX, clickY, editableRect)) {
      return;
    }

    // Find character position using the cursor positioning utility
    const positionResult = findCharacterFromClickFast(mockElement, clickX, clickY, editableRect);

    // Set cursor position using the controller
    if (controller && positionResult.index >= 0) {
      // Focus the element first
      controller.focus();
      
      // Use a timeout to ensure the element is focused before setting cursor
      setTimeout(() => {
        if (controller) {
          controller.setCursorPosition(positionResult.index);
        }
      }, 0);
    }
  }

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

  // Icon for node type
  const icon = $derived((children.length > 0 ? 'circle-ring' : 'circle') as IconName);
  const nodeColor = $derived(`hsl(var(--node-${nodeType}, var(--node-text)))`);
</script>

<div class={containerClasses} data-node-id={nodeId}>
  <!-- Node indicator -->
  <div class="node__indicator">
    <Icon name={icon} size={16} color={nodeColor} />
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

  <!-- Hidden mock text element for cursor positioning -->
  <MockTextElement
    bind:this={mockTextElement}
    content={content}
    fontFamily="inherit"
    fontSize={headerLevel > 0 ? `${headerLevel === 1 ? '2' : headerLevel === 2 ? '1.5' : headerLevel === 3 ? '1.25' : headerLevel === 4 ? '1.125' : '1'}rem` : '1rem'}
    fontWeight={headerLevel > 0 ? 'bold' : 'normal'}
    lineHeight={headerLevel > 0 ? (headerLevel === 1 ? '1.2' : headerLevel === 2 ? '1.3' : '1.4') : '1.6'}
    width={mockElementWidth}
    multiline={true}
  />
</div>

<style>
  .node {
    position: relative;
    padding: 0.25rem;
    padding-left: calc(0.25rem + 20px); /* Original padding + space for indicator */
    width: 100%;
  }

  .node__indicator {
    position: absolute;
    left: 8px; /* Center X coordinate (half of 16px icon width) */
    /* Precise vertical centering: container padding + empirically corrected visual text center */
    top: calc(
      0.25rem + var(--text-visual-center, calc(0.816em + var(--baseline-correction, -0.15em)))
    );
    transform: translate(-50%, -50%); /* Center icon on coordinates */
    width: 16px;
    height: 16px;
    display: flex;
    align-items: center;
    justify-content: center;
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

  /* Header styling with empirically corrected visual text center for pixel-perfect circle alignment */
  .node {
    /* Base correction factor adjusted based on precise Playwright measurements - circles positioned exactly at text visual center */
    --baseline-correction: -0.06375em; /* Refined: original -0.12em + 0.05625em to move circles UP by measured 0.9px on 16px text */
  }

  .node--h1 {
    /* 2rem * 1.2 * 0.5 + baseline correction = true visual center for H1 (corrected to move circles UP by measured 1.7px) */
    --text-visual-center: calc(1.2em + var(--baseline-correction) + 0.053125em);
  }

  .node--h1 .node__content {
    font-size: 2rem;
    font-weight: bold;
    line-height: 1.2;
  }

  .node--h2 {
    /* 1.5rem * 1.3 * 0.5 + baseline correction = true visual center for H2 (corrected to move circles UP by measured 1.3px) */
    --text-visual-center: calc(0.975em + var(--baseline-correction) + 0.0542em);
  }

  .node--h2 .node__content {
    font-size: 1.5rem;
    font-weight: bold;
    line-height: 1.3;
  }

  .node--h3 {
    /* 1.25rem * 1.4 * 0.5 + baseline correction = true visual center for H3 (fine-tuned for perfect alignment - additional 0.03em correction) */
    --text-visual-center: calc(0.875em + var(--baseline-correction) + 0.1em);
  }

  .node--h3 .node__content {
    font-size: 1.25rem;
    font-weight: bold;
    line-height: 1.4;
  }

  .node--h4 {
    /* 1.125rem * 1.4 * 0.5 + baseline correction = true visual center for H4 */
    --text-visual-center: calc(0.7875em + var(--baseline-correction));
  }

  .node--h4 .node__content {
    font-size: 1.125rem;
    font-weight: bold;
    line-height: 1.4;
  }

  .node--h5 {
    /* 1rem * 1.4 * 0.5 + baseline correction = true visual center for H5 */
    --text-visual-center: calc(0.7em + var(--baseline-correction));
  }

  .node--h5 .node__content {
    font-size: 1rem;
    font-weight: bold;
    line-height: 1.4;
  }

  .node--h6 {
    /* 0.875rem * 1.4 * 0.5 + baseline correction = true visual center for H6 */
    --text-visual-center: calc(0.6125em + var(--baseline-correction));
  }

  .node--h6 .node__content {
    font-size: 0.875rem;
    font-weight: bold;
    line-height: 1.4;
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
