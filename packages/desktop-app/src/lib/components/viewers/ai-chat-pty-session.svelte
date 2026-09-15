<!--
  ai-chat-pty-session — provider mode 2d (pty) sub-view, composed by
  AiChatNodeViewer. Not a Node/Viewer component (it's an internal helper, named
  like ChatMessage/ChatInput), so it carries no *Node/*Viewer/*View suffix.

  Per ADR-034, a PTY agent session IS an ai-chat node (provider: pty). This
  helper renders that mode: a launch config (harness picker + Launch) when no
  session is running, the embedded xterm terminal (via pty-terminal.svelte) while
  it runs, and a read-only summary once it ends. The node already exists; capture
  backfills it at session end via the node_id passed to launch.
-->

<script lang="ts">
  import { onMount } from 'svelte';
  import { listen, type UnlistenFn } from '@tauri-apps/api/event';
  import PtyTerminal from '$lib/components/agent/pty-terminal.svelte';
  import { sharedNodeStore } from '$lib/services/shared-node-store.svelte';
  import type { AiChatNode } from '$lib/types/ai-chat-node';
  import {
    getCaptureSettings,
    updateCaptureSettings,
    ptyCheckAgentAvailability,
    ptyLaunchSession,
    type AgentAvailabilityInfo,
    type CaptureContentLevel,
  } from '$lib/services/tauri-commands';
  import { createLogger } from '$lib/utils/logger';
  import { toError } from '$lib/types/errors';

  const log = createLogger('AiChatPtySession');

  let { nodeId }: { nodeId: string } = $props();

  const AGENT_OPTIONS = [
    { id: 'claude-code', label: 'Claude Code' },
    { id: 'codex', label: 'Codex' },
    { id: 'antigravity-cli', label: 'Antigravity CLI' },
    { id: 'pi', label: 'Pi' },
    { id: 'open-code', label: 'Open Code' },
  ];

  const CONTENT_LEVELS: { value: CaptureContentLevel; label: string }[] = [
    { value: 'metadata_only', label: 'Metadata only' },
    { value: 'summary', label: 'Summary' },
    { value: 'full', label: 'Full transcript' },
  ];

  type AgentStatus =
    | 'ready'
    | 'binary_missing'
    | 'auth_missing'
    | 'binary_missing_and_auth_missing'
    | 'unknown';

  const node = $derived(sharedNodeStore.getNode(nodeId));

  // A previously-launched session id persisted on the node. While the session
  // is live (sessionStatus 'active') this lets the terminal re-attach on
  // reopen (the daemon owns the PTY and supports multi-client streaming per
  // ADR-032). Once the session has ended (sessionStatus 'archived', set by
  // capture backfill) the PTY is gone — re-attaching would just show a blank,
  // silent terminal — so we render a read-only summary instead.
  //
  // sessionStatus is read from the TOP-LEVEL promoted field, not
  // `properties['ai-chat'].session_status` — the daemon flattens the
  // namespace before this reaches the frontend, same contract
  // `ai-chat-node-viewer.svelte` documents for turnStatus.
  const persistedSessionId = $derived(
    (node?.properties?.['capture:session_id'] as string | undefined) ?? null
  );

  // Latches when *this* viewer's just-launched session exits, so the UI flips
  // to the ended state live without waiting for a reload (the daemon's capture
  // backfill updates the DB out-of-band, not sharedNodeStore).
  let sessionEnded = $state(false);

  // Set when the user explicitly chooses "Start new session" from the ended
  // view, forcing the config step even though a stale `capture:session_id` is
  // still persisted (capture backfill only overwrites it once the *next*
  // session ends — see startNewSession()).
  let configuring = $state(false);

  let activeSessionId = $state<string | null>(null);

  // The session has ended if capture marked the node archived, or we observed
  // its exit this session.
  const isEnded = $derived(
    !configuring &&
      (sessionEnded ||
        (node as unknown as AiChatNode | undefined)?.sessionStatus === 'archived')
  );
  // Only host a live terminal when the session is still running. A session this
  // viewer launched (activeSessionId) always wins; otherwise re-attach to the
  // persisted id — but never while configuring a replacement, since the
  // persisted id may point at the previous (dead) session.
  const sessionId = $derived(
    isEnded || configuring ? activeSessionId : (activeSessionId ?? persistedSessionId)
  );

  const agentType = $derived(
    (node?.properties?.['capture:agent_type'] as string | undefined) ?? null
  );
  const summary = $derived(
    (node?.properties?.['capture:summary'] as string | undefined) ?? null
  );
  const transcript = $derived(
    (node?.properties?.['capture:transcript'] as string | undefined) ?? null
  );

  // Listen for the live session's exit so the view flips to the ended state
  // immediately (a re-attached dead session would otherwise render a blank
  // terminal). The $effect cleanup runs on both activeSessionId change and
  // component unmount, so the listener never leaks.
  $effect(() => {
    const id = activeSessionId;
    if (!id) return;
    let cancelled = false;
    let unlisten: UnlistenFn | null = null;
    listen(`pty-closed-${id}`, () => {
      sessionEnded = true;
    })
      .then((fn) => {
        // If cleanup already ran before the listener registered, unlisten now.
        if (cancelled) fn();
        else unlisten = fn;
      })
      .catch((e) => log.warn('Failed to register pty-closed listener', e));
    return () => {
      cancelled = true;
      unlisten?.();
    };
  });

  /**
   * Return to the config step to launch a fresh session on this node.
   *
   * Note: we do NOT clear the persisted `capture:session_id` here. The store's
   * property merge is additive (it cannot remove a key by omission, and writing
   * null would persist a literal null), so the stale id is left in place and
   * launch() overwrites it with the new session id. The `configuring` flag
   * suppresses re-attach to that stale id in the meantime.
   */
  function startNewSession(): void {
    sessionEnded = false;
    activeSessionId = null;
    error = null;
    configuring = true;
  }

  let selectedAgent = $state('claude-code');
  let launching = $state(false);
  let error = $state<string | null>(null);

  let captureEnabled = $state(false);
  let captureContent = $state<CaptureContentLevel>('metadata_only');

  let availability = $state<Record<string, AgentAvailabilityInfo>>({});
  let availabilityLoading = $state(true);

  onMount(async () => {
    // Pre-select the agent chosen in the header AiChatModelSelector, when one
    // was already stored on the node; otherwise keep the
    // 'claude-code' default.
    const nodeModel = node?.properties?.model as string | undefined;
    if (nodeModel) {
      selectedAgent = nodeModel;
    }

    try {
      const [settings, availResult] = await Promise.all([
        getCaptureSettings(),
        ptyCheckAgentAvailability(),
      ]);
      captureEnabled = settings.enabled;
      captureContent = settings.content;
      const map: Record<string, AgentAvailabilityInfo> = {};
      for (const agent of availResult.agents) {
        map[agent.agentType] = agent;
      }
      availability = map;
    } catch (e) {
      log.warn('Failed to load pty session settings', e);
    } finally {
      availabilityLoading = false;
    }
  });

  async function saveCaptureSettings() {
    try {
      await updateCaptureSettings({
        enabled: captureEnabled,
        content: captureContent,
      });
    } catch (e) {
      log.error('Failed to save capture settings', e);
    }
  }

  function selectedAvailability(): AgentAvailabilityInfo | undefined {
    return availability[selectedAgent];
  }

  function agentStatus(agentId: string): AgentStatus {
    const av = availability[agentId];
    if (!av) return 'unknown';
    if (!av.binaryFound && !av.authFound) return 'binary_missing_and_auth_missing';
    if (!av.binaryFound) return 'binary_missing';
    if (!av.authFound) return 'auth_missing';
    return 'ready';
  }

  async function launch() {
    launching = true;
    error = null;
    try {
      const result = await ptyLaunchSession({
        agentType: selectedAgent,
        prompt: null,
        cols: 80,
        rows: 24,
        nodeId,
      });
      activeSessionId = result.sessionId;
      configuring = false;

      // Record the chosen agent + session on the node up front so the node
      // reflects its mode immediately. Capture backfills the rest at session
      // end (transcript/summary/exit code) via the node_id passed above.
      const current = sharedNodeStore.getNode(nodeId);
      sharedNodeStore.updateNode(
        nodeId,
        {
          properties: {
            ...current?.properties,
            'capture:agent_type': selectedAgent,
            'capture:session_id': result.sessionId,
            // Canonical snake_case key, matching the schema's declared field
            // name and what capture_service.rs's backfill writes — ai-chat
            // has no dedicated typed write command, so whatever key this
            // object uses reaches storage verbatim (see AiChatNode::
            // from_node's doc comment for why a camelCase key here would be
            // silently wrong, not just inconsistent).
            session_status: 'active',
          },
        },
        { type: 'viewer', viewerId: 'ai-chat-pty-session' }
      );
    } catch (e) {
      log.error('Failed to launch session', e);
      error = toError(e).message;
    } finally {
      launching = false;
    }
  }

  function handleKeydown(event: KeyboardEvent) {
    if (event.key === 'Enter' && (event.metaKey || event.ctrlKey)) {
      launch();
    }
  }
