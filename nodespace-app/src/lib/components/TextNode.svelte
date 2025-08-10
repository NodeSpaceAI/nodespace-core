<!--
  TextNode Component
  
  Extends BaseNode with markdown support for display mode.
  Inherits universal edit/display functionality from BaseNode.
-->

<script lang="ts">
  import { createEventDispatcher, onMount } from 'svelte';
  import BaseNode from '$lib/design/components/BaseNode.svelte';
  import { mockTextService, type TextNodeData } from '$lib/services/mockTextService';
  import { parseMarkdown } from '$lib/services/markdownUtils';

  // Props
  export let nodeId: string;
  export let content: string = '';
  export let editable: boolean = true;
  export let markdown: boolean = true;
  export let placeholder: string = 'Click to add text...';
  export let autoSave: boolean = true;
  export let compact: boolean = false;

  // Component state
  let saveStatus: 'saved' | 'saving' | 'unsaved' | 'error' = 'saved';
  let saveError = '';
  let textNodeData: TextNodeData | null = null;
  let lastSavedContent = content;

  // Event dispatcher
  const dispatch = createEventDispatcher<{
    save: { nodeId: string; content: string };
    error: { nodeId: string; error: string };
  }>();

  // Load existing node data on mount if nodeId provided
  onMount(async () => {
    if (nodeId && nodeId !== 'new') {
      try {
        textNodeData = await mockTextService.loadTextNode(nodeId);
        if (textNodeData) {
          content = textNodeData.content;
          lastSavedContent = content;
          saveStatus = 'saved';
        }
      } catch (error) {
        console.error('Failed to load text node:', error);
        saveError = error instanceof Error ? error.message : 'Failed to load content';
        saveStatus = 'error';
      }
    }
  });

  // Auto-save functionality
  let debounceTimeout: NodeJS.Timeout;
  const DEBOUNCE_DELAY = 2000;

  function debounceAutoSave() {
    if (!autoSave) return;

    clearTimeout(debounceTimeout);
    saveStatus = 'unsaved';

    debounceTimeout = setTimeout(async () => {
      if (content !== lastSavedContent) {
        await handleAutoSave();
      }
    }, DEBOUNCE_DELAY);
  }

  // Handle content changes from BaseNode
  function handleContentChanged(event: CustomEvent) {
    content = event.detail.content;
    debounceAutoSave();
  }

  // Auto-save handler
  async function handleAutoSave() {
    if (!autoSave || !nodeId || nodeId === 'new') return;

    try {
      const result = await mockTextService.scheduleAutoSave(nodeId, content, '', 100);

      if (result.success) {
        lastSavedContent = content;
        saveStatus = 'saved';
        dispatch('save', { nodeId, content });
      } else {
        saveError = result.error || 'Auto-save failed';
        saveStatus = 'error';
        dispatch('error', { nodeId, error: saveError });
      }
    } catch (error) {
      saveError = error instanceof Error ? error.message : 'Auto-save error';
      saveStatus = 'error';
      dispatch('error', { nodeId, error: saveError });
    }
  }

  // Render markdown content
  $: renderedContent = markdown && content ? parseMarkdown(content) : content;

  // Dynamic multiline based on content - starts single-line, becomes multiline if content has line breaks
  $: isMultiline = content.includes('\n');
</script>

<BaseNode
  {nodeId}
  nodeType="text"
  bind:content
  hasChildren={textNodeData?.metadata.hasChildren || false}
  isProcessing={saveStatus === 'saving'}
  {editable}
  contentEditable={true}
  multiline={isMultiline}
  {placeholder}
  className="ns-text-node {compact ? 'ns-text-node--compact' : ''}"
  on:contentChanged={handleContentChanged}
>
  <!-- Override display content for markdown rendering -->
  <div slot="display-content">
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
        <span class="ns-node__text">{content}</span>
      {/if}
    {:else}
      <span class="ns-node__empty">{placeholder}</span>
    {/if}
  </div>

  <!-- Additional TextNode-specific content in default slot -->
  {#if saveStatus !== 'saved' || saveError}
    <div class="ns-text-node__status">
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
  {:else if textNodeData}
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
  /* Markdown rendering */
  .ns-text-node__markdown {
    font-size: 14px;
    line-height: 1.4;
    word-break: break-word;
  }

  /* Save status styling */
  .ns-text-node__status {
    display: flex;
    align-items: center;
    gap: 4px;
    font-size: 12px;
    font-weight: 500;
    margin-top: 4px;
    color: hsl(var(--muted-foreground));
  }

  .ns-text-node__status-icon {
    font-size: 10px;
  }

  /* Meta information */
  .ns-text-node__meta {
    display: flex;
    gap: 12px;
    font-size: 12px;
    margin-top: 4px;
    color: hsl(var(--muted-foreground));
  }
</style>
