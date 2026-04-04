<script lang="ts">
  import { onMount } from 'svelte';
  import { modelStore, formatBytes } from '$lib/stores/model-store.svelte';
  import { createLogger } from '$lib/utils/logger';

  const log = createLogger('ModelManager');

  const models = $derived(modelStore.models);
  const downloadProgress = $derived(modelStore.downloadProgress);
  const loadedModelId = $derived(modelStore.loadedModelId);
  const recommendedModel = $derived(modelStore.recommendedModel);
  const isLoading = $derived(modelStore.isLoading);

  onMount(() => {
    if (models.length === 0) {
      modelStore.refreshModels();
    }
    log.debug('ModelManager mounted');
  });

  async function handleDownload(modelId: string) {
    await modelStore.downloadModel(modelId);
  }

  async function handleLoad(modelId: string) {
    await modelStore.loadModel(modelId);
  }

  async function handleUnload() {
    await modelStore.unloadModel();
  }

  async function handleDelete(modelId: string) {
    await modelStore.deleteModel(modelId);
  }

  function getStatusLabel(status: string): string {
    switch (status) {
      case 'not_downloaded': return 'Not downloaded';
      case 'downloading': return 'Downloading';
      case 'verifying': return 'Verifying';
      case 'ready': return 'Ready';
      case 'loaded': return 'Loaded';
      case 'error': return 'Error';
      default: return status;
    }
  }

  function getStatusColor(status: string): string {
    switch (status) {
      case 'loaded': return 'status-loaded';
      case 'ready': return 'status-ready';
      case 'downloading':
      case 'verifying': return 'status-progress';
      case 'error': return 'status-error';
      default: return 'status-default';
    }
  }
</script>

