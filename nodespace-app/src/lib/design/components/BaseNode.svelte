<!--
  Clean BaseNode for ContentProcessor Integration
  
  Built specifically for dual-representation with focus/blur markdown syntax
  visibility. Uses ContentEditableController to eliminate reactive conflicts.
-->

<script lang="ts">
  import { createEventDispatcher, onDestroy } from 'svelte';
  import Icon from '$lib/design/icons';
  import { ContentEditableController, type ContentEditableEvents } from './ContentEditableController.js';

  // Props
  export let nodeId: string;
  export let nodeType: string = 'text';
  export let autoFocus: boolean = false;
  export let content: string = '';
  export let headerLevel: number = 0;
  export let children: any[] = [];

  // DOM element and controller
  let contentEditableElement: HTMLDivElement;
  let controller: ContentEditableController | null = null;

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

  // Initialize controller when element is available
  $: if (contentEditableElement && !controller) {
    controller = new ContentEditableController(
      contentEditableElement,
      nodeId,
      controllerEvents
    );
    controller.initialize(content, autoFocus);
  }

  // Update content when prop changes
  $: if (controller && content !== undefined) {
    controller.updateContent(content);
  }

  // Focus programmatically when autoFocus changes
  $: if (controller && autoFocus) {
    controller.focus();
  }

  onDestroy(() => {
    if (controller) {
      controller.destroy();
    }
  });

  // Compute CSS classes
  $: containerClasses = [
    'node',
    `node--${nodeType}`,
    headerLevel > 0 && `node--h${headerLevel}`,
    children.length > 0 && 'node--has-children'
  ].filter(Boolean).join(' ');

  // Icon for node type
  $: icon = children.length > 0 ? 'circle-ring' : 'circle';
  $: nodeColor = `hsl(var(--node-${nodeType}, var(--node-text)))`;
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
</div>

<style>
  .node {
    display: flex;
    align-items: flex-start;
    gap: 0.25rem;
    padding: 0.25rem;
    width: 100%;
  }

  .node__indicator {
    flex-shrink: 0;
    margin-top: 0.28rem;
    display: flex;
    align-items: center;
    justify-content: center;
    width: 20px;
    height: 20px;
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

  .node__content:empty::before {
    content: '';
    display: inline-block;
    width: 1px;
    height: 1em;
    background-color: hsl(var(--muted-foreground) / 0.3);
    cursor: text;
  }

  /* Header styling */
  .node--h1 .node__content {
    font-size: 2rem;
    font-weight: bold;
    line-height: 1.2;
  }

  .node--h2 .node__content {
    font-size: 1.5rem;
    font-weight: bold;
    line-height: 1.3;
  }

  .node--h3 .node__content {
    font-size: 1.25rem;
    font-weight: bold;
    line-height: 1.4;
  }

  .node--h4 .node__content {
    font-size: 1.125rem;
    font-weight: bold;
    line-height: 1.4;
  }

  .node--h5 .node__content {
    font-size: 1rem;
    font-weight: bold;
    line-height: 1.4;
  }

  .node--h6 .node__content {
    font-size: 0.875rem;
    font-weight: bold;
    line-height: 1.4;
  }

  /* Header empty state cursor fix */
  .node--h1 .node__content:empty::before,
  .node--h2 .node__content:empty::before,
  .node--h3 .node__content:empty::before,
  .node--h4 .node__content:empty::before,
  .node--h5 .node__content:empty::before,
  .node--h6 .node__content:empty::before {
    height: 1.25rem !important;
    background-color: hsl(var(--muted-foreground) / 0.2) !important;
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