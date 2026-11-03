/**
 * Tauri System Commands Service Tests
 *
 * Tests the non-node commands that remain in tauri-commands after C1a.
 * Node-CRUD functions were removed; use backend-adapter directly.
 *
 * Every exported command is a thin `invoke(...)` wrapper. These tests check
 * the command name and arguments for a representative sample of commands.
 */

import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';

import { mockTauriCore } from '../helpers/mock-tauri-core';

const mockInvoke = vi.fn();
vi.mock('@tauri-apps/api/core', () =>
  mockTauriCore({ invoke: (...args: unknown[]) => mockInvoke(...args) })
);

import * as tauriCommands from '$lib/services/tauri-commands';

interface WindowWithTauri extends Window {
  __TAURI__?: Record<string, unknown>;
  __TAURI_INTERNALS__?: Record<string, unknown>;
}

function mockTauriEnvironment(isTauri: boolean) {
  if (isTauri) {
    (window as WindowWithTauri).__TAURI_INTERNALS__ = {};
  } else {
    delete (window as WindowWithTauri).__TAURI__;
    delete (window as WindowWithTauri).__TAURI_INTERNALS__;
  }
}

describe('Tauri System Commands - API Surface', () => {
  describe('ACP stubs (transport removed ahead of PTY rewrite)', () => {
    it('acpListAgents resolves to an empty array', async () => {
      await expect(tauriCommands.acpListAgents()).resolves.toEqual([]);
    });

    it('acpStartSession resolves to a pending pty-session placeholder id', async () => {
      const result = await tauriCommands.acpStartSession('claude');
      expect(result).toMatch(/^pty-session-pending-\d+$/);
    });

    it('acpSendMessage resolves to undefined', async () => {
      await expect(tauriCommands.acpSendMessage('session-1', 'hi')).resolves.toBeUndefined();
    });

    it('acpEndSession resolves to undefined', async () => {
      await expect(tauriCommands.acpEndSession('session-1')).resolves.toBeUndefined();
    });

    it('acpRefreshAgents resolves to an empty array', async () => {
      await expect(tauriCommands.acpRefreshAgents()).resolves.toEqual([]);
    });
  });

  describe('PTY session commands', () => {
    beforeEach(() => {
      mockInvoke.mockReset();
    });

    it('ptyLaunchSession invokes launch_session with the input payload', async () => {
      const input: tauriCommands.PtyLaunchInput = {
        agentType: 'claude',
        cols: 80,
        rows: 24
      };
      mockInvoke.mockResolvedValue({ sessionId: 's1', createdAt: 123 });

      const result = await tauriCommands.ptyLaunchSession(input);

      expect(mockInvoke).toHaveBeenCalledWith('launch_session', { input });
      expect(result).toEqual({ sessionId: 's1', createdAt: 123 });
    });

    it('ptyWriteInput invokes write_input with sessionId and data', async () => {
      mockInvoke.mockResolvedValue(3);

      const result = await tauriCommands.ptyWriteInput('s1', [1, 2, 3]);

      expect(mockInvoke).toHaveBeenCalledWith('write_input', { sessionId: 's1', data: [1, 2, 3] });
      expect(result).toBe(3);
    });

    it('ptyResizeTerminal invokes resize_terminal with cols/rows', async () => {
      mockInvoke.mockResolvedValue(undefined);

      await tauriCommands.ptyResizeTerminal('s1', 100, 40);

      expect(mockInvoke).toHaveBeenCalledWith('resize_terminal', {
        sessionId: 's1',
        cols: 100,
        rows: 40
      });
    });

    it('ptyTerminateSession invokes terminate_session with sessionId', async () => {
      mockInvoke.mockResolvedValue({ sessionId: 's1', wasRunning: true });

      const result = await tauriCommands.ptyTerminateSession('s1');

      expect(mockInvoke).toHaveBeenCalledWith('terminate_session', { sessionId: 's1' });
      expect(result).toEqual({ sessionId: 's1', wasRunning: true });
    });

    it('ptyListSessions invokes list_sessions with no args', async () => {
      mockInvoke.mockResolvedValue({ sessions: [], count: 0 });

      const result = await tauriCommands.ptyListSessions();

      expect(mockInvoke).toHaveBeenCalledWith('list_sessions');
      expect(result).toEqual({ sessions: [], count: 0 });
    });

    it('ptyCheckAgentAvailability invokes check_agent_availability with no args', async () => {
      mockInvoke.mockResolvedValue({ agents: [] });

      const result = await tauriCommands.ptyCheckAgentAvailability();

      expect(mockInvoke).toHaveBeenCalledWith('check_agent_availability');
      expect(result).toEqual({ agents: [] });
    });
  });

  describe('Tauri branch (window.__TAURI_INTERNALS__ present)', () => {
    beforeEach(() => {
      mockInvoke.mockReset();
      mockTauriEnvironment(true);
    });

    afterEach(() => {
      mockTauriEnvironment(false);
    });

    it('localAgentStatus invokes local_agent_status', async () => {
      mockInvoke.mockResolvedValue({ status: 'busy' });

      const result = await tauriCommands.localAgentStatus();

      expect(mockInvoke).toHaveBeenCalledWith('local_agent_status');
      expect(result).toEqual({ status: 'busy' });
    });

    it('localAgentCancelTurn invokes local_agent_cancel_turn with nodeId', async () => {
      mockInvoke.mockResolvedValue(undefined);

      await tauriCommands.localAgentCancelTurn('node-1');

      expect(mockInvoke).toHaveBeenCalledWith('local_agent_cancel_turn', { nodeId: 'node-1' });
    });

    it('localAgentOpenBoundChat invokes local_agent_open_bound_chat and returns the chat id', async () => {
      mockInvoke.mockResolvedValue('chat-1');

      const chatId = await tauriCommands.localAgentOpenBoundChat('play-1', {
        provider: 'native',
        model: 'gemma-4-e4b'
      });

      expect(mockInvoke).toHaveBeenCalledWith('local_agent_open_bound_chat', {
        nodeId: 'play-1',
        provider: 'native',
        model: 'gemma-4-e4b'
      });
      expect(chatId).toBe('chat-1');
    });

    it('localAgentOpenBoundChat sends no model when there is no default', async () => {
      mockInvoke.mockResolvedValue('chat-1');

      await tauriCommands.localAgentOpenBoundChat('play-1', null);

      expect(mockInvoke).toHaveBeenCalledWith('local_agent_open_bound_chat', {
        nodeId: 'play-1',
        provider: null,
        model: null
      });
    });

    it('chatModelList invokes chat_model_list with forceRefresh', async () => {
      mockInvoke.mockResolvedValue([]);

      await tauriCommands.chatModelList(true);

      expect(mockInvoke).toHaveBeenCalledWith('chat_model_list', { forceRefresh: true });
    });

    it('chatModelList defaults forceRefresh to false', async () => {
      mockInvoke.mockResolvedValue([]);

      await tauriCommands.chatModelList();

      expect(mockInvoke).toHaveBeenCalledWith('chat_model_list', { forceRefresh: false });
    });

    it('chatModelRecommended invokes chat_model_recommended', async () => {
      mockInvoke.mockResolvedValue('model-x');

      const result = await tauriCommands.chatModelRecommended();

      expect(mockInvoke).toHaveBeenCalledWith('chat_model_recommended');
      expect(result).toBe('model-x');
    });

    it('chatModelDownload invokes chat_model_download with modelId', async () => {
      mockInvoke.mockResolvedValue(undefined);

      await tauriCommands.chatModelDownload('model-x');

      expect(mockInvoke).toHaveBeenCalledWith('chat_model_download', { modelId: 'model-x' });
    });

    it('chatModelCancelDownload invokes chat_model_cancel_download with modelId', async () => {
      mockInvoke.mockResolvedValue(undefined);

      await tauriCommands.chatModelCancelDownload('model-x');

      expect(mockInvoke).toHaveBeenCalledWith('chat_model_cancel_download', {
        modelId: 'model-x'
      });
    });

    it('chatModelDelete invokes chat_model_delete with modelId', async () => {
      mockInvoke.mockResolvedValue(undefined);

      await tauriCommands.chatModelDelete('model-x');

      expect(mockInvoke).toHaveBeenCalledWith('chat_model_delete', { modelId: 'model-x' });
    });

    it('chatModelLoad invokes chat_model_load with modelId', async () => {
      mockInvoke.mockResolvedValue(undefined);

      await tauriCommands.chatModelLoad('model-x');

      expect(mockInvoke).toHaveBeenCalledWith('chat_model_load', { modelId: 'model-x' });
    });

    it('chatModelUnload invokes chat_model_unload with no args', async () => {
      mockInvoke.mockResolvedValue(undefined);

      await tauriCommands.chatModelUnload();

      expect(mockInvoke).toHaveBeenCalledWith('chat_model_unload');
    });

    it('getSystemRamGb invokes get_system_ram_gb', async () => {
      mockInvoke.mockResolvedValue(32);

      const result = await tauriCommands.getSystemRamGb();

      expect(mockInvoke).toHaveBeenCalledWith('get_system_ram_gb');
      expect(result).toBe(32);
    });

    it('ensureModelReady invokes ensure_model_ready with modelId and returns its result', async () => {
      mockInvoke.mockResolvedValue(true);

      const result = await tauriCommands.ensureModelReady('model-x');

      expect(mockInvoke).toHaveBeenCalledWith('ensure_model_ready', { modelId: 'model-x' });
      expect(result).toBe(true);
    });
  });
});