<div class="model-manager">
  <div class="model-manager-header">
    <h3>Local Models</h3>
    <button class="refresh-button" onclick={() => modelStore.refreshModels()} disabled={isLoading} aria-label="Refresh models">
      <svg class:spinning={isLoading} viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" width="16" height="16">
        <path d="M21.5 2v6h-6M2.5 22v-6h6M2 11.5a10 10 0 0 1 18.8-4.3M22 12.5a10 10 0 0 1-18.8 4.2" />
      </svg>
    </button>
  </div>

  {#if models.length === 0 && !isLoading}
    <div class="empty-state">
      <p>No models in catalog. Click refresh to load available models.</p>
    </div>
  {:else}
    <div class="model-list">
      {#each models as model (model.id)}
        {@const isRecommended = recommendedModel?.id === model.id}
        {@const isLoaded = model.id === loadedModelId}
        {@const progress = downloadProgress[model.id]}

        <div class="model-card" class:model-loaded={isLoaded}>
          <div class="model-card-header">
            <div class="model-card-title">
              <span class="model-name">{model.name}</span>
              {#if isRecommended}
                <span class="recommended-badge">Recommended</span>
              {/if}
            </div>
            <span class="status-badge {getStatusColor(model.status.status)}">
              {getStatusLabel(model.status.status)}
            </span>
          </div>

          <div class="model-card-meta">
            <span>{formatBytes(model.size_bytes)}</span>
            <span>&middot;</span>
            <span>{model.quantization}</span>
          </div>

          {#if progress !== undefined}
            <div class="model-progress">
              <div class="progress-bar">
                <div class="progress-fill" style="width: {progress}%"></div>
              </div>
              <span class="progress-text">{Math.round(progress)}%</span>
            </div>
          {/if}

          <div class="model-card-actions">
            {#if model.status.status === 'not_downloaded'}
              <button class="action-button primary" onclick={() => handleDownload(model.id)}>
                Download
              </button>
            {:else if model.status.status === 'downloading'}
              <button class="action-button" onclick={() => modelStore.cancelDownload(model.id)}>
                Cancel
              </button>
            {:else if model.status.status === 'ready'}
              <button class="action-button primary" onclick={() => handleLoad(model.id)}>
                Load
              </button>
              <button class="action-button destructive" onclick={() => handleDelete(model.id)}>
                Delete
              </button>
            {:else if model.status.status === 'loaded'}
              <button class="action-button" onclick={handleUnload}>
                Unload
              </button>
            {:else if model.status.status === 'error'}
              <button class="action-button primary" onclick={() => handleDownload(model.id)}>
                Retry
              </button>
            {/if}
          </div>
        </div>
      {/each}
    </div>
  {/if}
</div>

<style>
  .model-manager {
    display: flex;
    flex-direction: column;
    gap: 1rem;
  }

  .model-manager-header {
    display: flex;
    align-items: center;
    justify-content: space-between;
  }

  .model-manager-header h3 {
    font-size: 1rem;
    font-weight: 600;
    margin: 0;
    color: hsl(var(--foreground));
  }

  .refresh-button {
    display: flex;
    align-items: center;
    justify-content: center;
    width: 2rem;
    height: 2rem;
    border-radius: 0.375rem;
    border: 1px solid hsl(var(--border));
    background: hsl(var(--background));
    cursor: pointer;
    color: hsl(var(--muted-foreground));
    transition: color 0.15s;
  }

  .refresh-button:hover:not(:disabled) {
    color: hsl(var(--foreground));
  }

  .refresh-button:disabled {
    opacity: 0.5;
    cursor: not-allowed;
  }

  .spinning {
    animation: spin 1s linear infinite;
  }

  @keyframes spin {
    from { transform: rotate(0deg); }
    to { transform: rotate(360deg); }
  }

  .empty-state {
    text-align: center;
    padding: 2rem 1rem;
    color: hsl(var(--muted-foreground));
    font-size: 0.875rem;
  }

  .empty-state p {
    margin: 0;
  }

  .model-list {
    display: flex;
    flex-direction: column;
    gap: 0.75rem;
  }

  .model-card {
    border: 1px solid hsl(var(--border));
    border-radius: 0.5rem;
    padding: 1rem;
    background: hsl(var(--background));
    display: flex;
    flex-direction: column;
    gap: 0.5rem;
  }

  .model-loaded {
    border-color: hsl(var(--primary) / 0.5);
    background: hsl(var(--primary) / 0.03);
  }

  .model-card-header {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 0.5rem;
  }

  .model-card-title {
    display: flex;
    align-items: center;
    gap: 0.5rem;
  }

  .model-name {
    font-weight: 500;
    font-size: 0.875rem;
    color: hsl(var(--foreground));
  }

  .recommended-badge {
    font-size: 0.6875rem;
    background: hsl(var(--primary) / 0.1);
    color: hsl(var(--primary));
    padding: 0.0625rem 0.375rem;
    border-radius: 9999px;
    font-weight: 500;
  }

  .status-badge {
    font-size: 0.75rem;
    padding: 0.125rem 0.5rem;
    border-radius: 9999px;
    font-weight: 500;
  }

  .status-default {
    background: hsl(var(--muted));
    color: hsl(var(--muted-foreground));
  }

  .status-ready {
    background: hsl(142 76% 36% / 0.1);
    color: hsl(142 76% 36%);
  }

  .status-loaded {
    background: hsl(var(--primary) / 0.1);
    color: hsl(var(--primary));
  }

  .status-progress {
    background: hsl(45 93% 47% / 0.1);
    color: hsl(45 93% 47%);
  }

  .status-error {
    background: hsl(var(--destructive) / 0.1);
    color: hsl(var(--destructive));
  }

  .model-card-meta {
    display: flex;
    gap: 0.375rem;
    font-size: 0.8125rem;
    color: hsl(var(--muted-foreground));
  }

  .model-progress {
    display: flex;
    align-items: center;
    gap: 0.625rem;
  }

  .progress-bar {
    flex: 1;
    height: 6px;
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
    font-size: 0.75rem;
    font-weight: 500;
    color: hsl(var(--foreground));
    min-width: 2rem;
    text-align: right;
  }

  .model-card-actions {
    display: flex;
    gap: 0.5rem;
    margin-top: 0.25rem;
  }

  .action-button {
    padding: 0.375rem 0.75rem;
    font-size: 0.8125rem;
    border-radius: 0.25rem;
    border: 1px solid hsl(var(--border));
    background: hsl(var(--background));
    color: hsl(var(--foreground));
    cursor: pointer;
    font-weight: 500;
    transition: background 0.15s;
  }

  .action-button:hover {
    background: hsl(var(--accent));
  }

  .action-button.primary {
    background: hsl(var(--primary));
    color: hsl(var(--primary-foreground));
    border-color: hsl(var(--primary));
  }

  .action-button.primary:hover {
    opacity: 0.9;
  }

  .action-button.destructive {
    color: hsl(var(--destructive));
    border-color: hsl(var(--destructive) / 0.5);
  }

  .action-button.destructive:hover {
    background: hsl(var(--destructive) / 0.1);
  }
</style>
