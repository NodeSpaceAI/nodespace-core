<!--
  AiChatHeader — the header both chat viewers share: the chat's editable title
  on the left and a snippet of viewer-specific controls on the right.

  Not a Node/Viewer component (it is an internal helper, named for what it
  renders). The title is the node's `content`; renaming persists through the
  shared store and patches the sidebar's chat list in place.
-->

<script lang="ts">
  import type { Snippet } from 'svelte';
  import { sharedNodeStore } from '$lib/services/shared-node-store.svelte';
  import { aiChatsData } from '$lib/stores/ai-chats.svelte';
  import {
    aiChatDisplayTitle,
    resolveChatTitleCommit,
    UNTITLED_CHAT_LABEL
  } from '$lib/utils/ai-chat-title';

  let {
    nodeId,
    actions
  }: {
    nodeId: string;
    /** Controls rendered at the right edge of the header. */
    actions?: Snippet;
  } = $props();

  const content = $derived(sharedNodeStore.getNode(nodeId)?.content);

  /** True while the title is an editable input rather than static text. */
  let editingTitle = $state(false);
  /** The in-progress edit. Only meaningful while editingTitle is true. */
  let titleDraft = $state('');
  let titleInputEl: HTMLInputElement | undefined = $state();

  /** Enter edit mode, seeded with the node's raw (untrimmed, possibly empty)
   *  content — never the "Untitled chat" placeholder, which is a display
   *  fallback, not a value to edit. */
  function startEditingTitle(): void {
    if (editingTitle) return;
    titleDraft = content ?? '';
    editingTitle = true;
  }

  /** Persist the edit if it actually changed anything, then leave edit mode. */
  function commitTitle(): void {
    if (!editingTitle) return;
    editingTitle = false;
    const toPersist = resolveChatTitleCommit(content ?? '', titleDraft);
    if (toPersist === null) return;
    sharedNodeStore.updateNode(
      nodeId,
      { content: toPersist },
      { type: 'viewer', viewerId: 'ai-chat-viewer' }
    );
    // Keep the sidebar's chat list in sync without a full reload — mirrors
    // how `aiChatsData.createChat` already prepends optimistically.
    aiChatsData.updateChatContent(nodeId, toPersist);
  }

  /** Leave edit mode without persisting — the draft is simply discarded. */
  function cancelEditingTitle(): void {
    editingTitle = false;
  }

  function onTitleKeydown(event: KeyboardEvent): void {
    if (event.key === 'Enter') {
      event.preventDefault();
      commitTitle();
    } else if (event.key === 'Escape') {
      event.preventDefault();
      cancelEditingTitle();
    }
  }

  // Focus (and select) the input the moment it mounts, so entering edit mode
  // drops the user straight into typing rather than requiring a second click.
  $effect(() => {
    if (editingTitle && titleInputEl) {
      titleInputEl.focus();
      titleInputEl.select();
    }
  });
</script>

<div class="chat-viewer-header">
  <div class="chat-viewer-header-left">
    {#if editingTitle}
      <input
        bind:this={titleInputEl}
        class="chat-viewer-title-input"
        type="text"
        value={titleDraft}
        oninput={(e) => (titleDraft = e.currentTarget.value)}
        onblur={commitTitle}
        onkeydown={onTitleKeydown}
        aria-label="Chat title"
        placeholder={UNTITLED_CHAT_LABEL}
      />
    {:else}
      <h2 class="chat-viewer-title">
        <button
          type="button"
          class="chat-viewer-title-button"
          onclick={startEditingTitle}
          aria-label="Rename chat"
        >
          {aiChatDisplayTitle(content)}
        </button>
      </h2>
    {/if}
  </div>
  <div class="chat-viewer-header-right">
    {@render actions?.()}
  </div>
</div>

<style>
  .chat-viewer-header {
    display: flex;
    align-items: center;
    justify-content: space-between;
    padding: 0.75rem 1rem;
    border-bottom: 1px solid hsl(var(--border));
    background: hsl(var(--background));
    flex-shrink: 0;
    gap: 0.75rem;
  }

  .chat-viewer-header-left {
    display: flex;
    flex-direction: column;
    gap: 0.25rem;
    flex: 1;
    min-width: 0;
  }

  .chat-viewer-header-right {
    display: flex;
    align-items: center;
    gap: 0.5rem;
    flex-shrink: 0;
  }

  .chat-viewer-title {
    font-size: 1rem;
    font-weight: 600;
    margin: 0;
    color: hsl(var(--foreground));
    min-width: 0;
  }

  /* The rename target and the input it turns into share one box: padding and
     a border around the text, pulled back out by the same negative margin, so
     the text sits on the header's left edge in both and doesn't move when one
     replaces the other. The box is therefore wider than its container by the
     padding and border on both sides, which the width allowance gives back —
     without it the padding comes out of the text's room and clips the title. */
  .chat-viewer-title-button,
  .chat-viewer-title-input {
    --title-inset-x: calc(0.25rem + 1px);
    box-sizing: border-box;
    display: block;
    padding: 0.125rem 0.25rem;
    margin: calc(-0.125rem - 1px) calc(-1 * var(--title-inset-x));
    border: 1px solid transparent;
    border-radius: 0.25rem;
    font: inherit;
    color: inherit;
  }

  .chat-viewer-title-button {
    max-width: calc(100% + 2 * var(--title-inset-x));
    background: transparent;
    text-align: left;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    cursor: text;
  }

  .chat-viewer-title-button:hover {
    background: hsl(var(--muted) / 0.6);
  }

  .chat-viewer-title-input {
    width: calc(100% + 2 * var(--title-inset-x));
    border-color: hsl(var(--border));
    background: hsl(var(--background));
    font-size: 1rem;
    font-weight: 600;
    color: hsl(var(--foreground));
  }

  .chat-viewer-title-input:focus {
    outline: none;
    border-color: hsl(var(--primary));
  }
</style>
