<!--
  TextNode Component
  
  Extends BaseNode with text-specific functionality and inline editing.
  Supports click-to-edit, keyboard shortcuts, auto-save, and basic markdown rendering.
-->

<script lang="ts">
  import { createEventDispatcher, onMount, tick } from 'svelte';
  import BaseNode from '$lib/design/components/BaseNode.svelte';
  import {
    mockTextService,
    type TextNodeData,
    type TextSaveResult
  } from '$lib/services/mockTextService';
  import { parseMarkdown, stripMarkdown, validateMarkdown } from '$lib/services/markdownUtils';

  // Props
  export let nodeId: string;
  export let title: string = '';
  export let content: string = '';
  export let editable: boolean = true;
  export let markdown: boolean = true;
  export let placeholder: string = 'Click to add text...';
  export let autoSave: boolean = true;
  export let compact: boolean = false; // New prop for responsive layout

  // Component state
  let isEditing = false;
  let editContent = content;
  let editTitle = title;
  let saveStatus: 'saved' | 'saving' | 'unsaved' | 'error' = 'saved';
  let saveError = '';
  let contentEditableElement: any;
  let textNodeData: TextNodeData | null = null;
  let lastSavedContent = content;
  let lastSavedTitle = title;

  // Event dispatcher
  const dispatch = createEventDispatcher<{
    save: { nodeId: string; content: string; title: string };
    cancel: { nodeId: string };
    error: { nodeId: string; error: string };
    focus: { nodeId: string };
    blur: { nodeId: string };
  }>();

  // Load existing node data on mount if nodeId provided
  onMount(async () => {
    if (nodeId && nodeId !== 'new') {
      try {
        textNodeData = await mockTextService.loadTextNode(nodeId);
        if (textNodeData) {
          content = textNodeData.content;
          title = textNodeData.title;
          editContent = content;
          editTitle = title;
          lastSavedContent = content;
          lastSavedTitle = title;
          saveStatus = 'saved';
        }
      } catch (error) {
        console.error('Failed to load text node:', error);
        saveError = error instanceof Error ? error.message : 'Failed to load content';
        saveStatus = 'error';
      }
    }
  });

  // Debounced auto-save functionality
  let debounceTimeout: NodeJS.Timeout;
  const DEBOUNCE_DELAY = 400; // 400ms debounce for better UX

  function debounceAutoSave() {
    if (!autoSave || !isEditing) return;

    clearTimeout(debounceTimeout);
    saveStatus = 'unsaved';

    debounceTimeout = setTimeout(async () => {
      // Only save if content has actually changed
      if (editContent !== lastSavedContent || editTitle !== lastSavedTitle) {
        await handleAutoSave();
      }
    }, DEBOUNCE_DELAY);
  }

  // Handle click-to-edit with cursor positioning
  async function handleClick(event: MouseEvent) {
    if (!editable || isEditing) return;

    await startEditing(event);
  }

  // Start editing mode with cursor positioning
  async function startEditing(clickEvent?: MouseEvent) {
    isEditing = true;
    editContent = content;
    editTitle = title;
    saveStatus = content === lastSavedContent && title === lastSavedTitle ? 'saved' : 'unsaved';

    // Focus the contenteditable element after DOM update
    await tick();
    if (contentEditableElement) {
      contentEditableElement.focus();

      // Position cursor based on click location or default to start
      if (clickEvent) {
        try {
          // Calculate approximate cursor position based on click
          const rect = contentEditableElement.getBoundingClientRect();
          const clickX = clickEvent.clientX - rect.left;
          const clickY = clickEvent.clientY - rect.top;

          // Simple heuristic: if clicking in the right half, place cursor at end
          const cursorAtEnd = clickX > rect.width / 2 || clickY > rect.height / 2;

          const range = document.createRange();
          const selection = window.getSelection();

          if (contentEditableElement.firstChild) {
            if (cursorAtEnd) {
              range.setStart(
                contentEditableElement.firstChild,
                contentEditableElement.textContent?.length || 0
              );
            } else {
              range.setStart(contentEditableElement.firstChild, 0);
            }
            range.collapse(true);
          } else {
            range.selectNodeContents(contentEditableElement);
            range.collapse(true);
          }

          selection?.removeAllRanges();
          selection?.addRange(range);
        } catch {
          // Fallback: place cursor at start
          const range = document.createRange();
          range.selectNodeContents(contentEditableElement);
          range.collapse(true);
          const selection = window.getSelection();
          selection?.removeAllRanges();
          selection?.addRange(range);
        }
      } else {
        // No click event, default to start
        const range = document.createRange();
        range.selectNodeContents(contentEditableElement);
        range.collapse(true);
        const selection = window.getSelection();
        selection?.removeAllRanges();
        selection?.addRange(range);
      }
    }

    dispatch('focus', { nodeId });
  }

  // Cancel editing
  function cancelEditing() {
    if (!isEditing) return;

    // Clear debounce timeout
    clearTimeout(debounceTimeout);
    if (autoSave && nodeId) {
      mockTextService.cancelAutoSave(nodeId);
    }

    // Revert changes
    editContent = lastSavedContent;
    editTitle = lastSavedTitle;
    isEditing = false;
    saveStatus = 'saved';
    saveError = '';

    dispatch('cancel', { nodeId });
  }

  // Save changes
  async function saveChanges(): Promise<void> {
    if (!isEditing) return;

    saveStatus = 'saving';
    saveError = '';

    try {
      // Validate markdown if enabled
      if (markdown) {
        const validation = validateMarkdown(editContent);
        if (!validation.isValid) {
          saveError = `Markdown errors: ${validation.errors.join(', ')}`;
          saveStatus = 'error';
          return;
        }
      }

      let result: TextSaveResult;

      if (nodeId && nodeId !== 'new') {
        // Update existing node
        result = await mockTextService.saveTextNode(nodeId, editContent, editTitle);
      } else {
        // Create new node
        const newNodeData = await mockTextService.createTextNode(editContent, editTitle);
        nodeId = newNodeData.id;
        result = { success: true, id: newNodeData.id, timestamp: new Date() };
      }

      if (result.success) {
        // Update local state
        content = editContent;
        title = editTitle;
        lastSavedContent = editContent;
        lastSavedTitle = editTitle;
        saveStatus = 'saved';
        isEditing = false;

        // Update node data
        textNodeData = await mockTextService.loadTextNode(nodeId);

        dispatch('save', { nodeId, content: editContent, title: editTitle });
      } else {
        saveError = result.error || 'Save failed';
        saveStatus = 'error';
        dispatch('error', { nodeId, error: saveError });
      }
    } catch (error) {
      saveError = error instanceof Error ? error.message : 'Unknown error';
      saveStatus = 'error';
      dispatch('error', { nodeId, error: saveError });
    }
  }

  // Handle contenteditable input with debounced saving
  function handleContentInput(event: any) {
    const target = event.target as any;
    editContent = target.textContent || '';
    debounceAutoSave();
  }

  // Auto-save handler
  async function handleAutoSave() {
    if (!autoSave || !nodeId || nodeId === 'new') return;

    saveStatus = 'saving';

    try {
      const result = await mockTextService.scheduleAutoSave(nodeId, editContent, editTitle, 100);

      if (result.success) {
        lastSavedContent = editContent;
        lastSavedTitle = editTitle;
        saveStatus = 'saved';

        // Update displayed content
        content = editContent;
        title = editTitle;
      } else {
        saveError = result.error || 'Auto-save failed';
        saveStatus = 'error';
      }
    } catch (error) {
      saveError = error instanceof Error ? error.message : 'Auto-save error';
      saveStatus = 'error';
    }
  }

  // Handle keyboard shortcuts
  function handleKeyDown(event: KeyboardEvent) {
    if (!isEditing) return;

    // Ctrl+Enter or Cmd+Enter to save and exit edit mode
    if ((event.ctrlKey || event.metaKey) && event.key === 'Enter') {
      event.preventDefault();
      saveChanges();
      return;
    }

    // Escape to cancel
    if (event.key === 'Escape') {
      event.preventDefault();
      cancelEditing();
      return;
    }
  }

  // Handle blur (save on blur if auto-save is disabled)
  async function handleBlur() {
    if (!isEditing) return;

    dispatch('blur', { nodeId });

    // If auto-save is disabled, save on blur
    if (!autoSave) {
      await saveChanges();
    }
  }

  // Render markdown content
  $: renderedContent = markdown && content ? parseMarkdown(content) : content;

  // Get display title (fallback to content preview)
  $: displayTitle =
    title ||
    (content
      ? stripMarkdown(content).substring(0, 50) + (content.length > 50 ? '...' : '')
      : 'Untitled');

  // Save status indicator
  $: saveStatusClass = {
    saved: 'ns-text-node-status--saved',
    saving: 'ns-text-node-status--saving',
    unsaved: 'ns-text-node-status--unsaved',
    error: 'ns-text-node-status--error'
  }[saveStatus];
