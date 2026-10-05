<!--
  ai-chat-pty-session — the terminal sub-view of an `ai-chat-pty` node, composed
  by AiChatPtyNodeViewer. Not a Node/Viewer component (it's an internal helper,
  named like ChatMessage/ChatInput), so it carries no *Node/*Viewer/*View suffix.

  A PTY agent session IS an `ai-chat-pty` node (ADR-088). This helper renders a
  launch config (harness picker, project, Launch) when no session is running,
  the embedded xterm terminal (via pty-terminal.svelte) while it runs, and a
  read-only summary once it ends. The node already exists; capture backfills
  it at session end via the node_id passed to launch.

  A session launched for a project runs in that project's folder on this
  machine (ADR-093 §8). The folder is asked for here the first time and stored
  on the project as its machine-bound `checkout_path`; the daemon checks that
  it exists before it stores it.
-->

<script lang="ts">
  import { onMount } from 'svelte';
  import { listen, type UnlistenFn } from '@tauri-apps/api/event';
  import { open as openDialog } from '@tauri-apps/plugin-dialog';
  import PtyTerminal from '$lib/components/agent/pty-terminal.svelte';
  import { backendAdapter } from '$lib/services/backend-adapter';
  import { sharedNodeStore } from '$lib/services/shared-node-store.svelte';
  import type { AiChatPtyNode } from '$lib/types/ai-chat-node';
  import { isProjectNode, type ProjectNode } from '$lib/types/project-node';
  import {
    getCaptureSettings,
    updateCaptureSettings,
    ptyCheckAgentAvailability,
    ptyLaunchSession,
    ptyListSessions,
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
    { id: 'antigravity', label: 'Antigravity CLI' },
    { id: 'pi', label: 'Pi' },
    { id: 'opencode', label: 'OpenCode' },
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

  // The typed fields are read from the TOP-LEVEL promoted keys, the same wire
  // contract the native viewer documents for turnStatus.
  const node = $derived(sharedNodeStore.getNode(nodeId) as unknown as AiChatPtyNode | undefined);

  // Latches when this viewer's session exits, so the UI flips to the ended
  // state live without waiting for a reload (the daemon records the session's
  // end out-of-band, not through sharedNodeStore).
  let sessionEnded = $state(false);

  // Set when the user explicitly chooses "Start new session" from the ended
  // view, forcing the config step while the node still reads as ended.
  let configuring = $state(false);

  // The node's running PTY session: the one this viewer launched, or the one
  // found running for the node when the viewer opened. It is the daemon's
  // handle on a live process, so it is never stored on the node; the node's
  // own `session_id` is the harness's id for the conversation, recorded when
  // the session ends.
  let activeSessionId = $state<string | null>(null);

  // True until the daemon has been asked whether a session is running for
  // this node, so an open session's terminal is not preceded by a flash of
  // the launch form.
  let findingSession = $state(true);

  // The session has ended if the node is marked ended, or we observed its exit
  // this session.
  const isEnded = $derived(!configuring && (sessionEnded || node?.sessionStatus === 'ended'));

  const agentType = $derived(node?.agent || null);
  const summary = $derived(node?.summary ?? null);
  const transcript = $derived(node?.transcript ?? null);

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

  /** Return to the config step to launch a fresh session on this node. */
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

  // The project the session is launched for; '' launches it for none, in a
  // private session folder.
  let projects = $state<ProjectNode[]>([]);
  let selectedProjectId = $state('');
  // The project's folder on this machine, as the form shows it. It starts as
  // the folder the project already has, and is sent only when it differs.
  let projectFolder = $state('');

  const selectedProject = $derived(
    projects.find((project) => project.id === selectedProjectId) ?? null
  );
  const storedFolder = $derived(selectedProject?.checkoutPath ?? '');
  const isFolderMissing = $derived(selectedProject !== null && projectFolder.trim() === '');

  onMount(async () => {
    // Pre-select the harness the node already names (chosen in the header
    // AiChatModelSelector before the chat became a PTY chat); otherwise keep
    // the 'claude-code' default.
    if (node?.agent) {
      selectedAgent = node.agent;
    }

    await Promise.all([attachToRunningSession(), loadLaunchSettings(), loadProjects()]);
  });

  async function loadProjects(): Promise<void> {
    try {
      const nodes = await backendAdapter.queryNodes({ nodeType: 'project' });
      projects = Array.isArray(nodes) ? nodes.filter(isProjectNode) : [];
    } catch (e) {
      log.warn('Failed to load projects', e);
    }
  }

  function selectProject(id: string): void {
    selectedProjectId = id;
    projectFolder = projects.find((project) => project.id === id)?.checkoutPath ?? '';
    error = null;
  }

  async function browseForFolder(): Promise<void> {
    try {
      const picked = await openDialog({
        directory: true,
        multiple: false,
        title: 'Choose the project folder',
        defaultPath: projectFolder.trim() || undefined,
      });
      if (typeof picked === 'string') {
        projectFolder = picked;
      }
    } catch (e) {
      log.warn('Failed to choose a project folder', e);
    }
  }

  async function loadLaunchSettings(): Promise<void> {
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
  }

  /**
   * Re-attach to the session already running for this node, if there is one:
   * the daemon owns the PTY and keeps it running while the viewer is closed
   * (ADR-032). A node whose session has ended has none to find.
   */
  async function attachToRunningSession(): Promise<void> {
    try {
      if (node?.sessionStatus !== 'ended') {
        const { sessions } = await ptyListSessions();
        // The latest, should more than one be running for the node (two
        // panes on it both launched).
        const running = sessions
          .filter((session) => session.nodeId === nodeId)
          .sort((a, b) => b.startedAt - a.startedAt)[0];
        if (running && !activeSessionId) {
          activeSessionId = running.sessionId;
        }
      }
    } catch (e) {
      log.warn('Failed to look up the running session', e);
    } finally {
      findingSession = false;
    }
  }

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
    if (isFolderMissing) return;
    launching = true;
    error = null;
    try {
      const folder = projectFolder.trim();
      const result = await ptyLaunchSession({
        agentType: selectedAgent,
        prompt: null,
        cols: 80,
        rows: 24,
        nodeId,
        projectId: selectedProject?.id ?? null,
        // Named only when it is new or changed: the daemon then checks it and
        // stores it on the project for this machine.
        projectFolder: selectedProject && folder !== storedFolder ? folder : null,
      });
      activeSessionId = result.sessionId;
      configuring = false;
      if (selectedProject) {
        // The daemon stored the folder; show it without waiting for a reload.
        projects = projects.map((project) =>
          project.id === selectedProject.id ? { ...project, checkoutPath: folder } : project
        );
      }

      // Record the chosen agent on the node up front so the node reflects its
      // mode immediately. The daemon records the session's end (status, exit
      // code, the harness's session id) on the node via the node_id passed
      // above, and the summary and transcript too when capture is on.
      // Canonical snake_case keys, matching the schema's declared field names:
      // the chat family has no typed write command, so whatever key this
      // object uses reaches storage verbatim. A patch, not a rewrite — the
      // store merges it onto the node's properties.
      sharedNodeStore.updateNode(
        nodeId,
        {
          properties: {
            agent: selectedAgent,
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
       session was about (capture is a reference, not a transcript a terminal
       can replay) + an affordance to start a fresh session. -->
  <div class="pty-ended">
    <div class="pty-ended-card">
      <h3 class="pty-ended-title">Session ended</h3>
      <p class="pty-ended-meta">
        {#if agentType}<span class="pty-ended-agent">{agentType}</span>{/if}
        <span class="pty-ended-badge">ended</span>
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
{:else if activeSessionId}
  <!-- Active session: the embedded terminal IS the node's viewer. -->
  <div class="pty-terminal-host">
    {#key activeSessionId}
      <PtyTerminal sessionId={activeSessionId} />
    {/key}
  </div>
{:else if findingSession}
  <!-- Asking the daemon for the node's running session: neither the terminal
       nor the launch form yet. -->
  <div class="pty-terminal-host" aria-busy="true"></div>
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

      <div class="field">
        <label class="field-label" for="project-select">Project</label>
        <select
          id="project-select"
          class="field-select"
          value={selectedProjectId}
          onchange={(event) => selectProject(event.currentTarget.value)}
          disabled={launching}
        >
          <option value="">No project</option>
          {#each projects as project (project.id)}
            <option value={project.id}>{project.title ?? project.content}</option>
          {/each}
        </select>
        <p class="field-hint">
          {#if selectedProject}
            The session runs in the project's folder on this machine.
          {:else}
            Without a project the session runs in a private folder of its own.
          {/if}
        </p>
      </div>

      {#if selectedProject}
        <div class="field">
          <label class="field-label" for="project-folder">Folder on this machine</label>
          <div class="folder-row">
            <input
              id="project-folder"
              class="field-select folder-input"
              type="text"
              placeholder="/path/to/checkout"
              autocomplete="off"
              spellcheck="false"
              bind:value={projectFolder}
              disabled={launching}
            />
            <button class="browse-button" type="button" onclick={browseForFolder} disabled={launching}>
              Browse…
            </button>
          </div>
          <p class="field-hint">
            {#if storedFolder}
              Remembered for this machine only. Change it here or in the project's properties.
            {:else}
              Where this project's checkout is. It is remembered for this machine only.
            {/if}
          </p>
        </div>
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

      <button
        class="launch-button"
        onclick={launch}
        onkeydown={handleKeydown}
        disabled={launching || isFolderMissing}
      >
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
  }

  .field-select:focus {
    outline: none;
    border-color: hsl(var(--ring));
  }

  .field-select:disabled {
    opacity: 0.6;
    cursor: not-allowed;
  }

  .field-hint {
    margin: 0;
    font-size: 0.75rem;
    color: hsl(var(--muted-foreground));
  }

  .folder-row {
    display: flex;
    gap: 0.5rem;
  }

  .folder-input {
    flex: 1;
    min-width: 0;
    font-family: ui-monospace, monospace;
  }

  .browse-button {
    flex-shrink: 0;
    padding: 0.5rem 0.75rem;
    border: 1px solid hsl(var(--border));
    border-radius: 0.375rem;
    background: hsl(var(--background));
    color: hsl(var(--foreground));
    font-size: 0.8125rem;
    font-family: inherit;
    cursor: pointer;
  }

  .browse-button:hover:not(:disabled) {
    background: hsl(var(--muted) / 0.5);
  }

  .browse-button:disabled {
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
  }

  .launch-button:hover:not(:disabled) {
    background: hsl(var(--primary-hover));
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
  }
</style>
