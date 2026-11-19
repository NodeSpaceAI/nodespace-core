<!--
  AcpSessionBar - the state of an ACP chat's session: active or ended, the
  agent, context usage, and the controls the session supports (stop the turn,
  end, resume). The state is text; a live region announces a change.
-->

<script lang="ts">
  import type { AiChatSessionStatus } from '$lib/types/ai-chat-node';
  import type { AcpSessionSource } from '$lib/types/acp-session';

  let {
    status,
    agent,
    session
  }: {
    status: AiChatSessionStatus;
    agent: string;
    session?: AcpSessionSource;
  } = $props();

  const numbers = new Intl.NumberFormat('en-US');
  const active = $derived(status === 'active');
</script>

<div class="acp-session-bar" role="group" aria-label="Session">
  <span class="acp-session-state" role="status" class:ended={!active}>
    Session {active ? 'active' : 'ended'}
  </span>
  {#if agent}
    <span class="acp-session-agent">{agent}</span>
  {/if}
  {#if session?.usage}
    <span class="acp-session-usage">
      Context {numbers.format(session.usage.used)} of {numbers.format(session.usage.size)} tokens
    </span>
  {/if}
  <span class="acp-session-spacer"></span>
  {#if session?.turnRunning && session.cancelTurn}
    <button type="button" class="acp-session-button" onclick={() => void session.cancelTurn?.()}>
      Stop
    </button>
  {/if}
  {#if active && session?.endSession}
    <button type="button" class="acp-session-button" onclick={() => void session.endSession?.()}>
      End session
    </button>
  {/if}
  {#if !active && session?.resumeSession}
    <button type="button" class="acp-session-button" onclick={() => void session.resumeSession?.()}>
      Resume session
    </button>
  {/if}
</div>

<style>
  .acp-session-bar {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 0.5rem 0.75rem;
    padding: 0.375rem 1rem;
    border-bottom: 1px solid hsl(var(--border));
    font-size: 0.8125rem;
    color: hsl(var(--muted-foreground));
  }

  .acp-session-state {
    font-weight: 600;
    color: hsl(var(--foreground));
  }

  .acp-session-state::before {
    content: '● ';
    color: hsl(var(--success));
  }

  .acp-session-state.ended::before {
    content: '○ ';
    color: hsl(var(--muted-foreground));
  }

  .acp-session-spacer {
    flex: 1;
  }

  .acp-session-button {
    padding: 0.125rem 0.625rem;
    border: 1px solid hsl(var(--border));
    border-radius: 0.25rem;
    background: transparent;
    color: hsl(var(--foreground));
    font: inherit;
    cursor: pointer;
  }

  .acp-session-button:hover {
    background: hsl(var(--accent));
  }

  .acp-session-button:focus-visible {
    outline: 2px solid hsl(var(--ring));
    outline-offset: 2px;
  }
</style>