</script>

<BaseNode
  {nodeId}
  nodeType="text"
  content={displayTitle}
  hasChildren={textNodeData?.metadata.hasChildren || false}
  isProcessing={saveStatus === 'saving'}
  iconName="text"
  className="ns-text-node {isEditing ? 'ns-text-node--editing' : ''} {compact
    ? 'ns-text-node--compact'
    : ''}"
>
  <!-- Main content in default slot -->
  <div class="ns-text-node__content">
    {#if isEditing}
      <!-- Editing mode - contenteditable with raw markdown -->
      <div class="ns-text-node__editor">
        <div
          bind:this={contentEditableElement}
          class="ns-text-node__contenteditable"
          contenteditable="true"
          role="textbox"
          tabindex="0"
          aria-multiline="true"
          on:input={handleContentInput}
          on:keydown={handleKeyDown}
          on:blur={handleBlur}
          aria-label="Edit text content"
          data-placeholder={placeholder}
        >
          {editContent}
        </div>

        {#if markdown}
          <div class="ns-text-node__hint">
            Tip: Use **bold**, *italic*, # headers • Press Ctrl+Enter to save • Esc to cancel
          </div>
        {/if}
      </div>
    {:else}
      <!-- Display mode - rendered markdown or plain text -->
      {#if editable}
        <div
          class="ns-text-node__display ns-text-node__display--clickable"
          role="button"
          tabindex="0"
          on:click={handleClick}
          on:keydown={(e) => {
            if (e.key === 'Enter' || e.key === ' ') {
              e.preventDefault();
              handleClick(new MouseEvent('click'));
            }
          }}
          aria-label="Click to edit text content"
        >
          {#if content}
            {#if markdown}
              <div class="ns-text-node__markdown">
                <!-- 
                  ESLint suppression approved: This is the ONLY exception in the codebase.
                  Safe because: HTML is generated from controlled markdown parsing with HTML escaping.
                  Follows industry standard: GitHub, React Markdown, etc. use same approach.
                  See CLAUDE.md for lint suppression policy.
                -->
                <!-- eslint-disable-next-line svelte/no-at-html-tags -->
                {@html renderedContent}
              </div>
            {:else}
              <div class="ns-text-node__plain">
                {content}
              </div>
            {/if}
          {:else}
            <div class="ns-text-node__empty">
              {placeholder}
            </div>
          {/if}
        </div>
      {:else}
        <div class="ns-text-node__display" role="region">
          {#if content}
            {#if markdown}
              <div class="ns-text-node__markdown">
                <!-- 
                  ESLint suppression approved: This is the ONLY exception in the codebase.
                  Safe because: HTML is generated from controlled markdown parsing with HTML escaping.
                  Follows industry standard: GitHub, React Markdown, etc. use same approach.
                  See CLAUDE.md for lint suppression policy.
                -->
                <!-- eslint-disable-next-line svelte/no-at-html-tags -->
                {@html renderedContent}
              </div>
            {:else}
              <div class="ns-text-node__plain">
                {content}
              </div>
            {/if}
          {:else}
            <div class="ns-text-node__empty">
              {placeholder}
            </div>
          {/if}
        </div>
      {/if}
    {/if}
  </div>

  <!-- Status info inline with content -->
  {#if saveStatus !== 'saved' || saveError}
    <div class="ns-text-node__status {saveStatusClass}">
      {#if saveStatus === 'saving'}
        <span class="ns-text-node__status-icon">⏳</span>
        Saving...
      {:else if saveStatus === 'unsaved'}
        <span class="ns-text-node__status-icon">●</span>
        Unsaved changes
      {:else if saveStatus === 'error'}
        <span class="ns-text-node__status-icon">⚠️</span>
        {saveError || 'Save error'}
      {/if}
    </div>
  {:else if textNodeData && !isEditing}
    <div class="ns-text-node__meta">
      <span class="ns-text-node__word-count">
        {textNodeData.metadata.wordCount} words
      </span>
      <span class="ns-text-node__updated">
        Updated {textNodeData.updatedAt.toLocaleDateString()}
      </span>
    </div>
  {/if}
</BaseNode>

<style>
  /* Text node content styling */
  .ns-text-node__content {
    width: 100%;
    min-height: 60px;
  }

  /* Editor styling */
  .ns-text-node__editor {
    width: 100%;
  }

  .ns-text-node__contenteditable {
    width: 100%;
    min-height: 80px;
    padding: var(--ns-spacing-3);
    border: 1px solid var(--ns-color-border-default);
    border-radius: var(--ns-radius-md);
    font-family: inherit;
    font-size: var(--ns-font-size-sm);
    line-height: var(--ns-line-height-relaxed);
    background: var(--ns-color-surface-default);
    color: var(--ns-color-text-primary);
    outline: none;
    white-space: pre-wrap;
    word-break: break-word;
    transition: border-color var(--ns-duration-fast) var(--ns-easing-easeInOut);
  }

  .ns-text-node__contenteditable:focus {
    border-color: var(--ns-color-primary-500);
  }

  .ns-text-node__contenteditable:empty::before {
    content: attr(data-placeholder);
    color: var(--ns-color-text-placeholder);
    font-style: italic;
  }

  .ns-text-node__hint {
    font-size: var(--ns-font-size-xs);
    color: var(--ns-color-text-tertiary);
    margin-top: var(--ns-spacing-2);
    text-align: center;
    padding: var(--ns-spacing-1);
    background: var(--ns-color-surface-panel);
    border-radius: var(--ns-radius-sm);
  }

  /* Display mode styling */
  .ns-text-node__display {
    min-height: 40px;
  }

  .ns-text-node__display--clickable {
    cursor: pointer;
  }

  .ns-text-node__display--clickable:hover {
    background: var(--ns-color-surface-panel);
    border-radius: var(--ns-radius-sm);
    transition: background-color var(--ns-duration-fast) var(--ns-easing-easeInOut);
  }

  .ns-text-node__plain {
    white-space: pre-wrap;
    word-break: break-word;
    font-size: var(--ns-font-size-sm);
    line-height: var(--ns-line-height-relaxed);
    color: var(--ns-color-text-primary);
  }

  .ns-text-node__empty {
    font-size: var(--ns-font-size-sm);
    color: var(--ns-color-text-placeholder);
    font-style: italic;
  }

  /* Markdown rendering */
  .ns-text-node__markdown {
    font-size: var(--ns-font-size-sm);
    line-height: var(--ns-line-height-relaxed);
    color: var(--ns-color-text-primary);
  }

  /* Markdown element styling */
  :global(.ns-markdown-heading) {
    margin: var(--ns-spacing-2) 0 var(--ns-spacing-1) 0;
    font-weight: var(--ns-font-weight-semibold);
    color: var(--ns-color-text-primary);
  }

  :global(.ns-markdown-h1) {
    font-size: var(--ns-font-size-xl);
  }
  :global(.ns-markdown-h2) {
    font-size: var(--ns-font-size-lg);
  }
  :global(.ns-markdown-h3) {
    font-size: var(--ns-font-size-base);
  }
  :global(.ns-markdown-h4) {
    font-size: var(--ns-font-size-sm);
  }
  :global(.ns-markdown-h5) {
    font-size: var(--ns-font-size-sm);
  }
  :global(.ns-markdown-h6) {
    font-size: var(--ns-font-size-xs);
  }

  :global(.ns-markdown-paragraph) {
    margin: var(--ns-spacing-2) 0;
  }

  :global(.ns-markdown-bold) {
    font-weight: var(--ns-font-weight-semibold);
  }

  :global(.ns-markdown-italic) {
    font-style: italic;
  }

  :global(.ns-markdown-code) {
    padding: var(--ns-spacing-1) calc(var(--ns-spacing-1) * 1.5);
    background: var(--ns-color-surface-panel);
    border-radius: var(--ns-radius-sm);
    font-family: var(--ns-font-family-mono);
    font-size: calc(var(--ns-font-size-sm) * 0.9);
    color: var(--ns-color-primary-700);
  }

  :global(.ns-markdown-link) {
    color: var(--ns-color-primary-500);
    text-decoration: underline;
  }

  :global(.ns-markdown-link:hover) {
    color: var(--ns-color-primary-600);
  }

  /* Footer styling */
  .ns-text-node__footer {
    display: flex;
    justify-content: space-between;
    align-items: center;
    gap: var(--ns-spacing-2);
    font-size: var(--ns-font-size-xs);
  }

  /* Save status styling */
  .ns-text-node__status {
    display: flex;
    align-items: center;
    gap: var(--ns-spacing-1);
    font-weight: var(--ns-font-weight-medium);
  }

  .ns-text-node__status-icon {
    font-size: var(--ns-font-size-xs);
  }

  .ns-text-node-status--saved {
    color: var(--ns-color-success-600);
  }

  .ns-text-node-status--saving {
    color: var(--ns-color-primary-500);
  }

  .ns-text-node-status--unsaved {
    color: var(--ns-color-warning-600);
  }

  .ns-text-node-status--error {
    color: var(--ns-color-error-600);
  }

  /* Meta information */
  .ns-text-node__meta {
    display: flex;
    gap: var(--ns-spacing-3);
    color: var(--ns-color-text-tertiary);
  }

  /* Editing state modifier */
  :global(.ns-text-node--editing) {
    border-color: var(--ns-color-primary-500);
    box-shadow: 0 0 0 1px var(--ns-color-primary-500);
  }

  /* Compact mode modifier for responsive panels */
  :global(.ns-text-node--compact) .ns-text-node__content {
    font-size: var(--ns-font-size-xs);
    line-height: var(--ns-line-height-normal);
  }

  :global(.ns-text-node--compact) .ns-text-node__hint {
    display: none; /* Hide editing hints in compact mode */
  }

  :global(.ns-text-node--compact) .ns-text-node__markdown {
    font-size: var(--ns-font-size-xs);
  }

  /* Responsive adjustments */
  @media (max-width: 640px) {
    .ns-text-node__footer {
      flex-direction: column;
      align-items: flex-start;
      gap: var(--ns-spacing-1);
    }
  }
</style>
