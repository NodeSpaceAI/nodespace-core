<script lang="ts">
  /* global HTMLSelectElement */
  import { onMount } from 'svelte';
  import { modelStore, formatBytes } from '$lib/stores/model-store.svelte';
  import {
    getOpenAiConfigs,
    saveOpenAiConfigs,
    getDefaultModelSelection,
    saveDefaultModelSelection,
    type ModelSelection,
  } from '$lib/stores/settings.svelte';
  import {
    chatModelList,
    getOpenAiCompatConfigsFromDaemon,
  } from '$lib/services/tauri-commands';
  import type { OpenAiCompatConfig } from '$lib/types/ai-chat-node';
  import type { ModelFamily } from '$lib/types/agent-types';
  import { createLogger } from '$lib/utils/logger';

  const log = createLogger('ModelManager');

  // --- Local model state (Ministral 8B only) ---
  const models = $derived(modelStore.models);
  const downloadProgress = $derived(modelStore.downloadProgress);
  const isLoading = $derived(modelStore.isLoading);
  const systemRamGb = $derived(modelStore.systemRamGb);

  // Settings shows the curated set already filtered at the Tauri layer
  // (see EXPOSED_GGUF_MODEL_IDS in chat_models.rs); families listed here must
  // match whatever that allowlist currently exposes.
  const LOCAL_FAMILIES: ModelFamily[] = ['gemma4'];
  const localModels = $derived(models.filter((m) => LOCAL_FAMILIES.includes(m.family)));

  // The global notice fires when NOTHING fits (the smallest exposed model's
  // own requirement), not a flat constant -- otherwise a machine that clears
  // one model's floor but not another's (e.g. E4B's 16GB but not 26B-A4B's
  // 32GB) would see every card dimmed by a bar that doesn't apply to all of
  // them. Per-card dimming below uses each model's own min_memory_gb.
  const minRequiredGb = $derived(
    localModels.length > 0 ? Math.min(...localModels.map((m) => m.min_memory_gb)) : 0
  );
  const ramTooLow = $derived(systemRamGb > 0 && systemRamGb < minRequiredGb);

  // --- Models discovered at configured OpenAI-compatible endpoints ---
  // The daemon queries each endpoint's /models and returns one row per model,
  // so "did it answer" is simply whether any rows came back.
  let remoteModels = $state<{ id: string; name: string }[]>([]);
  let remoteChecking = $state(true);

  async function refreshRemoteModels(forceRefresh = false) {
    remoteChecking = true;
    try {
      const list = await chatModelList(forceRefresh);
      remoteModels = list
        .filter((m) => m.backend === 'openai-compat')
        .map((m) => ({ id: m.id, name: m.name }));
    } catch (e) {
      log.warn('Failed to list remote models', e);
      remoteModels = [];
    } finally {
      buildDefaultOptions();
      remoteChecking = false;
    }
  }

  // --- OpenAI-compat configs ---
  let openAiConfigs = $state<OpenAiCompatConfig[]>([]);
  let editingConfig = $state<OpenAiCompatConfig | null>(null);
  let editForm = $state({ name: '', baseUrl: '', apiKey: '', model: '' });
  let isNewConfig = $state(false);

  // --- Default model ---
  let defaultModel = $state<ModelSelection | null>(null);
  let availableSelectionsForDefault = $state<{ label: string; value: string }[]>([]);

  function encodeSelection(s: ModelSelection): string {
    // openai-compat: the daemon's own id, fully qualified as
    //   "openai-compat:<uuid>" or "openai-compat:<uuid>:<model>".
    // native: "native:<model-id>"
    //
    // `modelId` is normalized rather than passed through: a ModelSelection can
    // legitimately carry a bare config UUID (older persisted defaults stored
    // one, and configId is the only id a config with no discovered models
    // has). Returning that unqualified would match no <option>, so the select
    // would silently fall back to "None" and quietly forget the saved default.
    if (s.provider === 'openai-compat') {
      return s.modelId.startsWith('openai-compat:')
        ? s.modelId
        : `openai-compat:${s.configId ?? s.modelId}`;
    }
    return `native:${s.modelId}`;
  }
  function decodeSelection(v: string): ModelSelection | null {
    if (v.startsWith('native:')) {
      return { provider: 'native', modelId: v.slice('native:'.length) };
    }
    if (v.startsWith('openai-compat:')) {
      // The config UUID is the segment up to the FIRST colon; a model name may
      // itself contain colons ("mistral:7b"), so the rest is not part of it.
      const configId = v.slice('openai-compat:'.length).split(':')[0];
      return { provider: 'openai-compat', modelId: v, configId };
    }
    return null;
  }

  onMount(async () => {
    if (models.length === 0) modelStore.refreshModels();
    // Show the local cache immediately, then refresh from the daemon (source
    // of truth) — avoids a blank list while the round-trip is in flight.
    openAiConfigs = getOpenAiConfigs();
    defaultModel = getDefaultModelSelection();
    try {
      const daemonConfigs = await getOpenAiCompatConfigsFromDaemon();
      openAiConfigs = daemonConfigs.map((c) => ({
        id: c.id,
        name: c.name,
        baseUrl: c.baseUrl,
        apiKey: c.apiKey,
        model: c.model,
      }));
      buildDefaultOptions();
    } catch (e) {
      log.warn('Failed to refresh OpenAI-compat configs from daemon', e);
    }

    await refreshRemoteModels();
  });

  // config write helpers below force a refresh, since they're the one
  // place a stale discovery cache is user-visible as a bug: the endpoint
  // set just changed underneath it.

  function buildDefaultOptions() {
    const opts: { label: string; value: string }[] = [];
    // Local models
    for (const m of models.filter((m) => LOCAL_FAMILIES.includes(m.family))) {
      if (m.status.status === 'ready' || m.status.status === 'loaded') {
        opts.push({ label: `Local — ${m.name}`, value: encodeSelection({ provider: 'native', modelId: m.id }) });
      }
    }
    // Remote models — the daemon ID is already fully qualified. Discovery
    // covers every endpoint that answered, so a config only earns its own
    // fallback row when it contributed no discovered models (endpoint down, or
    // a server with no /models listing). Otherwise it would appear twice.
    for (const m of remoteModels) {
      opts.push({
        label: m.name,
        value: encodeSelection({ provider: 'openai-compat', modelId: m.id }),
      });
    }
    const discoveredConfigIds = new Set(
      remoteModels.map((m) => m.id.slice('openai-compat:'.length).split(':')[0])
    );
    for (const c of openAiConfigs) {
      if (discoveredConfigIds.has(c.id)) continue;
      opts.push({
        label: c.name,
        value: encodeSelection({
          provider: 'openai-compat',
          modelId: `openai-compat:${c.id}`,
          configId: c.id,
        }),
      });
    }
    availableSelectionsForDefault = opts;
  }

  // --- OpenAI-compat CRUD ---
  function startAdd() {
    isNewConfig = true;
    editForm = { name: '', baseUrl: '', apiKey: '', model: '' };
    editingConfig = {
      id: globalThis.crypto.randomUUID(),
      name: '',
      baseUrl: '',
      apiKey: '',
      model: '',
    };
  }

  function startEdit(config: OpenAiCompatConfig) {
    isNewConfig = false;
    editingConfig = config;
    editForm = {
      name: config.name,
      baseUrl: config.baseUrl,
      apiKey: config.apiKey,
      model: config.model,
    };
  }

  async function saveConfig() {
    if (!editingConfig) return;
    const updated = { ...editingConfig, ...editForm };
    if (isNewConfig) {
      openAiConfigs = [...openAiConfigs, updated];
    } else {
      openAiConfigs = openAiConfigs.map((c) => (c.id === updated.id ? updated : c));
    }
    await saveOpenAiConfigs(openAiConfigs);
    editingConfig = null;
    buildDefaultOptions();
    await refreshRemoteModels(true);
  }

  async function deleteConfig(id: string) {
    openAiConfigs = openAiConfigs.filter((c) => c.id !== id);
    await saveOpenAiConfigs(openAiConfigs);
    if (defaultModel?.configId === id) {
      defaultModel = null;
      saveDefaultModelSelection(null);
    }
    buildDefaultOptions();
    await refreshRemoteModels(true);
  }

  function cancelEdit() {
    editingConfig = null;
  }

  function handleDefaultChange(e: Event) {
    const val = (e.target as HTMLSelectElement).value;
    if (!val) {
      defaultModel = null;
      saveDefaultModelSelection(null);
      return;
    }
    const decoded = decodeSelection(val);
    defaultModel = decoded;
    saveDefaultModelSelection(decoded);
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

  function getStatusClass(status: string): string {
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

  <!-- ── Local ──────────────────────────────────────────────────── -->
  <section class="mm-section">
    <div class="mm-section-header">
      <h3>Local</h3>
      <button
        class="refresh-btn"
        onclick={() => modelStore.refreshModels()}
        disabled={isLoading}
        aria-label="Refresh local models"
      >
        <svg class:spinning={isLoading} viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" width="14" height="14">
          <path d="M21.5 2v6h-6M2.5 22v-6h6M2 11.5a10 10 0 0 1 18.8-4.3M22 12.5a10 10 0 0 1-18.8 4.2" />
        </svg>
      </button>
    </div>

    {#if ramTooLow}
      <p class="mm-notice mm-notice--warn">
        Your machine has {systemRamGb} GB RAM. Local models require at least {minRequiredGb} GB.
        Use a remote endpoint instead.
      </p>
    {/if}

    {#if localModels.length === 0 && !isLoading}
      <p class="mm-empty">No local models found.</p>
    {:else}
      {#each localModels as m (m.id)}
        {@const progress = downloadProgress[m.id]}
        {@const modelRamTooLow = systemRamGb > 0 && systemRamGb < m.min_memory_gb}
        <div class="model-card" class:model-card--dim={modelRamTooLow}>
          <div class="model-card-top">
            <div class="model-card-info">
              <span class="model-name">{m.name}</span>
              <span class="model-meta">
                {formatBytes(m.size_bytes)}{m.quantization ? ` · ${m.quantization}` : ''}
                {#if m.min_memory_gb > 0} · Requires {m.min_memory_gb} GB RAM{/if}
              </span>
            </div>
            <span class="status-badge {getStatusClass(m.status.status)}">
              {getStatusLabel(m.status.status)}
            </span>
          </div>

          {#if progress !== undefined}
            <div class="progress-row">
              <div class="progress-bar"><div class="progress-fill" style="width: {progress}%"></div></div>
              <span class="progress-label">{Math.round(progress)}%</span>
            </div>
          {/if}

          <div class="model-card-actions">
            {#if m.status.status === 'not_downloaded'}
              <button class="btn btn--primary" disabled={modelRamTooLow} onclick={() => modelStore.downloadModel(m.id)}>Download</button>
            {:else if m.status.status === 'downloading'}
              <button class="btn" onclick={() => modelStore.cancelDownload(m.id)}>Cancel</button>
            {:else if m.status.status === 'ready'}
              <button class="btn btn--ghost btn--danger" onclick={() => modelStore.deleteModel(m.id)}>Delete</button>
            {:else if m.status.status === 'loaded'}
              <button class="btn" onclick={() => modelStore.unloadModel()}>Unload</button>
            {:else if m.status.status === 'error'}
              <button class="btn btn--primary" onclick={() => modelStore.downloadModel(m.id)}>Retry</button>
            {/if}
          </div>
        </div>
      {/each}
    {/if}
  </section>

  <hr class="mm-divider" />

  <!-- ── Discovered remote models ────────────────────────────────── -->
  <section class="mm-section">
    <div class="mm-section-header">
      <h3>Available remote models</h3>
      <button
        class="refresh-btn"
        onclick={() => refreshRemoteModels(true)}
        disabled={remoteChecking}
        aria-label="Refresh remote models"
      >
        <svg class:spinning={remoteChecking} viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" width="14" height="14">
          <path d="M21.5 2v6h-6M2.5 22v-6h6M2 11.5a10 10 0 0 1 18.8-4.3M22 12.5a10 10 0 0 1-18.8 4.2" />
        </svg>
      </button>
    </div>

    {#if remoteChecking}
      <p class="mm-empty">Checking…</p>
    {:else if remoteModels.length > 0}
      <ul class="remote-list">
        {#each remoteModels as m (m.id)}
          <li class="remote-item">{m.name}</li>
        {/each}
      </ul>
    {:else if openAiConfigs.length === 0}
      <p class="mm-empty">
        No endpoints configured. Add one below — a local Ollama server is
        reached at <code>http://localhost:11434/v1</code>.
      </p>
    {:else}
      <p class="mm-notice">
        No models found. Check that each configured endpoint is running and its
        base URL is correct.
      </p>
    {/if}
  </section>

  <hr class="mm-divider" />

  <!-- ── OpenAI-compatible ──────────────────────────────────────── -->
  <section class="mm-section">
    <div class="mm-section-header">
      <h3>OpenAI-compatible providers</h3>
      <button class="btn btn--primary btn--sm" onclick={startAdd}>Add</button>
    </div>

    {#if openAiConfigs.length === 0 && !editingConfig}
      <p class="mm-empty">No providers configured. Add an API key to use external models.</p>
    {/if}

    {#each openAiConfigs as config (config.id)}
      <div class="config-card">
        <div class="config-card-info">
          <span class="config-name">{config.name}</span>
          <span class="config-url">{config.baseUrl}</span>
        </div>
        <div class="config-card-actions">
          <button class="btn btn--sm" onclick={() => startEdit(config)}>Edit</button>
          <button class="btn btn--sm btn--ghost btn--danger" onclick={() => deleteConfig(config.id)}>Remove</button>
        </div>
      </div>
    {/each}

    {#if editingConfig}
      <div class="config-form">
        <h4 class="config-form-title">{isNewConfig ? 'Add provider' : 'Edit provider'}</h4>
        <label class="form-label">
          Name
          <input class="form-input" type="text" bind:value={editForm.name} placeholder="e.g. My OpenAI Key" />
        </label>
        <label class="form-label">
          Base URL
          <input class="form-input" type="url" bind:value={editForm.baseUrl} placeholder="https://api.openai.com/v1" />
        </label>
        <label class="form-label">
          Model
          <input class="form-input" type="text" bind:value={editForm.model} placeholder="e.g. gpt-4o" />
        </label>
        <p class="mm-desc">The exact model identifier the endpoint expects — required by the real OpenAI API and any server hosting more than one model.</p>
        <label class="form-label">
          API Key
          <input class="form-input" type="password" bind:value={editForm.apiKey} placeholder="sk-…" />
        </label>
        <div class="form-actions">
          <button class="btn btn--primary btn--sm" onclick={saveConfig} disabled={!editForm.name || !editForm.baseUrl || !editForm.model}>Save</button>
          <button class="btn btn--sm" onclick={cancelEdit}>Cancel</button>
        </div>
      </div>
    {/if}
  </section>

  <hr class="mm-divider" />

  <!-- ── Default model ─────────────────────────────────────────── -->
  <section class="mm-section">
    <h3>Default model</h3>
    <p class="mm-desc">New conversations start with this model pre-selected.</p>
    {#if availableSelectionsForDefault.length === 0}
      <p class="mm-empty">No ready models available. Download a local model or add a provider above.</p>
    {:else}
      <select
        class="form-select"
        value={defaultModel ? encodeSelection(defaultModel) : ''}
        onchange={handleDefaultChange}
      >
        <option value="">None</option>
        {#each availableSelectionsForDefault as opt (opt.value)}
          <option value={opt.value}>{opt.label}</option>
        {/each}
      </select>
    {/if}
  </section>

</div>

<style>
  .model-manager {
    display: flex;
    flex-direction: column;
    gap: 0;
  }

  .mm-section {
    display: flex;
    flex-direction: column;
    gap: 0.75rem;
    padding: 1.5rem 0;
  }

  .mm-section-header {
    display: flex;
    align-items: center;
    justify-content: space-between;
  }

  .mm-section h3 {
    margin: 0;
    font-size: 0.9375rem;
    font-weight: 600;
    color: hsl(var(--foreground));
  }

  .mm-section h4.config-form-title {
    margin: 0;
    font-size: 0.875rem;
    font-weight: 600;
    color: hsl(var(--foreground));
  }

  .mm-divider {
    border: none;
    border-top: 1px solid hsl(var(--border));
    margin: 0;
  }

  .mm-notice {
    font-size: 0.8125rem;
    color: hsl(var(--muted-foreground));
    margin: 0;
  }

  .mm-notice--warn {
    color: hsl(32 95% 44%);
    background: hsl(38 92% 50% / 0.08);
    border: 1px solid hsl(38 92% 50% / 0.2);
    border-radius: 0.375rem;
    padding: 0.5rem 0.75rem;
  }

  .mm-notice--ok {
    color: hsl(142 76% 36%);
  }

  .mm-link {
    color: hsl(var(--primary));
    text-decoration: underline;
  }

  .mm-empty {
    font-size: 0.8125rem;
    color: hsl(var(--muted-foreground));
    margin: 0;
    font-style: italic;
  }

  .mm-desc {
    font-size: 0.8125rem;
    color: hsl(var(--muted-foreground));
    margin: 0;
  }

  /* Model card */
  .model-card {
    border: 1px solid hsl(var(--border));
    border-radius: 0.5rem;
    padding: 0.875rem 1rem;
    background: hsl(var(--background));
    display: flex;
    flex-direction: column;
    gap: 0.5rem;
  }

  .model-card--dim {
    opacity: 0.55;
  }

  .model-card-top {
    display: flex;
    align-items: flex-start;
    justify-content: space-between;
    gap: 0.75rem;
  }

  .model-card-info {
    display: flex;
    flex-direction: column;
    gap: 0.125rem;
    min-width: 0;
  }

  .model-name {
    font-size: 0.875rem;
    font-weight: 500;
    color: hsl(var(--foreground));
  }

  .model-meta {
    font-size: 0.75rem;
    color: hsl(var(--muted-foreground));
  }

  .status-badge {
    flex-shrink: 0;
    font-size: 0.6875rem;
    font-weight: 500;
    padding: 0.125rem 0.5rem;
    border-radius: 9999px;
  }

  .status-default { background: hsl(var(--muted)); color: hsl(var(--muted-foreground)); }
  .status-ready   { background: hsl(142 76% 36% / 0.1); color: hsl(142 76% 36%); }
  .status-loaded  { background: hsl(var(--primary) / 0.1); color: hsl(var(--primary)); }
  .status-progress { background: hsl(45 93% 47% / 0.1); color: hsl(45 93% 47%); }
  .status-error   { background: hsl(var(--destructive) / 0.1); color: hsl(var(--destructive)); }

  .progress-row {
    display: flex;
    align-items: center;
    gap: 0.625rem;
  }

  .progress-bar {
    flex: 1;
    height: 5px;
    border-radius: 9999px;
    background: hsl(var(--muted));
    overflow: hidden;
  }

  .progress-fill {
    height: 100%;
    border-radius: 9999px;
    background: hsl(var(--primary));
  }

  .progress-label {
    font-size: 0.6875rem;
    color: hsl(var(--muted-foreground));
    min-width: 2.5rem;
    text-align: right;
    font-variant-numeric: tabular-nums;
  }

  .model-card-actions {
    display: flex;
    gap: 0.5rem;
  }

  /* Remote model list */
  .remote-list {
    list-style: none;
    margin: 0;
    padding: 0;
    display: flex;
    flex-direction: column;
    gap: 0.375rem;
  }

  .remote-item {
    font-size: 0.8125rem;
    color: hsl(var(--foreground));
    padding: 0.375rem 0.625rem;
    border: 1px solid hsl(var(--border));
    border-radius: 0.375rem;
    background: hsl(var(--muted) / 0.3);
  }

  /* Config card */
  .config-card {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 0.75rem;
    padding: 0.75rem 1rem;
    border: 1px solid hsl(var(--border));
    border-radius: 0.5rem;
    background: hsl(var(--background));
  }

  .config-card-info {
    display: flex;
    flex-direction: column;
    gap: 0.125rem;
    min-width: 0;
  }

  .config-name {
    font-size: 0.875rem;
    font-weight: 500;
    color: hsl(var(--foreground));
  }

  .config-url {
    font-size: 0.75rem;
    color: hsl(var(--muted-foreground));
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .config-card-actions {
    display: flex;
    gap: 0.375rem;
    flex-shrink: 0;
  }

  /* Config form */
  .config-form {
    border: 1px solid hsl(var(--border));
    border-radius: 0.5rem;
    padding: 1rem;
    background: hsl(var(--muted) / 0.15);
    display: flex;
    flex-direction: column;
    gap: 0.75rem;
  }

  .form-label {
    display: flex;
    flex-direction: column;
    gap: 0.25rem;
    font-size: 0.8125rem;
    font-weight: 500;
    color: hsl(var(--foreground));
  }

  .form-input {
    padding: 0.375rem 0.625rem;
    border: 1px solid hsl(var(--border));
    border-radius: 0.375rem;
    background: hsl(var(--background));
    color: hsl(var(--foreground));
    font-size: 0.8125rem;
    outline: none;
  }

  .form-input:focus {
    /* Border COLOR only — see the focus rule in DESIGN.md. The alpha ring that
       sat here composited against whatever was behind it. */
    border-color: hsl(var(--ring));
  }

  .form-actions {
    display: flex;
    gap: 0.5rem;
  }

  /* Default model select */
  .form-select {
    padding: 0.375rem 0.625rem;
    border: 1px solid hsl(var(--border));
    border-radius: 0.375rem;
    background: hsl(var(--background));
    color: hsl(var(--foreground));
    font-size: 0.8125rem;
    outline: none;
    max-width: 24rem;
  }

  .form-select:focus {
    /* Border COLOR only — see .form-input:focus above. */
    border-color: hsl(var(--ring));
  }

  /* Buttons */
  .btn {
    padding: 0.375rem 0.75rem;
    border: 1px solid hsl(var(--border));
    border-radius: 0.375rem;
    background: hsl(var(--background));
    color: hsl(var(--foreground));
    font-size: 0.8125rem;
    font-weight: 500;
    cursor: pointer;
    white-space: nowrap;
  }

  .btn:hover:not(:disabled) { background: hsl(var(--accent)); }
  .btn:disabled { opacity: 0.5; cursor: not-allowed; }

  .btn--sm { padding: 0.25rem 0.625rem; font-size: 0.75rem; }

  .btn--primary {
    background: hsl(var(--primary));
    color: hsl(var(--primary-foreground));
    border-color: hsl(var(--primary));
  }
  .btn--primary:hover:not(:disabled) { opacity: 0.9; }

  .btn--ghost {
    border: none;
    background: none;
    color: hsl(var(--muted-foreground));
  }
  .btn--ghost:hover:not(:disabled) { background: hsl(var(--accent)); }

  .btn--danger { color: hsl(var(--destructive)); }
  .btn--danger:hover:not(:disabled) { background: hsl(var(--destructive) / 0.08); }

  /* Refresh button */
  .refresh-btn {
    display: flex;
    align-items: center;
    justify-content: center;
    width: 1.75rem;
    height: 1.75rem;
    border-radius: 0.375rem;
    border: 1px solid hsl(var(--border));
    background: hsl(var(--background));
    cursor: pointer;
    color: hsl(var(--muted-foreground));
  }

  .refresh-btn:hover:not(:disabled) { color: hsl(var(--foreground)); }
  .refresh-btn:disabled { opacity: 0.5; cursor: not-allowed; }
</style>
