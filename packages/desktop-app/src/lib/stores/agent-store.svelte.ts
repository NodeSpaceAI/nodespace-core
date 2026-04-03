/**
 * Agent Store - Manages ACP agent availability and selection using Svelte 5 runes.
 *
 * Provides mock agent data for development. Real Tauri integration
 * will be wired in #1008.
 */

import { createLogger } from '$lib/utils/logger';
import type { AcpAgentInfo } from '$lib/types/agent-types';

const log = createLogger('AgentStore');

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
  {
    id: 'local-agent',
    name: 'Local Agent (Ministral)',
    binary: 'nodespace-agent',
    args: [],
    auth_method: { method: 'agent_managed' },
    available: true,
  },
];

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

  /** Refresh agent list from backend (mock). */
  async refreshAgents(): Promise<void> {
    this.isLoading = true;
    try {
      // Simulate network delay
      await new Promise((resolve) => setTimeout(resolve, 300));
      this.agents = [...MOCK_AGENTS];

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
