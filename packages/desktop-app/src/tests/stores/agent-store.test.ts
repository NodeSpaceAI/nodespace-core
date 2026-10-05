/**
 * Unit tests for AgentStore - agent selection, refresh, availability
 */

import { describe, it, expect, beforeEach, vi, afterEach } from 'vitest';
import { agentStore, availabilityToAgent } from '$lib/stores/agent-store.svelte';
import * as tauriCommands from '$lib/services/tauri-commands';
import type { AgentAvailabilityInfo, ChatModelEntry } from '$lib/services/tauri-commands';

// Mock the logger
vi.mock('$lib/utils/logger', () => ({
  createLogger: () => ({
    debug: vi.fn(),
    info: vi.fn(),
    warn: vi.fn(),
    error: vi.fn(),
  }),
}));

function mockTauriEnvironment(isTauri: boolean) {
  interface WindowWithTauri extends Window {
    __TAURI__?: Record<string, unknown>;
    __TAURI_INTERNALS__?: Record<string, unknown>;
  }
  if (isTauri) {
    (window as WindowWithTauri).__TAURI_INTERNALS__ = {};
  } else {
    // Clear BOTH markers `isTauri()` checks — see model-store.test.ts for why
    // this matters under vitest's shared-window `forks` pool.
    delete (window as WindowWithTauri).__TAURI__;
    delete (window as WindowWithTauri).__TAURI_INTERNALS__;
  }
}

