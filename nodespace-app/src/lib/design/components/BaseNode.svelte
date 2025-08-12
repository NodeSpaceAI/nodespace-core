<!--
  Simplified Base Node Component
  
  Clean foundational visual pattern for all node types in NodeSpace.
  Simplified from ~400 lines to ~120-150 lines by removing over-engineering.
-->

<script context="module" lang="ts">
  // Node type for styling variants
  export type NodeType = 'text' | 'task' | 'ai-chat' | 'entity' | 'query';
</script>

<script lang="ts">
  import { createEventDispatcher, tick } from 'svelte';
  import Icon, { type IconName } from '../icons/index.js';
  import MockTextElement from './MockTextElement.svelte';
  import { findCharacterFromClickFast, isClickWithinTextBounds } from './CursorPositioning';

  // Essential Props (Simplified to 5 core + processing)
  export let nodeType: NodeType = 'text';
  export let nodeId: string = '';
  export let content: string = '';
  export let hasChildren: boolean = false;
  export let className: string = '';
  export let editable: boolean = true;
  export let contentEditable: boolean = true; // Can content be directly edited by user?
  export let multiline: boolean = false; // Single-line by default
  export let placeholder: string = 'Click to add content...';

  // SVG Icon System
  export let iconName: IconName | undefined = undefined;

  // Processing State System
  export let isProcessing: boolean = false;
  export let processingAnimation: 'blink' | 'pulse' | 'spin' | 'fade' = 'pulse';
  export let processingIcon: IconName | undefined = undefined;

  // Edit state
  let focused = false;
  let textareaElement: HTMLTextAreaElement;
  let mockElementRef: any;

  // Event dispatcher
  const dispatch = createEventDispatcher<{
    click: { nodeId: string; event: MouseEvent };
    focus: { nodeId: string };
    blur: { nodeId: string };
    contentChanged: { nodeId: string; content: string };
  }>();

  // Handle display click to start editing
  function handleDisplayClick(event: MouseEvent) {
    if (!editable || !contentEditable || focused) return;
    startEditing(event);
  }

  // Handle general node clicks
  function handleClick(event: MouseEvent) {
    dispatch('click', { nodeId, event });

    // If clicking on display content, delegate to handleDisplayClick
    if (!focused && editable && contentEditable) {
      const target = event.target as HTMLElement;
      const displayElement = target.closest('.ns-node__display--clickable');
      if (displayElement) {
        handleDisplayClick(event);
      }
    }
  }

  // Start editing mode
  async function startEditing(clickEvent?: MouseEvent) {
    focused = true;
    dispatch('focus', { nodeId });

    await tick();
    if (textareaElement) {
      // Auto-resize for multiline content on focus
      if (multiline) {
        autoResizeTextarea(textareaElement);
      }

      textareaElement.focus();

      // Position cursor based on click location
      if (clickEvent) {
        await tick(); // Wait for textarea to be fully rendered
        positionCursorFromClick(clickEvent);
      } else {
        // No click event, place at end
        textareaElement.setSelectionRange(
          textareaElement.value.length,
          textareaElement.value.length
        );
      }
    }
  }

  // Position cursor based on click coordinates using mock element
  async function positionCursorFromClick(clickEvent: MouseEvent) {
    if (!textareaElement || !mockElementRef) return;

    const textareaRect = textareaElement.getBoundingClientRect();

    // Validate click is within reasonable bounds
    if (!isClickWithinTextBounds(clickEvent.clientX, clickEvent.clientY, textareaRect)) {
      // Click is too far from text area, position at end
      const textLength = textareaElement.value.length;
      textareaElement.setSelectionRange(textLength, textLength);
      return;
    }

    console.log('Mock element positioning:', {
      content: content.substring(0, 20) + '...',
      clickX: clickEvent.clientX,
      clickY: clickEvent.clientY,
      textareaRect: {
        left: textareaRect.left,
        top: textareaRect.top,
        width: textareaRect.width,
        height: textareaRect.height
      },
      multiline: multiline
    });

    try {
      // Get the mock element for position calculation
      const mockElement = mockElementRef.getElement();
      if (!mockElement) {
        console.warn('Mock element not available, falling back to end position');
        const textLength = textareaElement.value.length;
        textareaElement.setSelectionRange(textLength, textLength);
        return;
      }

      // Use the fast positioning algorithm
      const positionResult = findCharacterFromClickFast(
        mockElement,
        clickEvent.clientX,
        clickEvent.clientY,
        textareaRect
      );

      // Set cursor position based on result
      const targetPosition = Math.max(
        0,
        Math.min(positionResult.index, textareaElement.value.length)
      );

      console.log('Mock element positioning result:', {
        targetPosition,
        accuracy: positionResult.accuracy,
        distance: positionResult.distance
      });

      textareaElement.setSelectionRange(targetPosition, targetPosition);

      // Verify the cursor was set correctly
      setTimeout(() => {
        console.log('Actual cursor position after set:', textareaElement.selectionStart);
      }, 10);
    } catch (error) {
      console.error('Error in mock element positioning:', error);
      // Fallback to end position
      const textLength = textareaElement.value.length;
      textareaElement.setSelectionRange(textLength, textLength);
    }
  }

  // Handle textarea input
  function handleInput(event: Event & { currentTarget: HTMLTextAreaElement }) {
    const target = event.currentTarget;

    // For single-line, replace newlines with spaces
    if (!multiline) {
      target.value = target.value.replace(/\n/g, ' ');
    }

    content = target.value;

    // Auto-resize only if multiline
    if (multiline) {
      autoResizeTextarea(target);
    }

    dispatch('contentChanged', { nodeId, content });
  }

  // Auto-resize textarea (only for multiline)
  function autoResizeTextarea(textarea: HTMLTextAreaElement) {
    textarea.style.height = 'auto';
    textarea.style.height = textarea.scrollHeight + 'px';
  }

  // Handle blur
  function handleBlur() {
    if (!focused) return;
    focused = false;
    dispatch('blur', { nodeId });
  }

  // Handle keyboard shortcuts
  function handleKeyDown(event: KeyboardEvent) {
    if (focused) {
      // When editing, handle Escape and Enter
      if (event.key === 'Escape') {
        event.preventDefault();
        focused = false;
        dispatch('blur', { nodeId });
        return;
      }

      // For single-line, Enter saves and exits
      if (!multiline && event.key === 'Enter' && !event.shiftKey) {
        event.preventDefault();
        focused = false;
        dispatch('blur', { nodeId });
        return;
      }

      // Other keys pass through naturally
      return;
    }

    // When not editing, handle Enter/Space to start editing
    if (editable && contentEditable && (event.code === 'Enter' || event.code === 'Space')) {
      event.preventDefault();
      startEditing();
    }
  }

  // Get node type label for accessibility
  function getNodeTypeLabel(type: NodeType): string {
    switch (type) {
      case 'text':
        return 'Text';
      case 'task':
        return 'Task';
      case 'ai-chat':
        return 'AI Chat';
      case 'entity':
        return 'Entity';
      case 'query':
        return 'Query';
      default:
        return 'Node';
    }
  }

  // CSS classes
  $: nodeClasses = [
    'ns-node',
    `ns-node--${nodeType}`,
    hasChildren && 'ns-node--has-children',
    isProcessing && 'ns-node--processing',
    className
  ]
    .filter(Boolean)
    .join(' ');

  // Icon selection: processing icon overrides regular icon when processing
  $: displayIcon = isProcessing && processingIcon ? processingIcon : iconName;

  // Animation class for processing state
  $: iconAnimationClass = isProcessing ? `ns-node__icon--${processingAnimation}` : '';
