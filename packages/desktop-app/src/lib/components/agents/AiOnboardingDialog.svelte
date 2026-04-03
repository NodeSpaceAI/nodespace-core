<script lang="ts">
  import { modelStore, formatBytes } from '$lib/stores/model-store.svelte';
  import { agentStore } from '$lib/stores/agent-store.svelte';
  import { createLogger } from '$lib/utils/logger';

  const log = createLogger('AiOnboarding');

  let {
    open = false,
    onClose,
  }: {
    open: boolean;
    onClose: () => void;
  } = $props();

  type OnboardingPath = 'choose' | 'acp' | 'local';
  let currentPath = $state<OnboardingPath>('choose');
  let isDownloading = $state(false);

  const agents = $derived(agentStore.agents);
  const availableAgents = $derived(agentStore.availableAgents);
  const recommendedModel = $derived(modelStore.recommendedModel);
  const downloadProgress = $derived(modelStore.downloadProgress);

  function selectAcpPath() {
    currentPath = 'acp';
  }

  function selectLocalPath() {
    currentPath = 'local';
  }

  function goBack() {
    currentPath = 'choose';
  }

  function selectAcpAgent(agentId: string) {
    agentStore.selectAgent(agentId);
    log.info('ACP agent selected via onboarding', { agentId });
    onClose();
  }

  async function downloadAndStart() {
    if (!recommendedModel) return;
    isDownloading = true;
    try {
      await modelStore.downloadModel(recommendedModel.id);
      await modelStore.loadModel(recommendedModel.id);
      log.info('Model downloaded and loaded via onboarding', { modelId: recommendedModel.id });
      onClose();
    } catch {
      // Error handled by modelStore
    } finally {
      isDownloading = false;
    }
  }

  function handleBackdropClick(event: MouseEvent) {
    if (event.target === event.currentTarget) {
      onClose();
    }
  }

  function handleKeydown(event: KeyboardEvent) {
    if (event.key === 'Escape') {
      onClose();
    }
  }
</script>