describe('AgentStore', () => {
  beforeEach(() => {
    agentStore.reset();
    mockTauriEnvironment(false);
    vi.useFakeTimers();
  });

  afterEach(() => {
    mockTauriEnvironment(false);
    vi.useRealTimers();
    vi.restoreAllMocks();
  });

  describe('Initial State', () => {
    it('starts with empty agents', () => {
      expect(agentStore.agents).toEqual([]);
    });

    it('starts with no selected agent', () => {
      expect(agentStore.selectedAgentId).toBeNull();
    });

    it('starts not loading', () => {
      expect(agentStore.isLoading).toBe(false);
    });

    it('reports no agents available', () => {
      expect(agentStore.hasAgents).toBe(false);
    });
  });

  describe('refreshAgents', () => {
    it('loads mock agents', async () => {
      const refreshPromise = agentStore.refreshAgents();
      await vi.runAllTimersAsync();
      await refreshPromise;

      expect(agentStore.agents.length).toBeGreaterThan(0);
      expect(agentStore.hasAgents).toBe(true);
    });

    it('sets loading state during refresh', async () => {
      const refreshPromise = agentStore.refreshAgents();
      expect(agentStore.isLoading).toBe(true);

      await vi.runAllTimersAsync();
      await refreshPromise;

      expect(agentStore.isLoading).toBe(false);
    });

    it('auto-selects first available agent', async () => {
      const refreshPromise = agentStore.refreshAgents();
      await vi.runAllTimersAsync();
      await refreshPromise;

      expect(agentStore.selectedAgentId).toBeTruthy();

      // The selected agent should be available
      const selected = agentStore.selectedAgent;
      expect(selected).toBeDefined();
      expect(selected!.available).toBe(true);
    });

    it('preserves existing selection on refresh', async () => {
      // First refresh to populate
      let promise = agentStore.refreshAgents();
      await vi.runAllTimersAsync();
      await promise;

      // Manually select a specific agent
      const agentId = agentStore.agents[0].id;
      agentStore.selectAgent(agentId);

      // Refresh again
      promise = agentStore.refreshAgents();
      await vi.runAllTimersAsync();
      await promise;

      // Selection should be preserved
      expect(agentStore.selectedAgentId).toBe(agentId);
    });
  });

  describe('refreshAgents (Tauri, catalog filtering)', () => {
    beforeEach(() => {
      mockTauriEnvironment(true);
      vi.spyOn(tauriCommands, 'ptyCheckAgentAvailability').mockResolvedValue({ agents: [] });
    });

    afterEach(() => {
      mockTauriEnvironment(false);
    });

    it('surfaces only GGUF catalog rows as local agents', async () => {
      const catalog: ChatModelEntry[] = [
        {
          id: 'gemma-4-e4b-q4km',
          name: 'Gemma 4 E4B Instruct Q4_K_M',
          backend: 'gguf',
          sizeBytes: 1,
          quantization: 'Q4_K_M',
          minMemoryGb: 8,
          status: { status: 'not_downloaded' },
        },
        {
          id: 'remote-gpt-4o',
          name: 'GPT-4o (remote)',
          backend: 'openai-compat',
          sizeBytes: 0,
          quantization: '',
          minMemoryGb: 0,
          status: { status: 'not_downloaded' },
        },
      ];
      vi.spyOn(tauriCommands, 'chatModelList').mockResolvedValue(catalog);

      const promise = agentStore.refreshAgents();
      await vi.runAllTimersAsync();
      await promise;

      const localAgentNames = agentStore.agents
        .filter((a) => a.binary === 'local')
        .map((a) => a.name);
      expect(localAgentNames).toEqual(['Gemma 4 E4B Instruct Q4_K_M']);
      expect(localAgentNames).not.toContain('GPT-4o (remote)');
    });
  });

  describe('selectAgent', () => {
    it('selects a valid agent', async () => {
      const refreshPromise = agentStore.refreshAgents();
      await vi.runAllTimersAsync();
      await refreshPromise;

      const agent = agentStore.agents[0];
      agentStore.selectAgent(agent.id);

      expect(agentStore.selectedAgentId).toBe(agent.id);
      expect(agentStore.selectedAgent).toEqual(agent);
    });

    it('ignores selection of unknown agent', async () => {
      const refreshPromise = agentStore.refreshAgents();
      await vi.runAllTimersAsync();
      await refreshPromise;

      const originalId = agentStore.selectedAgentId;
      agentStore.selectAgent('nonexistent-agent');

      expect(agentStore.selectedAgentId).toBe(originalId);
    });
  });

  describe('availableAgents', () => {
    it('filters to only available agents', async () => {
      const refreshPromise = agentStore.refreshAgents();
      await vi.runAllTimersAsync();
      await refreshPromise;

      const available = agentStore.availableAgents;
      expect(available.length).toBeGreaterThan(0);
      expect(available.length).toBeLessThanOrEqual(agentStore.agents.length);

      for (const agent of available) {
        expect(agent.available).toBe(true);
      }
    });
  });

  describe('selectedAgent', () => {
    it('returns undefined when nothing selected', () => {
      expect(agentStore.selectedAgent).toBeUndefined();
    });

    it('returns the selected agent object', async () => {
      const refreshPromise = agentStore.refreshAgents();
      await vi.runAllTimersAsync();
      await refreshPromise;

      const agent = agentStore.agents[0];
      agentStore.selectAgent(agent.id);

      const selected = agentStore.selectedAgent;
      expect(selected).toBeDefined();
      expect(selected!.id).toBe(agent.id);
      expect(selected!.name).toBe(agent.name);
    });
  });

  describe('availabilityToAgent', () => {
    it('maps a known agent type to its display label', () => {
      const info: AgentAvailabilityInfo = {
        agentType: 'claude-code',
        binary: 'claude',
        binaryFound: true,
        authFound: true,
        binaryPath: '/usr/local/bin/claude',
        installHint: null,
      };

      const agent = availabilityToAgent(info);

      expect(agent).toEqual({
        id: 'claude-code',
        name: 'Claude Code',
        binary: 'claude',
        args: [],
        auth_method: { method: 'agent_managed' },
        available: true,
      });
    });

    it('falls back to the raw agentType when no label is known', () => {
      const info: AgentAvailabilityInfo = {
        agentType: 'some-future-agent',
        binary: 'future-agent',
        binaryFound: true,
        authFound: true,
        binaryPath: '/usr/local/bin/future-agent',
        installHint: null,
      };

      expect(availabilityToAgent(info).name).toBe('some-future-agent');
    });

    it('is unavailable when the binary is missing', () => {
      const info: AgentAvailabilityInfo = {
        agentType: 'antigravity',
        binary: 'agy',
        binaryFound: false,
        authFound: true,
        binaryPath: null,
        installHint: 'curl -fsSL https://antigravity.google/cli/install.sh | bash',
      };

      expect(availabilityToAgent(info).available).toBe(false);
    });

    it('is unavailable when auth is missing even if the binary is found', () => {
      const info: AgentAvailabilityInfo = {
        agentType: 'antigravity',
        binary: 'agy',
        binaryFound: true,
        authFound: false,
        binaryPath: '/usr/local/bin/agy',
        installHint: null,
      };

      expect(availabilityToAgent(info).available).toBe(false);
    });
  });

  describe('reset', () => {
    it('clears all state', async () => {
      const refreshPromise = agentStore.refreshAgents();
      await vi.runAllTimersAsync();
      await refreshPromise;

      expect(agentStore.agents.length).toBeGreaterThan(0);

      agentStore.reset();

      expect(agentStore.agents).toEqual([]);
      expect(agentStore.selectedAgentId).toBeNull();
      expect(agentStore.isLoading).toBe(false);
    });
  });
});
