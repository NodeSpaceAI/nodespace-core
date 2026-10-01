/**
 * ai-chat-pty-session — error handling regression coverage.
 *
 * This is the originally-reported bug: launching a PTY agent session calls
 * `ptyLaunchSession()`, a thin wrapper over the Tauri `launch_session` command
 * (`Result<LaunchSessionResult, CommandError>` on the Rust side). Tauri
 * serializes a command's `Err` as a plain JS object on rejection — never an
 * `Error` instance — so the old `e instanceof Error ? e.message : String(e)`
 * catch always fell through to `String(e)`, which stringifies a plain object
 * to the literal `"[object Object]"`, discarding the real message. The fix
 * uses `toError(e).message`, which special-cases a `CommandError`-shaped
 * object via `isCommandError()`.
 */
import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import { render, fireEvent, cleanup } from '@testing-library/svelte';

vi.mock('$lib/utils/logger', () => ({
  createLogger: () => ({ debug: vi.fn(), info: vi.fn(), warn: vi.fn(), error: vi.fn() })
}));

const mockInvoke = vi.fn();
import { mockTauriCore } from '../helpers/mock-tauri-core';

vi.mock('@tauri-apps/api/core', () =>
  mockTauriCore({ invoke: (...args: unknown[]) => mockInvoke(...args) })
);

vi.mock('@tauri-apps/api/event', () => ({
  listen: vi.fn().mockResolvedValue(() => {})
}));

import { listen } from '@tauri-apps/api/event';
import AiChatPtySession from '$lib/components/viewers/ai-chat-pty-session.svelte';
import { sharedNodeStore } from '$lib/services/shared-node-store.svelte';
import type { Node } from '$lib/types/node';

function seedPtyChat(id: string, fields: Record<string, unknown>): void {
  const node = {
    id,
    nodeType: 'ai-chat-pty',
    content: 'Terminal chat',
    version: 1,
    createdAt: '2026-01-01T00:00:00Z',
    modifiedAt: '2026-01-01T00:00:00Z',
    properties: {},
    lifecycleStatus: 'active',
    agent: 'claude-code',
    sessionStatus: 'active',
    ...fields
  } as unknown as Node;
  sharedNodeStore.setNode(node, { type: 'database', reason: 'test' });
}

describe('AiChatPtySession', () => {
  beforeEach(() => {
    mockInvoke.mockReset();
    mockInvoke.mockImplementation((cmd: string) => {
      if (cmd === 'check_agent_availability') return Promise.resolve({ agents: [] });
      return Promise.resolve(undefined);
    });
  });

  afterEach(() => {
    cleanup();
    vi.restoreAllMocks();
  });

  it('surfaces the real CommandError message when launch_session rejects with a plain object, not "[object Object]"', async () => {
    mockInvoke.mockImplementation((cmd: string) => {
      if (cmd === 'check_agent_availability') return Promise.resolve({ agents: [] });
      if (cmd === 'launch_session') {
        // Exactly what Tauri hands back for a Rust `Err(CommandError)` — a
        // plain object, never an Error instance.
        return Promise.reject({
          message: 'Agent binary "claude" not found on PATH',
          code: 'AGENT_NOT_FOUND'
        });
      }
      return Promise.resolve(undefined);
    });

    const { getByText, findByRole } = render(AiChatPtySession, {
      props: { nodeId: 'test-node-1' }
    });

    await fireEvent.click(getByText('Launch'));

    const banner = await findByRole('alert');
    expect(banner.textContent).toContain('Agent binary "claude" not found on PATH');
    expect(banner.textContent).not.toContain('[object Object]');
  });

  it('still handles a genuine Error instance (non-CommandError rejection) correctly', async () => {
    mockInvoke.mockImplementation((cmd: string) => {
      if (cmd === 'check_agent_availability') return Promise.resolve({ agents: [] });
      if (cmd === 'launch_session') return Promise.reject(new Error('daemon unreachable'));
      return Promise.resolve(undefined);
    });

    const { getByText, findByRole } = render(AiChatPtySession, {
      props: { nodeId: 'test-node-2' }
    });

    await fireEvent.click(getByText('Launch'));

    const banner = await findByRole('alert');
    expect(banner.textContent).toContain('daemon unreachable');
  });

  describe('typed fields', () => {
    it('treats sessionStatus "ended" as ended and reads agent, summary and transcript from the typed fields', async () => {
      seedPtyChat('ended-chat', {
        sessionStatus: 'ended',
        sessionId: 'dead-session',
        summary: 'Refactored the parser',
        transcript: 'user: hi\nagent: done'
      });

      const { findByText, getByText, container } = render(AiChatPtySession, {
        props: { nodeId: 'ended-chat' }
      });

      expect(await findByText('Session ended')).toBeTruthy();
      expect(container.querySelector('.pty-ended-agent')?.textContent).toBe('claude-code');
      expect(container.querySelector('.pty-ended-badge')?.textContent).toBe('ended');
      expect(getByText('Refactored the parser')).toBeTruthy();
      expect(container.querySelector('.pty-ended-transcript pre')?.textContent).toContain(
        'agent: done'
      );
      // The PTY is gone: no terminal re-attaches to the stale session id.
      expect(container.querySelector('.pty-terminal-host')).toBeNull();
    });

    it('does not treat the old "archived" value as ended', async () => {
      seedPtyChat('legacy-chat', { sessionStatus: 'archived' });

      const { findByText, queryByText } = render(AiChatPtySession, {
        props: { nodeId: 'legacy-chat' }
      });

      expect(await findByText('Launch agent session')).toBeTruthy();
      expect(queryByText('Session ended')).toBeNull();
    });

    it('pre-selects the harness from agent, not model', async () => {
      seedPtyChat('codex-chat', { agent: 'codex', model: 'claude-code' });

      const { container } = render(AiChatPtySession, { props: { nodeId: 'codex-chat' } });

      await vi.waitFor(() => {
        const select = container.querySelector('#agent-select') as HTMLSelectElement | null;
        expect(select?.value).toBe('codex');
      });
    });

    it('launches by patching agent, session_id and session_status only', async () => {
      seedPtyChat('launch-chat', { agent: 'codex', properties: { 'custom:tag': 'keep' } });
      mockInvoke.mockImplementation((cmd: string) => {
        if (cmd === 'check_agent_availability') return Promise.resolve({ agents: [] });
        if (cmd === 'launch_session') {
          return Promise.resolve({ sessionId: 'new-session', createdAt: 1 });
        }
        return Promise.resolve(undefined);
      });
      // restoreAllMocks in afterEach drops the factory's resolved value.
      vi.mocked(listen).mockResolvedValue(() => {});
      const updateNode = vi.spyOn(sharedNodeStore, 'updateNode');

      const { getByText } = render(AiChatPtySession, { props: { nodeId: 'launch-chat' } });
      await fireEvent.click(getByText('Launch'));

      await vi.waitFor(() => expect(updateNode).toHaveBeenCalled());
      const [id, changes] = updateNode.mock.calls[0];
      expect(id).toBe('launch-chat');
      expect(changes).toEqual({
        properties: { agent: 'codex', session_id: 'new-session', session_status: 'active' }
      });
    });
  });
});