</script>

<!-- Simplified node container - div approach to avoid button/contenteditable conflicts -->
<div
  class={nodeClasses}
  role="button"
  tabindex="0"
  aria-label="{getNodeTypeLabel(nodeType)}: {content || 'Empty node'}"
  data-node-id={nodeId}
  data-node-type={nodeType}
  on:click={handleClick}
  on:keydown={handleKeyDown}
>
  <!-- Node header -->
  <header class="ns-node__header">
    <!-- Icon or Circle Indicator -->
    <div class="ns-node__indicator" data-node-type={nodeType}>
      {#if displayIcon}
        <Icon name={displayIcon} size={16} className="ns-node__icon {iconAnimationClass}" />
      {:else}
        <!-- Circle fallback indicator -->
        <div
          class="ns-node__circle {iconAnimationClass}"
          class:ns-node__circle--has-children={hasChildren}
        ></div>
      {/if}
    </div>

    <!-- Node content with edit/display logic -->
    <div class="ns-node__content">
      <slot name="content">
        {#if focused && contentEditable}
          <!-- Edit mode (only if content is directly editable) -->
          <textarea
            bind:this={textareaElement}
            class="ns-node__textarea {multiline
              ? 'ns-node__textarea--multiline'
              : 'ns-node__textarea--single'}"
            bind:value={content}
            on:input={handleInput}
            on:blur={handleBlur}
            on:keydown={handleKeyDown}
            on:keydown|stopPropagation
            {placeholder}
            rows={1}
          ></textarea>

          <!-- Mock element for precise cursor positioning -->
          {#if textareaElement}
            <!-- @ts-expect-error: Svelte component binding inference issue -->
            <MockTextElement
              bind:this={mockElementRef}
              {content}
              fontFamily={getComputedStyle(textareaElement).fontFamily}
              fontSize={getComputedStyle(textareaElement).fontSize}
              fontWeight={getComputedStyle(textareaElement).fontWeight}
              lineHeight={getComputedStyle(textareaElement).lineHeight}
              width={textareaElement.offsetWidth}
              {multiline}
            />
          {/if}
        {:else}
          <!-- Display mode -->
          {#if editable && contentEditable}
            <div
              class="ns-node__display ns-node__display--clickable"
              role="button"
              tabindex="0"
              on:click={handleDisplayClick}
              on:keydown={(e) => {
                if (e.key === 'Enter' || e.key === ' ') {
                  e.preventDefault();
                  startEditing();
                }
              }}
              aria-label="Click to edit content"
            >
              <slot name="display-content">
                {#if content}
                  <span class="ns-node__text">{content}</span>
                {:else}
                  <span class="ns-node__empty">{placeholder}</span>
                {/if}
              </slot>
            </div>
          {:else}
            <!-- Non-editable display -->
            <div class="ns-node__display" role="region">
              <slot name="display-content">
                {#if content}
                  <span class="ns-node__text">{content}</span>
                {:else}
                  <span class="ns-node__empty">{placeholder}</span>
                {/if}
              </slot>
            </div>
          {/if}
        {/if}
      </slot>

      <!-- Additional node-specific content -->
      <slot></slot>
    </div>
  </header>
</div>

<style>
  /* Simplified Base Node Styling */
  .ns-node {
    /* Layout */
    display: flex;
    width: 100%;
    padding: 12px;
    margin: 4px 0;

    /* Styling */
    background-color: transparent;
    border: none;
    border-radius: 8px;
    cursor: text;
    outline: none;
    transition: all 200ms ease;

    /* Text styling */
    font: inherit;
    text-align: left;
  }

  /* Hover state - minimal for clean appearance - no styles needed */

  /* Focus state */
  .ns-node:focus-visible {
    outline: 2px solid hsl(var(--ring));
    outline-offset: 2px;
  }

  /* Processing state - indication through circle animation, no border styles needed */

  /* Header layout */
  .ns-node__header {
    display: flex;
    align-items: center;
    gap: 12px;
    width: 100%;
  }

  /* Indicator container */
  .ns-node__indicator {
    flex-shrink: 0;
    width: 16px;
    height: 16px;
    display: flex;
    align-items: center;
    justify-content: center;
  }

  /* SVG Icon styling */
  .ns-node__icon {
    color: currentColor;
  }

  /* Circle fallback indicator */
  .ns-node__circle {
    width: 10px;
    height: 10px;
    border-radius: 50%;
    position: relative;
  }

  /* Parent nodes get a ring effect */
  .ns-node__circle--has-children::before {
    content: '';
    position: absolute;
    top: -3px;
    left: -3px;
    right: -3px;
    bottom: -3px;
    border-radius: 50%;
    border: 1px solid currentColor;
    opacity: 0.3;
  }

  /* Node type colors for circles */
  .ns-node--text .ns-node__circle {
    background-color: hsl(142 71% 45%);
  }
  .ns-node--task .ns-node__circle {
    background-color: hsl(25 95% 53%);
  }
  .ns-node--ai-chat .ns-node__circle {
    background-color: hsl(221 83% 53%);
  }
  .ns-node--entity .ns-node__circle {
    background-color: hsl(271 81% 56%);
  }
  .ns-node--query .ns-node__circle {
    background-color: hsl(330 81% 60%);
  }

  /* Content section */
  .ns-node__content {
    flex: 1;
    min-width: 0; /* Prevent overflow */
  }

  /* Textarea styling */
  .ns-node__textarea {
    width: 100%;
    min-height: 20px;
    padding: 0;
    border: none;
    border-radius: 0;
    font-family: inherit;
    font-size: 14px;
    line-height: 1.4;
    background: transparent;
    color: hsl(var(--foreground));
    outline: none;
    resize: none;
    overflow: hidden;
  }

  /* Single-line textarea (default) */
  .ns-node__textarea--single {
    height: 20px;
    white-space: nowrap;
    overflow-x: auto;
    overflow-y: hidden;
  }

  /* Multi-line textarea */
  .ns-node__textarea--multiline {
    min-height: 20px;
    white-space: pre-wrap;
    overflow-y: auto;
  }

  .ns-node__textarea::placeholder {
    color: hsl(var(--muted-foreground));
    font-style: italic;
  }

  /* Display styling */
  .ns-node__display {
    min-height: 20px;
  }

  .ns-node__display--clickable {
    cursor: text;
  }

  .ns-node__text {
    color: hsl(var(--foreground));
    font-family: inherit;
    font-size: 14px;
    line-height: 1.4;
    word-break: break-word;
  }

  .ns-node__empty {
    color: hsl(var(--muted-foreground));
    font-size: 14px;
    font-style: italic;
  }

  /* Processing Animations */
  .ns-node__icon--pulse,
  .ns-node__circle--pulse {
    animation: pulse 2s infinite;
  }

  .ns-node__icon--blink,
  .ns-node__circle--blink {
    animation: blink 1s infinite;
  }

  .ns-node__icon--spin,
  .ns-node__circle--spin {
    animation: spin 1s linear infinite;
  }

  .ns-node__icon--fade,
  .ns-node__circle--fade {
    animation: fade 2s infinite;
  }

  /* Animation keyframes */
  @keyframes pulse {
    0%,
    100% {
      opacity: 1;
      transform: scale(1);
    }
    50% {
      opacity: 0.7;
      transform: scale(1.1);
    }
  }

  @keyframes blink {
    0%,
    50% {
      opacity: 1;
    }
    51%,
    100% {
      opacity: 0.3;
    }
  }

  @keyframes spin {
    from {
      transform: rotate(0deg);
    }
    to {
      transform: rotate(360deg);
    }
  }

  @keyframes fade {
    0%,
    100% {
      opacity: 1;
    }
    50% {
      opacity: 0.4;
    }
  }

  /* Responsive adjustments */
  @media (max-width: 640px) {
    .ns-node {
      padding: 8px;
      margin: 2px 0;
    }

    .ns-node__header {
      gap: 8px;
    }

    .ns-node__text {
      font-size: 13px;
    }
  }
</style>
