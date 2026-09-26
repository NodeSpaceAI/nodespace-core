/**
 * Tauri System Commands Service Tests
 *
 * Tests the non-node commands that remain in tauri-commands after C1a.
 * Node-CRUD functions were removed; use backend-adapter directly.
 *
 * Every exported command branches on `isTauri()` (a local check of
 * `window.__TAURI__` / `window.__TAURI_INTERNALS__`, NOT the `isTauri` export
 * from `@tauri-apps/api/core`) into either `invoke(...)` or an HTTP call to
 * the dev-proxy via `fetch`. These tests exercise both branches for a
 * representative sample of commands in each category, plus the proxy helper
 * behaviors (`body === undefined`, `204 No Content`, non-ok error throwing).
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

  describe('PTY session commands (always invoke, no proxy fallback)', () => {
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

    it('getCaptureSettings invokes get_capture_settings', async () => {
      mockInvoke.mockResolvedValue({ enabled: true, content: 'full' });

      const result = await tauriCommands.getCaptureSettings();

      expect(mockInvoke).toHaveBeenCalledWith('get_capture_settings');
      expect(result).toEqual({ enabled: true, content: 'full' });
    });

    it('updateCaptureSettings invokes update_capture_settings with enabled/content, defaulting missing fields to null', async () => {
      mockInvoke.mockResolvedValue({ enabled: true, content: 'summary' });

      await tauriCommands.updateCaptureSettings({ enabled: true });

      expect(mockInvoke).toHaveBeenCalledWith('update_capture_settings', {
        enabled: true,
        content: null
      });
    });

    it('updateCaptureSettings defaults a missing enabled field to null when only content is provided', async () => {
      mockInvoke.mockResolvedValue({ enabled: false, content: 'full' });

      await tauriCommands.updateCaptureSettings({ content: 'full' });

      expect(mockInvoke).toHaveBeenCalledWith('update_capture_settings', {
        enabled: null,
        content: 'full'
      });
    });

    it('updateCaptureSettings passes both fields through when both are provided', async () => {
      mockInvoke.mockResolvedValue({ enabled: false, content: 'metadata_only' });

      await tauriCommands.updateCaptureSettings({ enabled: false, content: 'metadata_only' });

      expect(mockInvoke).toHaveBeenCalledWith('update_capture_settings', {
        enabled: false,
        content: 'metadata_only'
      });
    });

    it('getOpenAiCompatConfigsFromDaemon invokes get_openai_compat_configs', async () => {
      const configs: tauriCommands.OpenAiCompatConfigDto[] = [
        { id: '1', name: 'n', baseUrl: 'https://x', apiKey: 'k', model: 'm' }
      ];
      mockInvoke.mockResolvedValue(configs);

      const result = await tauriCommands.getOpenAiCompatConfigsFromDaemon();

      expect(mockInvoke).toHaveBeenCalledWith('get_openai_compat_configs');
      expect(result).toEqual(configs);
    });

    it('setOpenAiCompatConfigsOnDaemon invokes set_openai_compat_configs with configs', async () => {
      const configs: tauriCommands.OpenAiCompatConfigDto[] = [
        { id: '1', name: 'n', baseUrl: 'https://x', apiKey: 'k', model: 'm' }
      ];
      mockInvoke.mockResolvedValue(configs);

      const result = await tauriCommands.setOpenAiCompatConfigsOnDaemon(configs);

      expect(mockInvoke).toHaveBeenCalledWith('set_openai_compat_configs', { configs });
      expect(result).toEqual(configs);
    });
  });

  describe('Non-Tauri fallbacks (outside desktop)', () => {
    let fetchMock: ReturnType<typeof vi.fn>;

    beforeEach(() => {
      mockTauriEnvironment(false);
      fetchMock = vi.fn().mockResolvedValue({
        ok: true,
        status: 200,
        json: () => Promise.resolve([])
      });
      vi.stubGlobal('fetch', fetchMock);
    });

    afterEach(() => {
      vi.unstubAllGlobals();
    });

    it('localAgentStatus returns idle outside Tauri without calling fetch', async () => {
      const result = await tauriCommands.localAgentStatus();
      expect(result).toEqual({ status: 'idle' });
      expect(fetchMock).not.toHaveBeenCalled();
    });

    it('localAgentCancelTurn proxies to POST /api/agent/cancel-turn with nodeId body', async () => {
      fetchMock.mockResolvedValue({ ok: true, status: 204 });

      await tauriCommands.localAgentCancelTurn('node-1');

      expect(fetchMock).toHaveBeenCalledWith('http://localhost:3001/api/agent/cancel-turn', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ nodeId: 'node-1' })
      });
    });

    it('chatModelList proxies to GET /api/agent/models with forceRefresh query param', async () => {
      fetchMock.mockResolvedValue({ ok: true, status: 200, json: () => Promise.resolve([]) });

      const result = await tauriCommands.chatModelList(true);

      expect(fetchMock).toHaveBeenCalledWith(
        'http://localhost:3001/api/agent/models?force_refresh=true'
      );
      expect(result).toEqual([]);
    });

    it('chatModelRecommended proxies to GET /api/agent/recommended-model and unwraps modelId', async () => {
      fetchMock.mockResolvedValue({
        ok: true,
        status: 200,
        json: () => Promise.resolve({ modelId: 'model-y' })
      });

      const result = await tauriCommands.chatModelRecommended();

      expect(fetchMock).toHaveBeenCalledWith('http://localhost:3001/api/agent/recommended-model');
      expect(result).toBe('model-y');
    });

    it('chatModelDownload proxies to POST /api/agent/models/:id/download with no body', async () => {
      fetchMock.mockResolvedValue({ ok: true, status: 204 });

      await tauriCommands.chatModelDownload('model x');

      expect(fetchMock).toHaveBeenCalledWith(
        'http://localhost:3001/api/agent/models/model%20x/download',
        { method: 'POST', headers: {}, body: undefined }
      );
    });

    it('chatModelCancelDownload proxies to DELETE /api/agent/models/:id/download', async () => {
      fetchMock.mockResolvedValue({ ok: true, status: 204 });

      await tauriCommands.chatModelCancelDownload('model-x');

      expect(fetchMock).toHaveBeenCalledWith(
        'http://localhost:3001/api/agent/models/model-x/download',
        { method: 'DELETE' }
      );
    });

    it('chatModelDelete proxies to DELETE /api/agent/models/:id', async () => {
      fetchMock.mockResolvedValue({ ok: true, status: 204 });

      await tauriCommands.chatModelDelete('model-x');

      expect(fetchMock).toHaveBeenCalledWith('http://localhost:3001/api/agent/models/model-x', {
        method: 'DELETE'
      });
    });

    it('chatModelLoad proxies to POST /api/agent/models/:id/load', async () => {
      fetchMock.mockResolvedValue({ ok: true, status: 204 });

      await tauriCommands.chatModelLoad('model-x');

      expect(fetchMock).toHaveBeenCalledWith(
        'http://localhost:3001/api/agent/models/model-x/load',
        { method: 'POST', headers: {}, body: undefined }
      );
    });

    it('chatModelUnload proxies to POST /api/agent/models/unload', async () => {
      fetchMock.mockResolvedValue({ ok: true, status: 204 });

      await tauriCommands.chatModelUnload();

      expect(fetchMock).toHaveBeenCalledWith('http://localhost:3001/api/agent/models/unload', {
        method: 'POST',
        headers: {},
        body: undefined
      });
    });

    it('getSystemRamGb proxies to GET /api/agent/system-ram and unwraps ramGb', async () => {
      fetchMock.mockResolvedValue({
        ok: true,
        status: 200,
        json: () => Promise.resolve({ ramGb: 16 })
      });

      const result = await tauriCommands.getSystemRamGb();

      expect(fetchMock).toHaveBeenCalledWith('http://localhost:3001/api/agent/system-ram');
      expect(result).toBe(16);
    });

    it('ensureModelReady proxies to POST /api/agent/ensure-model-ready and always returns false', async () => {
      fetchMock.mockResolvedValue({ ok: true, status: 204 });

      const result = await tauriCommands.ensureModelReady('model-x');

      expect(fetchMock).toHaveBeenCalledWith(
        'http://localhost:3001/api/agent/ensure-model-ready',
        {
          method: 'POST',
          headers: { 'Content-Type': 'application/json' },
          body: JSON.stringify({ modelId: 'model-x' })
        }
      );
      expect(result).toBe(false);
    });

    it('getCaptureSettings returns defaults outside Tauri without calling fetch', async () => {
      const result = await tauriCommands.getCaptureSettings();
      expect(result).toEqual({ enabled: false, content: 'metadata_only' });
      expect(fetchMock).not.toHaveBeenCalled();
    });

    it('updateCaptureSettings merges partial settings into defaults outside Tauri without calling fetch', async () => {
      const result = await tauriCommands.updateCaptureSettings({ enabled: true });
      expect(result).toEqual({ enabled: true, content: 'metadata_only' });
      expect(fetchMock).not.toHaveBeenCalled();
    });

    it('updateCaptureSettings with no overrides returns the bare defaults', async () => {
      const result = await tauriCommands.updateCaptureSettings({});
      expect(result).toEqual({ enabled: false, content: 'metadata_only' });
    });

    it('getOpenAiCompatConfigsFromDaemon returns an empty array outside Tauri without calling fetch', async () => {
      const result = await tauriCommands.getOpenAiCompatConfigsFromDaemon();
      expect(result).toEqual([]);
      expect(fetchMock).not.toHaveBeenCalled();
    });

    it('setOpenAiCompatConfigsOnDaemon echoes back the input configs outside Tauri without calling fetch', async () => {
      const configs: tauriCommands.OpenAiCompatConfigDto[] = [
        { id: '1', name: 'n', baseUrl: 'https://x', apiKey: 'k', model: 'm' }
      ];
      const result = await tauriCommands.setOpenAiCompatConfigsOnDaemon(configs);
      expect(result).toBe(configs);
      expect(fetchMock).not.toHaveBeenCalled();
    });
  });

  describe('proxy helper error paths', () => {
    let fetchMock: ReturnType<typeof vi.fn>;

    beforeEach(() => {
      mockTauriEnvironment(false);
      fetchMock = vi.fn();
      vi.stubGlobal('fetch', fetchMock);
    });

    afterEach(() => {
      vi.unstubAllGlobals();
    });

    it('surfaces the proxy error body message when present', async () => {
      fetchMock.mockResolvedValue({
        ok: false,
        status: 500,
        json: async () => ({ code: 'GRPC_ERROR', message: 'Out of memory' })
      });
      await expect(tauriCommands.ensureModelReady('model-x')).rejects.toThrow('Out of memory');
    });

    it('proxyGet (via chatModelRecommended) throws a descriptive Error on a non-ok response', async () => {
      fetchMock.mockResolvedValue({ ok: false, status: 503 });

      await expect(tauriCommands.chatModelRecommended()).rejects.toThrow(
        'Proxy /api/agent/recommended-model failed: 503'
      );
    });

    it('proxyPost (via chatModelUnload) throws a descriptive Error on a non-ok response', async () => {
      fetchMock.mockResolvedValue({ ok: false, status: 500 });

      await expect(tauriCommands.chatModelUnload()).rejects.toThrow(
        'Proxy /api/agent/models/unload failed: 500'
      );
    });

    it('proxyDelete (via chatModelDelete) throws a descriptive Error on a non-ok response', async () => {
      fetchMock.mockResolvedValue({ ok: false, status: 404 });

      await expect(tauriCommands.chatModelDelete('missing-model')).rejects.toThrow(
        'Proxy DELETE /api/agent/models/missing-model failed: 404'
      );
    });

    it('proxyPost returns undefined on a 204 response without calling .json()', async () => {
      const json = vi.fn();
      fetchMock.mockResolvedValue({ ok: true, status: 204, json });

      const result = await tauriCommands.chatModelUnload();

      expect(result).toBeUndefined();
      expect(json).not.toHaveBeenCalled();
    });

    it('proxyPost with a body sets Content-Type and JSON-serializes the body', async () => {
      fetchMock.mockResolvedValue({ ok: true, status: 204 });

      await tauriCommands.localAgentCancelTurn('node-42');

      const [, init] = fetchMock.mock.calls[0];
      expect(init.headers).toEqual({ 'Content-Type': 'application/json' });
      expect(init.body).toBe(JSON.stringify({ nodeId: 'node-42' }));
    });

    it('proxyPost with body === undefined sends no Content-Type header and no body', async () => {
      fetchMock.mockResolvedValue({ ok: true, status: 204 });

      await tauriCommands.chatModelDownload('model-x');

      const [, init] = fetchMock.mock.calls[0];
      expect(init.headers).toEqual({});
      expect(init.body).toBeUndefined();
    });

    it('proxyPost parses the JSON body on a non-204 success response', async () => {
      const json = vi.fn().mockResolvedValue({ modelId: 'from-download' });
      fetchMock.mockResolvedValue({ ok: true, status: 200, json });

      const result = await tauriCommands.chatModelDownload('model-x');

      expect(json).toHaveBeenCalled();
      expect(result).toEqual({ modelId: 'from-download' });
    });
  });
});
