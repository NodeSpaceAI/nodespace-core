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

// The projects the launch form offers come through the backend adapter.
const mockQueryNodes = vi.fn();
vi.mock('$lib/services/backend-adapter', () => ({
  backendAdapter: { queryNodes: (...args: unknown[]) => mockQueryNodes(...args) }
}));

const mockOpenDialog = vi.fn();
vi.mock('@tauri-apps/plugin-dialog', () => ({
  open: (...args: unknown[]) => mockOpenDialog(...args)
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

/**
 * The daemon as the viewer sees it: no agents detected and no session running,
 * unless a test says otherwise for a command.
 */
function mockDaemon(overrides: Record<string, () => Promise<unknown>> = {}): void {
  mockInvoke.mockImplementation((cmd: string) => {
    if (overrides[cmd]) return overrides[cmd]();
    if (cmd === 'check_agent_availability') return Promise.resolve({ agents: [] });
    if (cmd === 'list_sessions') return Promise.resolve({ sessions: [], count: 0 });
    return Promise.resolve(undefined);
  });
}

describe('AiChatPtySession', () => {
  beforeEach(() => {
    mockInvoke.mockReset();
    mockQueryNodes.mockReset();
    mockQueryNodes.mockResolvedValue([]);
    mockOpenDialog.mockReset();
    mockDaemon();
  });

  afterEach(() => {
    cleanup();
    vi.restoreAllMocks();
  });

  it('surfaces the real CommandError message when launch_session rejects with a plain object, not "[object Object]"', async () => {
    mockDaemon({
      // Exactly what Tauri hands back for a Rust `Err(CommandError)` — a
      // plain object, never an Error instance.
      launch_session: () =>
        Promise.reject({
          message: 'Agent binary "claude" not found on PATH',
          code: 'AGENT_NOT_FOUND'
        })
    });

    const { findByText, findByRole } = render(AiChatPtySession, {
      props: { nodeId: 'test-node-1' }
    });

    await fireEvent.click(await findByText('Launch'));

    const banner = await findByRole('alert');
    expect(banner.textContent).toContain('Agent binary "claude" not found on PATH');
    expect(banner.textContent).not.toContain('[object Object]');
  });

  it('still handles a genuine Error instance (non-CommandError rejection) correctly', async () => {
    mockDaemon({ launch_session: () => Promise.reject(new Error('daemon unreachable')) });

    const { findByText, findByRole } = render(AiChatPtySession, {
      props: { nodeId: 'test-node-2' }
    });

    await fireEvent.click(await findByText('Launch'));

    const banner = await findByRole('alert');
    expect(banner.textContent).toContain('daemon unreachable');
  });

  describe('launching for a project', () => {
    const project = (fields: Record<string, unknown> = {}) => ({
      id: 'p1',
      nodeType: 'project',
      content: 'Widgets',
      title: 'Widgets',
      version: 1,
      status: 'active',
      properties: {},
      ...fields
    });

    /** The `input` of the one `launch_session` call. */
    function launchInput(): Record<string, unknown> {
      const call = mockInvoke.mock.calls.find(([cmd]) => cmd === 'launch_session');
      return (call?.[1] as { input: Record<string, unknown> }).input;
    }

    async function renderWith(projects: unknown[]) {
      mockQueryNodes.mockResolvedValue(projects);
      vi.mocked(listen).mockResolvedValue(() => {});
      mockDaemon({
        launch_session: () =>
          Promise.resolve({ sessionId: 's1', createdAt: 1, workingDir: '/work/widgets' })
      });
      const view = render(AiChatPtySession, { props: { nodeId: 'chat-1' } });
      const select = (await view.findByLabelText('Project')) as HTMLSelectElement;
      await vi.waitFor(() => expect(select.options.length).toBe(projects.length + 1));
      expect(mockQueryNodes).toHaveBeenCalledWith({ nodeType: 'project' });
      return { ...view, select };
    }

    it('launches for no project unless one is chosen', async () => {
      const { findByText, queryByLabelText } = await renderWith([project()]);

      expect(queryByLabelText('Folder on this machine')).toBeNull();
      await fireEvent.click(await findByText('Launch'));

      await vi.waitFor(() => expect(launchInput()).toBeDefined());
      expect(launchInput()).toMatchObject({ nodeId: 'chat-1', projectId: null, projectFolder: null });
    });

    it('asks for the folder of a project that has none on this machine, and sends it', async () => {
      const { findByText, findByLabelText, select } = await renderWith([project()]);

      await fireEvent.change(select, { target: { value: 'p1' } });
      const folder = (await findByLabelText('Folder on this machine')) as HTMLInputElement;
      const launch = (await findByText('Launch')).closest('button') as HTMLElement & {
        disabled: boolean;
      };

      // No folder, no launch: the session would have nowhere to run.
      expect(folder.value).toBe('');
      expect(launch.disabled).toBe(true);

      await fireEvent.input(folder, { target: { value: '/work/widgets' } });
      expect(launch.disabled).toBe(false);
      await fireEvent.click(launch);

      await vi.waitFor(() => expect(launchInput()).toBeDefined());
      expect(launchInput()).toMatchObject({ projectId: 'p1', projectFolder: '/work/widgets' });
    });

    it('fills the folder from the folder picker', async () => {
      mockOpenDialog.mockResolvedValue('/picked/widgets');
      const { findByText, findByLabelText, select } = await renderWith([project()]);

      await fireEvent.change(select, { target: { value: 'p1' } });
      await fireEvent.click(await findByText('Browse…'));

      const folder = (await findByLabelText('Folder on this machine')) as HTMLInputElement;
      await vi.waitFor(() => expect(folder.value).toBe('/picked/widgets'));
      expect(mockOpenDialog).toHaveBeenCalledWith(expect.objectContaining({ directory: true }));
    });

    it('uses the folder the project already has, and names it only when it is changed', async () => {
      const { findByText, findByLabelText, select } = await renderWith([
        project({ checkoutPath: '/work/widgets' })
      ]);

      await fireEvent.change(select, { target: { value: 'p1' } });
      const folder = (await findByLabelText('Folder on this machine')) as HTMLInputElement;
      expect(folder.value).toBe('/work/widgets');

      await fireEvent.click(await findByText('Launch'));

      await vi.waitFor(() => expect(launchInput()).toBeDefined());
      expect(launchInput()).toMatchObject({ projectId: 'p1', projectFolder: null });
    });

    it('sends a changed folder for the daemon to check and store', async () => {
      const { findByText, findByLabelText, select } = await renderWith([
        project({ checkoutPath: '/work/widgets' })
      ]);

      await fireEvent.change(select, { target: { value: 'p1' } });
      const folder = (await findByLabelText('Folder on this machine')) as HTMLInputElement;
      await fireEvent.input(folder, { target: { value: '/elsewhere/widgets' } });
      await fireEvent.click(await findByText('Launch'));

      await vi.waitFor(() => expect(launchInput()).toBeDefined());
      expect(launchInput()).toMatchObject({ projectId: 'p1', projectFolder: '/elsewhere/widgets' });
    });
  });

  describe('typed fields', () => {
    it('treats sessionStatus "ended" as ended and reads agent, summary and transcript from the typed fields', async () => {
      seedPtyChat('ended-chat', {
        sessionStatus: 'ended',
        sessionId: '0c5a8c1e-7d0b-4b5e-9f3a-2f1d6f0f8a01',
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
      // The PTY is gone: there is no session to look for, and no terminal.
      expect(container.querySelector('.pty-terminal-host')).toBeNull();
      expect(mockInvoke).not.toHaveBeenCalledWith('list_sessions');
    });

    it('re-attaches to the session the daemon is running for the node', async () => {
      seedPtyChat('open-chat', { sessionStatus: 'active' });
      mockDaemon({
        list_sessions: () =>
          Promise.resolve({
            sessions: [
              { sessionId: 'other', agentType: 'codex', startedAt: 1, nodeId: 'another-chat' },
              { sessionId: 'no-node', agentType: 'codex', startedAt: 2, nodeId: null },
              // Two for the node: the later launch is the one attached to.
              { sessionId: 'earlier', agentType: 'claude-code', startedAt: 3, nodeId: 'open-chat' },
              { sessionId: 'running', agentType: 'claude-code', startedAt: 9, nodeId: 'open-chat' },
              { sessionId: 'earliest', agentType: 'claude-code', startedAt: 1, nodeId: 'open-chat' }
            ],
            count: 5
          })
      });
      vi.mocked(listen).mockResolvedValue(() => {});

      const { container, queryByText } = render(AiChatPtySession, {
        props: { nodeId: 'open-chat' }
      });

      await vi.waitFor(() =>
        expect(vi.mocked(listen).mock.calls.map(([event]) => event)).toContain(
          'pty-closed-running'
        )
      );
      expect(container.querySelector('.pty-terminal-host')).not.toBeNull();
      expect(queryByText('Launch agent session')).toBeNull();
      const listened = vi.mocked(listen).mock.calls.map(([event]) => event);
      expect(listened).not.toContain('pty-closed-earlier');
      expect(listened).not.toContain('pty-closed-earliest');
    });

    it('offers a launch when the node reads active but no session is running for it', async () => {
      // The harness session id a previous session left is not a running
      // session: nothing re-attaches to it.
      seedPtyChat('stale-chat', {
        sessionStatus: 'active',
        sessionId: '0c5a8c1e-7d0b-4b5e-9f3a-2f1d6f0f8a01'
      });

      const { findByText, container } = render(AiChatPtySession, {
        props: { nodeId: 'stale-chat' }
      });

      expect(await findByText('Launch agent session')).toBeTruthy();
      expect(container.querySelector('.pty-terminal-host')).toBeNull();
    });

    it('offers a launch when the daemon cannot be asked for the running session', async () => {
      seedPtyChat('unreachable-chat', { sessionStatus: 'active' });
      mockDaemon({ list_sessions: () => Promise.reject(new Error('daemon unreachable')) });

      const { findByText } = render(AiChatPtySession, { props: { nodeId: 'unreachable-chat' } });

      expect(await findByText('Launch agent session')).toBeTruthy();
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

    it('launches by patching agent and session_status only', async () => {
      seedPtyChat('launch-chat', { agent: 'codex', properties: { 'custom:tag': 'keep' } });
      mockDaemon({
        launch_session: () => Promise.resolve({ sessionId: 'new-session', createdAt: 1 })
      });
      // restoreAllMocks in afterEach drops the factory's resolved value.
      vi.mocked(listen).mockResolvedValue(() => {});
      const updateNode = vi.spyOn(sharedNodeStore, 'updateNode');

      const { findByText } = render(AiChatPtySession, { props: { nodeId: 'launch-chat' } });
      await fireEvent.click(await findByText('Launch'));

      await vi.waitFor(() => expect(updateNode).toHaveBeenCalled());
      const [id, changes] = updateNode.mock.calls[0];
      expect(id).toBe('launch-chat');
      // The PTY session's id is the daemon's handle on a live process and is
      // not stored: the node's `session_id` is the harness's, written by the
      // daemon when the session ends.
      expect(changes).toEqual({
        properties: { agent: 'codex', session_status: 'active' }
      });
    });
  });
});
