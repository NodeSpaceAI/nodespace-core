/**
 * Agent Store - Manages agent availability and selection using Svelte 5 runes.
 *
 * Merges two agent sources into a unified dropdown:
 * 1. ACP agents (Claude Code, Gemini CLI, etc.) — external subprocesses
 * 2. Local model agents (Ministral 3B/8B) — in-process llama.cpp inference
 *
 * Local model agents use IDs prefixed with "local:" (e.g., "local:ministral-3b-q4km").
 * The chat-store uses this prefix to route messages to the correct backend.
 */

import { createLogger } from '$lib/utils/logger';
import type { AcpAgentInfo, ModelInfo } from '$lib/types/agent-types';
import * as tauriCommands from '$lib/services/tauri-commands';

const log = createLogger('AgentStore');

/** Prefix for local model agent IDs. */
export const LOCAL_AGENT_PREFIX = 'local:';

/** Check if an agent ID refers to a local model agent. */
export function isLocalAgent(agentId: string | null): boolean {
  return agentId !== null && agentId.startsWith(LOCAL_AGENT_PREFIX);
}

/** Extract the model ID from a local agent ID (e.g., "local:ministral-3b-q4km" → "ministral-3b-q4km"). */
export function localAgentModelId(agentId: string): string {
  return agentId.replace(LOCAL_AGENT_PREFIX, '');
}

/** Check if running in Tauri desktop environment. */
function isTauri(): boolean {
  return (
    typeof window !== 'undefined' &&
    ('__TAURI__' in window || '__TAURI_INTERNALS__' in window)
  );
}

/** Mock local agent for non-Tauri environments and error fallback. */
const MOCK_LOCAL_AGENT: AcpAgentInfo = {
  id: `${LOCAL_AGENT_PREFIX}ministral-3b-q4km`,
  name: 'Ministral 3B Instruct Q4_K_M',
  binary: 'local',
  args: [],
  auth_method: { method: 'agent_managed' },
  available: true,
};

/** Mock ACP agents for development. */
const MOCK_AGENTS: AcpAgentInfo[] = [
  {
    id: 'claude-code',
    name: 'Claude Code',
    binary: 'claude',
    args: ['--chat'],
    auth_method: { method: 'agent_managed' },
    available: true,
    version: '1.0.0',
  },
  {
    id: 'gemini-cli',
    name: 'Gemini CLI',
    binary: 'gemini',
    args: [],
    auth_method: { method: 'env_api_key', var_name: 'GEMINI_API_KEY' },
    available: false,
    version: '0.9.0',
  },
];

/** Convert a ModelInfo to an AcpAgentInfo for unified display. */
function modelToAgent(model: ModelInfo): AcpAgentInfo {
  return {
    id: `${LOCAL_AGENT_PREFIX}${model.id}`,
    name: model.name,
    binary: 'local',
    args: [],
    auth_method: { method: 'agent_managed' },
    // Local models are always "available" — download happens on first use
    available: true,
  };
}

class AgentStore {
  agents = $state<AcpAgentInfo[]>([]);
  selectedAgentId = $state<string | null>(null);
  isLoading = $state(false);

  /** The currently selected agent, derived from agents and selectedAgentId. */
  get selectedAgent(): AcpAgentInfo | undefined {
    return this.agents.find((a) => a.id === this.selectedAgentId);
  }

  /** Available (installed and detected) agents. */
  get availableAgents(): AcpAgentInfo[] {
    return this.agents.filter((a) => a.available);
  }

  /** Whether any agents have been detected. */
  get hasAgents(): boolean {
    return this.agents.length > 0;
  }

  /** Select an agent by ID. */
  selectAgent(agentId: string): void {
    const agent = this.agents.find((a) => a.id === agentId);
    if (agent) {
      this.selectedAgentId = agentId;
      log.info('Agent selected', { agentId, name: agent.name });
    } else {
      log.warn('Attempted to select unknown agent', { agentId });
    }
  }

  /** Refresh agent list from backend (real Tauri or mock fallback). */
  async refreshAgents(): Promise<void> {
    this.isLoading = true;
    try {
      if (isTauri()) {
        // Fetch ACP agents and local models in parallel
        const [acpAgents, models] = await Promise.all([
          tauriCommands.acpRefreshAgents(),
          tauriCommands.chatModelList(),
        ]);

        // Local models first, then ACP agents
        const localAgents = models.map(modelToAgent);
        this.agents = [...localAgents, ...acpAgents];
      } else {
        // Mock fallback: simulate network delay
        await new Promise((resolve) => setTimeout(resolve, 300));
        this.agents = [MOCK_LOCAL_AGENT, ...MOCK_AGENTS];
      }

      // Auto-select first available agent if none selected
      if (!this.selectedAgentId) {
        const firstAvailable = this.agents.find((a) => a.available);
        if (firstAvailable) {
          this.selectedAgentId = firstAvailable.id;
        }
      }

      log.info('Agents refreshed', { count: this.agents.length });
    } catch (err) {
      const message = err instanceof Error ? err.message : 'Failed to refresh agents';
      log.error('Failed to refresh agents', { error: message });

      // Fall back to mock on error
      if (this.agents.length === 0) {
        this.agents = [MOCK_LOCAL_AGENT, ...MOCK_AGENTS];
        log.info('Fell back to mock agents after error');
      }
    } finally {
      this.isLoading = false;
    }
  }

  /** Reset the store to initial state. */
  reset(): void {
    this.agents = [];
    this.selectedAgentId = null;
    this.isLoading = false;
  }
}

export const agentStore = new AgentStore();