</script>

{#if isEnded}
  <!-- Ended session: the PTY is gone. Show a read-only summary of what the
       session was about (mode 2d capture is a reference, not a transcript a
       terminal can replay) + an affordance to start a fresh session. -->
  <div class="pty-ended">
    <div class="pty-ended-card">
      <h3 class="pty-ended-title">Session ended</h3>
      <p class="pty-ended-meta">
        {#if agentType}<span class="pty-ended-agent">{agentType}</span>{/if}
        <span class="pty-ended-badge">archived</span>
      </p>

      {#if summary}
        <p class="pty-ended-summary">{summary}</p>
      {/if}

      {#if transcript}
        <details class="pty-ended-transcript">
          <summary>Transcript</summary>
          <pre>{transcript}</pre>
        </details>
      {:else if !summary}
        <p class="pty-ended-empty">
          No transcript or summary was captured for this session.
        </p>
      {/if}

      <button class="launch-button" onclick={startNewSession}>Start new session</button>
    </div>
  </div>
{:else if sessionId}
  <!-- Active session: the embedded terminal IS the node's viewer. -->
  <div class="pty-terminal-host">
    {#key sessionId}
      <PtyTerminal {sessionId} />
    {/key}
  </div>
{:else}
  <!-- Config step: pick an agent and launch. -->
  <div class="pty-config">
    <div class="pty-config-card">
      <h3 class="pty-config-title">Launch agent session</h3>
      <p class="pty-config-subtitle">
        Run an external agent CLI in an embedded terminal. The session is this node.
      </p>

      <div class="field">
        <label class="field-label" for="agent-select">Agent</label>
        <select id="agent-select" class="field-select" bind:value={selectedAgent} disabled={launching}>
          {#each AGENT_OPTIONS as option (option.id)}
            {@const status = agentStatus(option.id)}
            <option value={option.id}>
              {option.label}{status === 'ready' || status === 'unknown' ? '' : ' ⚠'}
            </option>
          {/each}
        </select>
      </div>

      {#if !availabilityLoading}
        {@const av = selectedAvailability()}
        {@const status = agentStatus(selectedAgent)}
        {#if av && status !== 'ready' && status !== 'unknown'}
          <div class="availability-banner availability-banner--warning" role="alert">
            {#if status === 'binary_missing' || status === 'binary_missing_and_auth_missing'}
              <div class="availability-row">
                <span class="availability-icon">⚠</span>
                <span>
                  <strong>{av.binary}</strong> not found on PATH.
                  {#if av.installHint}
                    <span class="install-hint">{av.installHint}</span>
                  {/if}
                </span>
              </div>
            {/if}
            {#if status === 'auth_missing' || status === 'binary_missing_and_auth_missing'}
              <div class="availability-row">
                <span class="availability-icon">⚠</span>
                <span>Auth credential not configured for this agent.</span>
              </div>
            {/if}
          </div>
        {:else if av && status === 'ready'}
          <div class="availability-banner availability-banner--ready" role="status">
            <span class="availability-icon">✓</span> Ready
          </div>
        {/if}
      {/if}

      <details class="capture-section">
        <summary class="capture-summary">Session capture</summary>
        <div class="capture-body">
          <label class="capture-row">
            <input
              type="checkbox"
              class="capture-checkbox"
              bind:checked={captureEnabled}
              onchange={saveCaptureSettings}
            />
            <span class="capture-label">Save session back to this node</span>
          </label>

          {#if captureEnabled}
            <div class="capture-row capture-indent">
              <label class="field-label" for="capture-content">Content</label>
              <select
                id="capture-content"
                class="field-select capture-select"
                bind:value={captureContent}
                onchange={saveCaptureSettings}
              >
                {#each CONTENT_LEVELS as level (level.value)}
                  <option value={level.value}>{level.label}</option>
                {/each}
              </select>
            </div>

          {/if}
        </div>
      </details>

      {#if error}
        <div class="error-banner" role="alert">{error}</div>
      {/if}

      <button class="launch-button" onclick={launch} onkeydown={handleKeydown} disabled={launching}>
        {#if launching}
          <span class="spinner" aria-hidden="true"></span>
          Launching…
        {:else}
          Launch
        {/if}
      </button>
    </div>
  </div>
{/if}

<style>
  .pty-terminal-host {
    flex: 1;
    min-height: 0;
    overflow: hidden;
    background: hsl(222 47% 8%);
  }

  .pty-ended {
    flex: 1;
    display: flex;
    align-items: flex-start;
    justify-content: center;
    overflow-y: auto;
    padding: 2rem 1rem;
  }

  .pty-ended-card {
    display: flex;
    flex-direction: column;
    gap: 0.75rem;
    width: 100%;
    max-width: 32rem;
    padding: 1.25rem;
    border: 1px solid hsl(var(--border));
    border-radius: 0.5rem;
    background: hsl(var(--card));
  }

  .pty-ended-title {
    margin: 0;
    font-size: 0.9375rem;
    font-weight: 600;
    color: hsl(var(--foreground));
  }

  .pty-ended-meta {
    display: flex;
    align-items: center;
    gap: 0.5rem;
    margin: 0;
    font-size: 0.75rem;
  }

  .pty-ended-agent {
    color: hsl(var(--muted-foreground));
    font-family: ui-monospace, monospace;
  }

  .pty-ended-badge {
    text-transform: uppercase;
    letter-spacing: 0.05em;
    color: hsl(var(--muted-foreground));
    background: hsl(var(--muted));
    border-radius: 0.25rem;
    padding: 0 0.375rem;
  }

  .pty-ended-summary {
    margin: 0;
    font-size: 0.8125rem;
    line-height: 1.5;
    color: hsl(var(--foreground));
    white-space: pre-wrap;
  }

  .pty-ended-empty {
    margin: 0;
    font-size: 0.8125rem;
    color: hsl(var(--muted-foreground));
  }

  .pty-ended-transcript {
    border: 1px solid hsl(var(--border));
    border-radius: 0.375rem;
    overflow: hidden;
  }

  .pty-ended-transcript > summary {
    padding: 0.5rem 0.75rem;
    font-size: 0.8125rem;
    font-weight: 500;
    cursor: pointer;
    user-select: none;
    background: hsl(var(--muted) / 0.3);
    color: hsl(var(--foreground));
  }

  .pty-ended-transcript pre {
    margin: 0;
    padding: 0.75rem;
    max-height: 24rem;
    overflow: auto;
    font-family: ui-monospace, monospace;
    font-size: 0.75rem;
    line-height: 1.4;
    white-space: pre-wrap;
    word-break: break-word;
    color: hsl(var(--foreground));
  }

  .pty-config {
    flex: 1;
    display: flex;
    align-items: flex-start;
    justify-content: center;
    overflow-y: auto;
    padding: 2rem 1rem;
  }

  .pty-config-card {
    display: flex;
    flex-direction: column;
    gap: 0.875rem;
    width: 100%;
    max-width: 26rem;
    padding: 1.25rem;
    border: 1px solid hsl(var(--border));
    border-radius: 0.5rem;
    background: hsl(var(--card));
  }

  .pty-config-title {
    margin: 0;
    font-size: 0.9375rem;
    font-weight: 600;
    color: hsl(var(--foreground));
  }

  .pty-config-subtitle {
    margin: -0.5rem 0 0;
    font-size: 0.8125rem;
    color: hsl(var(--muted-foreground));
  }

  .field {
    display: flex;
    flex-direction: column;
    gap: 0.375rem;
  }

  .field-label {
    font-size: 0.8125rem;
    font-weight: 500;
    color: hsl(var(--foreground));
  }

  .field-select {
    padding: 0.5rem 0.625rem;
    border: 1px solid hsl(var(--border));
    border-radius: 0.375rem;
    background: hsl(var(--background));
    color: hsl(var(--foreground));
    font-size: 0.8125rem;
    font-family: inherit;
    transition: border-color 0.15s;
  }

  .field-select:focus {
    outline: none;
    border-color: hsl(var(--ring));
  }

  .field-select:disabled {
    opacity: 0.6;
    cursor: not-allowed;
  }

  .availability-banner {
    display: flex;
    flex-direction: column;
    gap: 0.25rem;
    padding: 0.5rem 0.75rem;
    border-radius: 0.375rem;
    font-size: 0.8125rem;
  }

  .availability-banner--warning {
    background: hsl(38 92% 50% / 0.1);
    border: 1px solid hsl(38 92% 50% / 0.35);
    color: hsl(32 95% 44%);
  }

  .availability-banner--ready {
    background: hsl(142 71% 45% / 0.1);
    border: 1px solid hsl(142 71% 45% / 0.3);
    color: hsl(142 71% 35%);
    flex-direction: row;
    align-items: center;
    gap: 0.4rem;
  }

  .availability-row {
    display: flex;
    align-items: flex-start;
    gap: 0.4rem;
  }

  .availability-icon {
    flex-shrink: 0;
    font-size: 0.75rem;
    margin-top: 0.05rem;
  }

  .install-hint {
    display: block;
    margin-top: 0.2rem;
    font-size: 0.75rem;
    opacity: 0.85;
    font-family: ui-monospace, monospace;
    word-break: break-all;
  }

  .capture-section {
    border: 1px solid hsl(var(--border));
    border-radius: 0.375rem;
    overflow: hidden;
  }

  .capture-summary {
    padding: 0.5rem 0.75rem;
    font-size: 0.8125rem;
    font-weight: 500;
    color: hsl(var(--foreground));
    cursor: pointer;
    user-select: none;
    background: hsl(var(--muted) / 0.3);
  }

  .capture-summary:hover {
    background: hsl(var(--muted) / 0.5);
  }

  .capture-body {
    display: flex;
    flex-direction: column;
    gap: 0.625rem;
    padding: 0.75rem;
  }

  .capture-row {
    display: flex;
    align-items: center;
    gap: 0.5rem;
    cursor: pointer;
  }

  .capture-indent {
    padding-left: 1.25rem;
  }

  .capture-checkbox {
    width: 14px;
    height: 14px;
    cursor: pointer;
    flex-shrink: 0;
  }

  .capture-label {
    font-size: 0.8125rem;
    color: hsl(var(--foreground));
  }

  .capture-select {
    flex: 1;
    margin-top: 0.25rem;
  }

  .error-banner {
    padding: 0.5rem 0.75rem;
    border-radius: 0.375rem;
    background: hsl(0 72% 51% / 0.1);
    border: 1px solid hsl(0 72% 51% / 0.3);
    color: hsl(0 72% 51%);
    font-size: 0.8125rem;
  }

  .launch-button {
    display: flex;
    align-items: center;
    justify-content: center;
    gap: 0.5rem;
    padding: 0.5rem 1rem;
    border: none;
    border-radius: 0.375rem;
    background: hsl(var(--primary));
    color: hsl(var(--primary-foreground));
    font-size: 0.875rem;
    font-weight: 500;
    cursor: pointer;
    transition: opacity 0.15s;
  }

  .launch-button:hover:not(:disabled) {
    opacity: 0.9;
  }

  .launch-button:disabled {
    opacity: 0.6;
    cursor: not-allowed;
  }

  .spinner {
    width: 14px;
    height: 14px;
    border: 2px solid hsl(var(--primary-foreground) / 0.3);
    border-top-color: hsl(var(--primary-foreground));
    border-radius: 50%;
    animation: spin 0.7s linear infinite;
  }

  @keyframes spin {
    to {
      transform: rotate(360deg);
    }
  }
</style>