{#if open}
  <!-- svelte-ignore a11y_no_noninteractive_element_interactions -->
  <div
    class="onboarding-backdrop"
    onclick={handleBackdropClick}
    onkeydown={handleKeydown}
    role="dialog"
    aria-modal="true"
    aria-label="AI Setup"
  >
    <div class="onboarding-dialog">
      <button class="close-button" onclick={onClose} aria-label="Close dialog">
        <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" width="18" height="18">
          <line x1="18" y1="6" x2="6" y2="18" />
          <line x1="6" y1="6" x2="18" y2="18" />
        </svg>
      </button>

      {#if currentPath === 'choose'}
        <div class="onboarding-header">
          <h2>Set up AI in NodeSpace</h2>
          <p>Choose how you want to interact with AI</p>
        </div>

        <div class="path-cards">
          <button class="path-card" onclick={selectAcpPath}>
            <div class="path-card-icon">
              <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.5" width="32" height="32">
                <path d="M12 2L2 7l10 5 10-5-10-5zM2 17l10 5 10-5M2 12l10 5 10-5" />
              </svg>
            </div>
            <h3>Use an existing AI provider</h3>
            <p>Connect to Claude Code, Gemini CLI, or other ACP-compatible agents</p>
            {#if availableAgents.length > 0}
              <span class="path-badge">{availableAgents.length} agent{availableAgents.length === 1 ? '' : 's'} detected</span>
            {/if}
          </button>

          <button class="path-card" onclick={selectLocalPath}>
            <div class="path-card-icon">
              <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.5" width="32" height="32">
                <rect x="4" y="4" width="16" height="16" rx="2" />
                <path d="M9 9h6M9 13h6M9 17h4" />
              </svg>
            </div>
            <h3>Download a local model</h3>
            <p>Run AI completely offline on your machine</p>
            {#if recommendedModel}
              <span class="path-badge">Recommended: {formatBytes(recommendedModel.size_bytes)}</span>
            {/if}
          </button>
        </div>

      {:else if currentPath === 'acp'}
        <div class="onboarding-header">
          <button class="back-button" onclick={goBack} aria-label="Go back">
            <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" width="16" height="16">
              <polyline points="15 18 9 12 15 6" />
            </svg>
          </button>
          <h2>Available AI Providers</h2>
          <p>Select an agent to start chatting</p>
        </div>

        <div class="agent-list">
          {#each agents as agent (agent.id)}
            <button
              class="agent-card"
              onclick={() => selectAcpAgent(agent.id)}
              disabled={!agent.available}
            >
              <div class="agent-card-left">
                <span class="availability-dot" class:available={agent.available}></span>
                <div>
                  <span class="agent-card-name">{agent.name}</span>
                  {#if agent.version}
                    <span class="agent-card-version">v{agent.version}</span>
                  {/if}
                </div>
              </div>
              <span class="agent-card-status" class:status-available={agent.available}>
                {agent.available ? 'Installed' : 'Not installed'}
              </span>
            </button>
          {/each}
          {#if agents.length === 0}
            <div class="no-agents">
              <p>No ACP agents detected on your system.</p>
              <p class="hint-text">Install Claude Code, Gemini CLI, or another ACP-compatible agent to get started.</p>
            </div>
          {/if}
        </div>

      {:else if currentPath === 'local'}
        <div class="onboarding-header">
          <button class="back-button" onclick={goBack} aria-label="Go back">
            <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" width="16" height="16">
              <polyline points="15 18 9 12 15 6" />
            </svg>
          </button>
          <h2>Download Local Model</h2>
          <p>Run AI privately on your machine</p>
        </div>

        {#if recommendedModel}
          <div class="model-recommendation">
            <div class="model-info">
              <span class="model-name">{recommendedModel.name}</span>
              <span class="model-details">
                {formatBytes(recommendedModel.size_bytes)} &middot; {recommendedModel.quantization}
              </span>
            </div>

            {#if isDownloading && downloadProgress[recommendedModel.id] !== undefined}
              <div class="download-progress">
                <div class="progress-bar">
                  <div
                    class="progress-fill"
                    style="width: {downloadProgress[recommendedModel.id]}%"
                  ></div>
                </div>
                <span class="progress-text">
                  {Math.round(downloadProgress[recommendedModel.id])}%
                </span>
              </div>
            {:else}
              <button class="download-button" onclick={downloadAndStart} disabled={isDownloading}>
                Download and Start
              </button>
            {/if}

            <p class="model-hint">You can switch models later in Settings.</p>
          </div>
        {:else}
          <div class="no-agents">
            <p>No models available. Please check your configuration.</p>
          </div>
        {/if}
      {/if}
    </div>
  </div>
{/if}

<style>
  .onboarding-backdrop {
    position: fixed;
    inset: 0;
    background: hsl(0 0% 0% / 0.5);
    display: flex;
    align-items: center;
    justify-content: center;
    z-index: 100;
    padding: 1rem;
  }

  .onboarding-dialog {
    background: hsl(var(--background));
    border: 1px solid hsl(var(--border));
    border-radius: 0.75rem;
    padding: 2rem;
    max-width: 32rem;
    width: 100%;
    position: relative;
    max-height: 85vh;
    overflow-y: auto;
    box-shadow: 0 12px 40px hsl(0 0% 0% / 0.15);
  }

  .close-button {
    position: absolute;
    top: 0.75rem;
    right: 0.75rem;
    background: none;
    border: none;
    cursor: pointer;
    color: hsl(var(--muted-foreground));
    padding: 0.25rem;
    border-radius: 0.25rem;
    display: flex;
    align-items: center;
  }

  .close-button:hover {
    color: hsl(var(--foreground));
  }

  .onboarding-header {
    margin-bottom: 1.5rem;
    position: relative;
  }

  .onboarding-header h2 {
    font-size: 1.25rem;
    font-weight: 600;
    margin: 0 0 0.375rem;
    color: hsl(var(--foreground));
  }

  .onboarding-header p {
    font-size: 0.875rem;
    color: hsl(var(--muted-foreground));
    margin: 0;
  }

  .back-button {
    background: none;
    border: none;
    cursor: pointer;
    color: hsl(var(--muted-foreground));
    display: inline-flex;
    align-items: center;
    margin-bottom: 0.5rem;
    padding: 0.25rem;
    border-radius: 0.25rem;
  }

  .back-button:hover {
    color: hsl(var(--foreground));
  }

  .path-cards {
    display: flex;
    flex-direction: column;
    gap: 0.75rem;
  }

  .path-card {
    display: flex;
    flex-direction: column;
    align-items: flex-start;
    gap: 0.5rem;
    padding: 1.25rem;
    border: 1px solid hsl(var(--border));
    border-radius: 0.5rem;
    background: hsl(var(--background));
    cursor: pointer;
    text-align: left;
    transition: border-color 0.15s, box-shadow 0.15s;
    width: 100%;
  }

  .path-card:hover {
    border-color: hsl(var(--ring));
    box-shadow: 0 0 0 2px hsl(var(--ring) / 0.1);
  }

  .path-card-icon {
    color: hsl(var(--primary));
    margin-bottom: 0.25rem;
  }

  .path-card h3 {
    font-size: 0.9375rem;
    font-weight: 600;
    margin: 0;
    color: hsl(var(--foreground));
  }

  .path-card p {
    font-size: 0.8125rem;
    color: hsl(var(--muted-foreground));
    margin: 0;
    line-height: 1.4;
  }

  .path-badge {
    font-size: 0.75rem;
    background: hsl(var(--primary) / 0.1);
    color: hsl(var(--primary));
    padding: 0.125rem 0.5rem;
    border-radius: 9999px;
    font-weight: 500;
  }

  /* Agent list */
  .agent-list {
    display: flex;
    flex-direction: column;
    gap: 0.5rem;
  }

  .agent-card {
    display: flex;
    align-items: center;
    justify-content: space-between;
    padding: 0.75rem 1rem;
    border: 1px solid hsl(var(--border));
    border-radius: 0.375rem;
    background: hsl(var(--background));
    cursor: pointer;
    width: 100%;
    transition: border-color 0.15s;
  }

  .agent-card:hover:not(:disabled) {
    border-color: hsl(var(--ring));
  }

  .agent-card:disabled {
    opacity: 0.6;
    cursor: not-allowed;
  }

  .agent-card-left {
    display: flex;
    align-items: center;
    gap: 0.5rem;
  }

  .agent-card-name {
    font-size: 0.875rem;
    font-weight: 500;
    color: hsl(var(--foreground));
  }

  .agent-card-version {
    font-size: 0.75rem;
    color: hsl(var(--muted-foreground));
    margin-left: 0.375rem;
  }

  .agent-card-status {
    font-size: 0.75rem;
    color: hsl(var(--muted-foreground));
  }

  .agent-card-status.status-available {
    color: hsl(142 76% 36%);
  }

  .availability-dot {
    width: 8px;
    height: 8px;
    border-radius: 50%;
    background: hsl(var(--muted-foreground) / 0.3);
    flex-shrink: 0;
  }

  .availability-dot.available {
    background: hsl(142 76% 36%);
  }

  .no-agents {
    text-align: center;
    padding: 1.5rem 1rem;
    color: hsl(var(--muted-foreground));
    font-size: 0.875rem;
  }

  .no-agents p {
    margin: 0 0 0.5rem;
  }

  .hint-text {
    font-size: 0.8125rem;
    color: hsl(var(--muted-foreground) / 0.8);
  }

  /* Local model */
  .model-recommendation {
    display: flex;
    flex-direction: column;
    gap: 1rem;
    padding: 1.25rem;
    border: 1px solid hsl(var(--border));
    border-radius: 0.5rem;
    background: hsl(var(--muted) / 0.3);
  }

  .model-info {
    display: flex;
    flex-direction: column;
    gap: 0.25rem;
  }

  .model-name {
    font-size: 1rem;
    font-weight: 600;
    color: hsl(var(--foreground));
  }

  .model-details {
    font-size: 0.8125rem;
    color: hsl(var(--muted-foreground));
  }

  .download-button {
    padding: 0.625rem 1.25rem;
    border-radius: 0.375rem;
    border: none;
    background: hsl(var(--primary));
    color: hsl(var(--primary-foreground));
    font-size: 0.875rem;
    font-weight: 500;
    cursor: pointer;
    transition: opacity 0.15s;
    align-self: flex-start;
  }

  .download-button:hover:not(:disabled) {
    opacity: 0.9;
  }

  .download-button:disabled {
    opacity: 0.5;
    cursor: not-allowed;
  }

  .download-progress {
    display: flex;
    align-items: center;
    gap: 0.75rem;
  }

  .progress-bar {
    flex: 1;
    height: 8px;
    border-radius: 9999px;
    background: hsl(var(--muted));
    overflow: hidden;
  }

  .progress-fill {
    height: 100%;
    border-radius: 9999px;
    background: hsl(var(--primary));
    transition: width 0.2s ease;
  }

  .progress-text {
    font-size: 0.8125rem;
    font-weight: 500;
    color: hsl(var(--foreground));
    min-width: 2.5rem;
    text-align: right;
  }

  .model-hint {
    font-size: 0.75rem;
    color: hsl(var(--muted-foreground));
    margin: 0;
  }
</style>
