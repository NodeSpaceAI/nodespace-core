<!--
  AiChatModelSelector — unified single-dropdown model picker for AiChatNodeViewer.

  Replaces the two-step provider → model full-page flow with a compact
  dropdown in the chat header. Renders section headers (Local, remote endpoints)
  and "Set up..." action as a native <select>-based custom UI.

  On mount fetches: chatModelList(), getSystemRamGb(), and
  OpenAI-compat configs from the settings store. Emits a ModelSelection via onSelect.

  For native models that need download, calls onSelect immediately with the selection
  — the parent viewer owns the download modal.
-->

<script lang="ts" module>
  import type { AiChatProvider } from '$lib/types/ai-chat-node';

  export interface ModelSelection {
    provider: AiChatProvider;
    modelId: string;
    configId?: string; // for openai-compat
  }
</script>

<script lang="ts">
  /* global HTMLSelectElement */
  import { onMount, onDestroy } from 'svelte';
  import { listen, type UnlistenFn } from '@tauri-apps/api/event';
  import {
    chatModelList,
    getSystemRamGb,
    getOpenAiCompatConfigsFromDaemon,
    type ChatModelEntry,
  } from '$lib/services/tauri-commands';
  import { AGENT_EVENTS } from '$lib/types/agent-types';
  import { getOpenAiConfigs } from '$lib/stores/settings.svelte';
  import { settingsStore } from '$lib/stores/settings.svelte';
  import { addTab, navigationStore, setActiveTab } from '$lib/stores/navigation.svelte';
  import { createLogger } from '$lib/utils/logger';
  import type { OpenAiCompatConfig } from '$lib/types/ai-chat-node';
  import { agentStore, isLocalAgent } from '$lib/stores/agent-store.svelte';

  const log = createLogger('AiChatModelSelector');

  // Sentinel values used in the <select> value attribute.
  const SETUP_SENTINEL = '__setup__';
  const HEADER_SENTINEL_PREFIX = '__header__';
  const PTY_PREFIX = 'pty:';

  let {
    nodeId,
    disabled = false,
    currentValue = '',
    onSelect,
  }: {
    nodeId: string;
    disabled?: boolean;
    /** Currently selected value, reflected in the <select>. */
    currentValue?: string;
    onSelect?: (_selection: ModelSelection) => void;
  } = $props();

  // --- Async data ---
  let models = $state<ChatModelEntry[]>([]);
  let ramGb = $state(0);
  let openAiConfigs = $state<OpenAiCompatConfig[]>([]);
  let loading = $state(true);

  // Live download tracking (model_id → bytes)
  let downloads = $state<Record<string, { downloaded: number; total: number }>>({});
  let unlistenProgress: UnlistenFn | null = null;
  let unlistenReady: UnlistenFn | null = null;

  // --- Derived subsets ---
  const nativeModels = $derived(models.filter((m) => m.backend === 'gguf'));
  // Models discovered at a configured OpenAI-compatible endpoint (Ollama's
  // /v1, LM Studio, vLLM, ...). The daemon returns one row per discovered
  // model, already carrying the full "openai-compat:<config>:<model>" id.
  const remoteModels = $derived(models.filter((m) => m.backend === 'openai-compat'));

  // PTY agents (Claude Code, Antigravity CLI, Codex, ...) — excludes agentStore's
  // "local:" entries, which are in-process llama.cpp models already surfaced
  // in the "Local" section above. The full list (available + unavailable) is
  // needed to render unavailable agents disabled rather than hiding them.
  const ptyAgents = $derived(agentStore.agents.filter((a) => !isLocalAgent(a.id)));
  const availablePtyAgents = $derived(
    agentStore.availableAgents.filter((a) => !isLocalAgent(a.id))
  );

  function isReady(m: ChatModelEntry): boolean {
    return m.status?.status === 'ready' || m.status?.status === 'loaded';
  }

  function isDownloading(m: ChatModelEntry): boolean {
    return m.status?.status === 'downloading' || m.id in downloads;
  }

  /** Build the value string used in the <select> for a given model entry. */
  function nativeValue(m: ChatModelEntry): string {
    return `native:${m.id}`;
  }

  // Discovered model IDs from the daemon already carry the full
  // "openai-compat:<config>:<model>" prefix. Use the raw ID as the option
  // value so handleChange can pass it straight through without rebuilding it.
  function remoteValue(m: ChatModelEntry): string {
    return m.id;
  }

  const isTauri =
    typeof window !== 'undefined' &&
    ('__TAURI__' in window || '__TAURI_INTERNALS__' in window);

  async function refresh(): Promise<void> {
    try {
      const [list, ram, daemonConfigs] = await Promise.all([
        chatModelList(),
        getSystemRamGb(),
        getOpenAiCompatConfigsFromDaemon().catch((err) => {
          log.warn('Failed to load OpenAI-compat configs from daemon, using local cache', err);
          return null;
        }),
      ]);
      models = list;
      ramGb = ram;
      openAiConfigs =
        daemonConfigs?.map((c) => ({
          id: c.id,
          name: c.name,
          baseUrl: c.baseUrl,
          apiKey: c.apiKey,
          model: c.model,
        })) ?? getOpenAiConfigs();
    } catch (err) {
      log.error('Failed to load model list', err);
    } finally {
      loading = false;
    }
  }

  onMount(async () => {
    await refresh();
    if (agentStore.agents.length === 0) {
      await agentStore.refreshAgents();
    }

    if (isTauri) {
      try {
        unlistenProgress = await listen<{
          model_id: string;
          bytes_downloaded: number;
          bytes_total: number;
        }>(AGENT_EVENTS.MODEL_DOWNLOAD_PROGRESS, (event) => {
          const { model_id, bytes_downloaded, bytes_total } = event.payload;
          downloads = {
            ...downloads,
            [model_id]: { downloaded: bytes_downloaded, total: bytes_total },
          };
        });
      } catch (err) {
        log.warn('Failed to listen for download progress', err);
      }

      try {
        unlistenReady = await listen<{ model_id: string }>(
          AGENT_EVENTS.MODEL_DOWNLOAD_READY,
          (event) => {
            const next = { ...downloads };
            delete next[event.payload.model_id];
            downloads = next;
            // Refresh to pick up new status (ready/loaded).
            refresh();
          }
        );
      } catch (err) {
        log.warn('Failed to listen for download ready', err);
      }
    }
  });

  onDestroy(() => {
    unlistenProgress?.();
    unlistenReady?.();
  });

  /** Open the Settings tab pre-focused on the AI Models section. */
  function openAiModelsSettings(): void {
    settingsStore.initialCategory = 'ai-models';
    const state = navigationStore.state;
    const existing = state.tabs.find((t) => t.type === 'settings');
    if (existing) {
      setActiveTab(existing.id, existing.paneId);
    } else {
      addTab({
        id: 'settings',
        title: 'Settings',
        type: 'settings',
        closeable: true,
        paneId: state.activePaneId,
      });
    }
  }

  function handleChange(e: Event): void {
    const value = (e.currentTarget as HTMLSelectElement).value;

    if (!value || value.startsWith(HEADER_SENTINEL_PREFIX)) return;

    if (value === SETUP_SENTINEL) {
      openAiModelsSettings();
      // Reset the <select> back to the current value so it doesn't stay on "Set up…".
      (e.currentTarget as HTMLSelectElement).value = currentValue;
      return;
    }

    if (value.startsWith('native:')) {
      const modelId = value.slice('native:'.length);
      log.debug('Model selected', { provider: 'native', modelId, nodeId });
      onSelect?.({ provider: 'native', modelId });
      return;
    }

    // Option values for remote models are the daemon's own ids
    // ("openai-compat:<config>:<model>") — pass the full value through so the
    // daemon resolves the same endpoint and model it advertised. The config
    // UUID is the segment up to the FIRST colon: a UUID never contains one,
    // whereas a model name routinely does ("llama3.1:8b").
    if (value.startsWith('openai-compat:')) {
      const rest = value.slice('openai-compat:'.length);
      const configId = rest.split(':')[0];
      log.debug('Remote model selected', { configId, modelId: value, nodeId });
      onSelect?.({ provider: 'openai-compat', modelId: value, configId });
      return;
    }

    if (value.startsWith(PTY_PREFIX)) {
      const agentId = value.slice(PTY_PREFIX.length);
      log.debug('PTY agent selected', { provider: 'pty', agentId, nodeId });
      onSelect?.({ provider: 'pty', modelId: agentId });
      return;
    }
  }

  function ptyValue(agentId: string): string {
    return `${PTY_PREFIX}${agentId}`;
  }

  function downloadBadgeText(m: ChatModelEntry): string {
    const dl = downloads[m.id];
    if (dl && dl.total > 0) {
      const pct = Math.round((dl.downloaded / dl.total) * 100);
      return ` (${pct}%)`;
    }
    return ' (downloading…)';
  }
