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

import AiChatPtySession from '$lib/components/viewers/ai-chat-pty-session.svelte';

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
});
