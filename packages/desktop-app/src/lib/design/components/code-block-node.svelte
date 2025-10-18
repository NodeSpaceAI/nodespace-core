<!--
  CodeBlockNode - Wraps BaseNode with code block-specific functionality

  Responsibilities:
  - Manages language selection for code blocks
  - Provides monospace font styling with muted background
  - Handles code content editing (multiline with Shift+Enter)
  - Displays closing ``` fence as visual indicator
  - Language dropdown UI (appears on hover)
  - Copy button functionality (appears on hover)
  - Forwards all other events to BaseNode

  Integration:
  - Uses icon registry for proper CodeBlockIcon rendering
  - Maintains compatibility with BaseNode API
  - Works seamlessly in node tree structure

  Design System Reference: docs/design-system/components.html → Code Block Nodes
-->

<script lang="ts">
  import { createEventDispatcher } from 'svelte';
  import BaseNode from './base-node.svelte';
  import { focusManager } from '$lib/services/focus-manager.svelte';

  // Supported languages for code blocks
  // Alphabetically sorted with 'plaintext' as default first option
  const LANGUAGES = [
    'plaintext', // Default - always first
    'cpp',
    'css',
    'go',
    'html',
    'java',
    'javascript',
    'kotlin',
    'markdown',
    'php',
    'python',
    'ruby',
    'rust',
    'shell',
    'sql',
    'swift',
    'typescript'
  ];

  // Props using Svelte 5 runes mode - same interface as BaseNode
  let {
    nodeId,
    nodeType = 'code-block',
    autoFocus = false,
    content = '',
    children = []
  }: {
    nodeId: string;
    nodeType?: string;
    autoFocus?: boolean;
    content?: string;
    children?: string[];
  } = $props();

  const dispatch = createEventDispatcher();

  // Internal reactive state - sync with content prop changes
  let internalContent = $state(content);

  // Sync internalContent when content prop changes externally
  // Auto-complete if content doesn't have closing fence
  $effect(() => {
    if (content && content.startsWith('```') && !content.includes('\n```')) {
      // Missing closing fence - auto-complete
      const afterFence = content.substring(3); // Content after ```

      if (afterFence === '' || afterFence === '\n' || afterFence === '\n\n') {
        // Empty or just newlines
        internalContent = '```plaintext\n\n```';
      } else {
        // Has content - add closing fence
        internalContent = `${content}\n\`\`\``;
      }

      // Update parent
      dispatch('contentChanged', { content: internalContent });
    } else {
      internalContent = content;
    }
  });

  // Parse language from opening fence (```language or just ```)
  let language = $state<string>(parseLanguage(content));

  // Code blocks use multiline editing (Shift+Enter for new lines, Enter creates new node)
  const editableConfig = { allowMultiline: true };

  // Check if this node is being edited (from FocusManager)
  let isEditing = $derived(focusManager.editingNodeId === nodeId);

  // Dropdown state
  let showLanguageDropdown = $state(false);
  let wrapperElement = $state<HTMLDivElement | undefined>(undefined);

  // Copy button state
  let copied = $state(false);

  // Create reactive metadata object with language and markdown flag
  let codeMetadata = $derived({
    language,
    disableMarkdown: true // Code blocks should not process markdown
  });

  /**
   * Parse language from content (opening fence)
   * Supports: ```javascript, ```python, etc.
   * Defaults to 'plaintext' if no language specified
   */
  function parseLanguage(content: string): string {
    // Match ```language at start of content
    const match = content.match(/^```(\w+)?/);
    if (match && match[1]) {
      return match[1].toLowerCase();
    }
    return 'plaintext';
  }

  /**
   * Extract code content for editing (remove language from opening fence only)
   * User sees and edits: ```\ncode\n```
   * Storage has: ```language\ncode\n```
   */
  function extractCodeForEditing(content: string): string {
    // Replace ```language with just ``` (user doesn't see/edit language)
    return content.replace(/^```\w+/, '```');
  }

  /**
   * Extract code content for display (replace fences with empty lines)
   * View mode: fence lines become empty (preserving line breaks for spacing)
   */
  function extractCodeForDisplay(content: string): string {
    // Replace ```language with empty string, keep newline (creates leading empty line)
    let result = content.replace(/^```\w*/, '');
    // Replace closing ``` with newline (creates trailing empty line)
    result = result.replace(/```$/, '\n');
    return result;
  }

  // Edit mode: Show fences without language (```\ncode\n```)
  // View mode: Show just code (no fences)
  let editContent = $derived(extractCodeForEditing(internalContent));
  let displayContent = $derived(extractCodeForDisplay(internalContent));

  /**
   * Extract code content (strip both opening and closing fences for copy functionality)
   */
  function extractCodeContent(content: string): string {
    // Remove opening ```language fence
    let result = content.replace(/^```\w*\s*\n?/, '');
    // Remove closing ``` fence
    result = result.replace(/\n?```\s*$/, '');
    return result;
  }

  /**
   * Update language and content when language changes
   */
  function handleLanguageChange(newLanguage: string) {
    language = newLanguage;
    showLanguageDropdown = false;

    // Just replace the language in the existing content
    const newContent = internalContent.replace(/^```\w*/, `\`\`\`${newLanguage}`);

    internalContent = newContent;
    dispatch('contentChanged', { content: newContent });
  }

  /**
   * Handle content changes from BaseNode
   * User edits: ```\ncode\n```
   * We inject language: ```{language}\ncode\n```
   */
  function handleContentChange(event: CustomEvent<{ content: string }>) {
    const userContent = event.detail.content;

    // Inject language into opening fence
    const withLanguage = userContent.replace(/^```/, `\`\`\`${language}`);

    internalContent = withLanguage;
    dispatch('contentChanged', { content: withLanguage });
  }

  /**
   * Copy code content to clipboard
   */
  async function handleCopy() {
    const codeContent = extractCodeContent(internalContent);
    try {
      // Check if clipboard API is available (browser environment)
      if (typeof window !== 'undefined' && window.navigator?.clipboard) {
        await window.navigator.clipboard.writeText(codeContent);
        // Show "copied!" feedback for 2 seconds
        copied = true;
        setTimeout(() => {
          copied = false;
        }, 2000);
      }
    } catch {
      // Silently fail - clipboard access may be denied
    }
  }

  /**
   * Toggle language dropdown
   */
  function toggleLanguageDropdown() {
    showLanguageDropdown = !showLanguageDropdown;
  }

  /**
   * Close dropdown when clicking outside
   */
  function handleClickOutside(event: MouseEvent) {
    if (wrapperElement && !wrapperElement.contains(event.target as Node)) {
      showLanguageDropdown = false;
    }
  }

  // Bind click outside handler
  $effect(() => {
    if (showLanguageDropdown) {
      document.addEventListener('click', handleClickOutside);
      return () => {
        document.removeEventListener('click', handleClickOutside);
      };
    }
  });

  /**
   * Handle createNewNode event - disable splitting, create blank code-block below
   */
  function handleCreateNewNode(event: CustomEvent) {
    const detail = event.detail;

    // For code-blocks: no splitting, create blank code-block below with cursor ready
    if (detail.nodeType === 'code-block') {
      detail.currentContent = internalContent; // Keep current node unchanged
      detail.newContent = '```plaintext\n\n```'; // New blank code-block with template
      // Cursor position is for edit content (```\n|\n```) which is position 4
      detail.newNodeCursorPosition = 4; // After ```\n, on the empty line
    }

    dispatch('createNewNode', detail);
  }

  /**
   * Handle node type conversion - auto-complete structure when converting to code-block
   */
  function handleNodeTypeChanged(event: CustomEvent) {
    const detail = event.detail;

    // First dispatch the event as-is
    dispatch('nodeTypeChanged', detail);

    // Then auto-complete if needed (after node is converted)
    if (detail.newNodeType === 'code-block') {
      const cleaned = detail.cleanedContent || '';

      // Check if it needs auto-completion (doesn't have closing fence)
      if (!cleaned.includes('```', 3)) {
        // Check for closing fence (skip first ```)
        // Schedule auto-completion for next tick
        setTimeout(() => {
          const afterFence = cleaned.substring(3).trim(); // Content after ```

          let completedContent;
          if (afterFence === '' || afterFence === '\n') {
            // Empty: ```plaintext\n\n```
            completedContent = '```plaintext\n\n```';
          } else {
            // Has content: ```plaintext\nContent\n```
            const contentLines = afterFence.split('\n').filter((line: string) => line.trim());
            completedContent = `\`\`\`plaintext\n${contentLines.join('\n')}\n\`\`\``;
          }

          internalContent = completedContent;
          dispatch('contentChanged', { content: completedContent });
        }, 0);
      }
    }
  }

  /**
   * Forward all other events to parent components
   */
  function forwardEvent<T>(eventName: string) {
    return (event: CustomEvent<T>) => dispatch(eventName, event.detail);
  }
</script>

<!-- Wrap BaseNode with code-block-specific styling -->
<div class="code-block-node-wrapper" bind:this={wrapperElement}>
  <BaseNode
    {nodeId}
    {nodeType}
    {autoFocus}
    content={editContent}
    {displayContent}
    {children}
    {editableConfig}
    metadata={codeMetadata}
    on:createNewNode={handleCreateNewNode}
    on:contentChanged={handleContentChange}
    on:indentNode={forwardEvent('indentNode')}
    on:outdentNode={forwardEvent('outdentNode')}
    on:navigateArrow={forwardEvent('navigateArrow')}
    on:combineWithPrevious={forwardEvent('combineWithPrevious')}
    on:deleteNode={forwardEvent('deleteNode')}
    on:focus={forwardEvent('focus')}
    on:blur={forwardEvent('blur')}
    on:nodeReferenceSelected={forwardEvent('nodeReferenceSelected')}
    on:slashCommandSelected={forwardEvent('slashCommandSelected')}
    on:nodeTypeChanged={handleNodeTypeChanged}
    on:iconClick={forwardEvent('iconClick')}
  />

  <!-- Language selector button (appears on hover, only in edit mode) -->
  {#if isEditing}
    <button
      class="code-language-button"
      onmousedown={(e) => {
        e.preventDefault(); // Prevent blur
        e.stopPropagation();
      }}
      onclick={(e) => {
        e.preventDefault();
        e.stopPropagation();
        toggleLanguageDropdown();
      }}
      type="button"
    >
      {language}
    </button>

    <!-- Language dropdown menu -->
    {#if showLanguageDropdown}
      <div
        class="code-language-dropdown"
        onclick={(e) => e.stopPropagation()}
        role="menu"
        tabindex="-1"
        onkeydown={(e) => {
          if (e.key === 'Escape') {
            showLanguageDropdown = false;
          }
        }}
      >
        {#each LANGUAGES as lang}
          <button
            class="language-option"
            class:selected={lang === language}
            onmousedown={(e) => {
              e.preventDefault(); // Prevent blur
            }}
            onclick={(e) => {
              e.preventDefault();
              handleLanguageChange(lang);
            }}
            type="button"
          >
            {lang}
          </button>
        {/each}
      </div>
    {/if}
  {/if}

  <!-- Copy button (appears on hover, always visible) -->
  <button
    class="code-copy-button"
    class:copied
    onclick={(e) => {
      e.stopPropagation();
      handleCopy();
    }}
    type="button"
  >
    {copied ? 'copied!' : 'copy'}
  </button>
</div>

<style>
  /* Code block wrapper - icon outside, content with background */
  .code-block-node-wrapper {
    width: 100%;
    display: block;
    position: relative;
  }

  /* Apply muted background to the content area only (BaseNode wraps it) */
  .code-block-node-wrapper :global(.node__content) {
    background: hsl(var(--muted));
    padding: 0.25rem;
    border-radius: var(--radius);
    font-family: 'SF Mono', Monaco, 'Cascadia Code', 'Roboto Mono', Consolas, monospace;
    font-size: 0.875rem;
    line-height: 1.5;
    padding-right: 10rem; /* Space for buttons */
  }

  /* Language selector button (top-right, appears on hover) */
  .code-language-button {
    position: absolute;
    top: 0.5rem;
    right: 3.9375rem;
    background: hsl(var(--background));
    border: 1px solid hsl(var(--border));
    color: hsl(var(--foreground));
    padding: 0.25rem 0.5rem;
    border-radius: 0.25rem;
    font-size: 0.75rem;
    cursor: pointer;
    opacity: 0;
    transition: opacity 0.2s ease;
    width: 8rem;
    box-sizing: border-box;
    text-align: left;
  }

  .code-block-node-wrapper:hover .code-language-button {
    opacity: 1;
  }

  /* Language dropdown menu */
  .code-language-dropdown {
    position: absolute;
    top: 2rem;
    right: 3.9375rem;
    background: hsl(var(--background));
    border: 1px solid hsl(var(--border));
    border-radius: 0.25rem;
    width: 8rem;
    max-height: 10.5rem;
    overflow-y: auto;
    box-shadow: 0 4px 6px rgba(0, 0, 0, 0.1);
    z-index: 10;
    box-sizing: border-box;
  }

  /* Language option in dropdown */
  .language-option {
    padding: 0.5rem 0.75rem;
    cursor: pointer;
    font-size: 0.75rem;
    color: hsl(var(--foreground));
    border: none;
    border-bottom: 1px solid hsl(var(--border));
    background: transparent;
    text-align: left;
    width: 100%;
    display: block;
  }

  .language-option:last-child {
    border-bottom: none;
  }

  .language-option:hover {
    background: hsl(var(--muted));
  }

  .language-option.selected {
    background: hsl(var(--muted));
    font-weight: 600;
  }

  /* Copy button (top-right, appears on hover) */
  .code-copy-button {
    position: absolute;
    top: 0.5rem;
    right: 0.5rem;
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
  }

  .code-block-node-wrapper:hover .code-copy-button {
    opacity: 1;
  }

  /* Copied state - green background with success color */
  .code-copy-button.copied {
    opacity: 1;
    background: hsl(var(--success, 142 76% 36%));
    color: hsl(var(--success-foreground, 0 0% 100%));
    border-color: hsl(var(--success, 142 76% 36%));
  }
</style>