</script>

<div class="model-selector-wrapper">
  {#if loading}
    <span class="model-selector-loading">Loading…</span>
  {:else}
    <select
      class="model-selector"
      aria-label="Select AI model"
      value={currentValue}
      {disabled}
      onchange={handleChange}
    >
      {#if !currentValue}
        <option value="" disabled selected>Choose a model…</option>
      {/if}

      <!-- ── Local section ── -->
      <optgroup label="Local">
        {#each nativeModels as m (m.id)}
          {@const ready = isReady(m)}
          {@const downloading = isDownloading(m)}
          {@const tooLow = ramGb > 0 && ramGb < m.minMemoryGb}
          <option
            value={nativeValue(m)}
            disabled={tooLow}
            title={tooLow ? `Requires ${m.minMemoryGb} GB RAM (system has ${ramGb} GB)` : undefined}
          >
            {m.name}{tooLow
              ? ` (requires ${m.minMemoryGb} GB RAM)`
              : !ready && !downloading
                ? ' (download needed)'
                : downloading
                  ? downloadBadgeText(m)
                  : ''}
          </option>
        {/each}
        {#if nativeModels.length === 0}
          <option value={`${HEADER_SENTINEL_PREFIX}no-local`} disabled>No local models found</option>
        {/if}
      </optgroup>

      <!-- ── Remote endpoints (OpenAI-compatible: Ollama /v1, LM Studio, …) ── -->
      {#if remoteModels.length > 0}
        <optgroup label="Remote endpoints">
          {#each remoteModels as m (m.id)}
            <option value={remoteValue(m)}>{m.name}</option>
          {/each}
        </optgroup>
      {:else if openAiConfigs.length > 0}
        <!-- Endpoints are configured but none answered /models: the server is
             down or the base URL is wrong. Say so rather than showing nothing. -->
        <optgroup label="Remote endpoints">
          <option value={`${HEADER_SENTINEL_PREFIX}no-remote`} disabled>
            No models found — check that the endpoint is running
          </option>
        </optgroup>
      {/if}

      <!-- ── PTY Agents ── -->
      {#if availablePtyAgents.length > 0}
        <optgroup label="PTY Agents">
          {#each ptyAgents as agent (agent.id)}
            <option value={ptyValue(agent.id)} disabled={!agent.available}>
              {agent.name}{agent.available ? '' : ' (not installed)'}
            </option>
          {/each}
        </optgroup>
      {/if}

      <!-- ── Action: Set up… ── -->
      <optgroup label="─────────────────" disabled></optgroup>
      <option value={SETUP_SENTINEL}>Set up…</option>
    </select>
  {/if}
</div>

<style>
  .model-selector-wrapper {
    display: flex;
    align-items: center;
  }

  .model-selector {
    font-size: 0.8125rem;
    padding: 0.3125rem 0.5rem;
    border: 1px solid hsl(var(--border));
    border-radius: 0.375rem;
    background: hsl(var(--background));
    color: hsl(var(--foreground));
    cursor: pointer;
    max-width: 14rem;
  }

  .model-selector:hover:not(:disabled) {
    border-color: hsl(var(--ring));
  }

  .model-selector:disabled {
    opacity: 0.6;
    cursor: default;
  }

  .model-selector-loading {
    font-size: 0.8125rem;
    color: hsl(var(--muted-foreground));
  }
</style>
