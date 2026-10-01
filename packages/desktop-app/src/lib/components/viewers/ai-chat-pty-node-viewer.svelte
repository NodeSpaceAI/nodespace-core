<!--
  AiChatPtyNodeViewer - Page-level viewer for terminal AI chat nodes

  `ai-chat-pty` is a chat an external agent harness (Claude Code, Codex, ...)
  runs in a terminal (ADR-088). This viewer renders the shared header (the
  editable title; there is no model selector, the harness is the node's `agent`)
  and hosts the terminal session (AiChatPtySession). Native chats are a
  separate subtype with their own viewer (AiChatNativeNodeViewer).
-->

<script lang="ts">
  import AiChatHeader from './ai-chat-header.svelte';
  import AiChatPtySession from './ai-chat-pty-session.svelte';

  let {
    nodeId,
  }: {
    nodeId: string;
  } = $props();
</script>

<div class="ai-chat-viewer">
  <AiChatHeader {nodeId} />
  <AiChatPtySession {nodeId} />
</div>

<style>
  .ai-chat-viewer {
    display: flex;
    flex-direction: column;
    height: 100%;
    background: hsl(var(--background));
    position: relative;
  }
</style>
