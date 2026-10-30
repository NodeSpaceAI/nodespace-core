/**
 * IntegrationsSettings — a failing status command must not read as "no agent
 * installed", and one failing command must not hide the others.
 */
import { describe, it, expect, vi, beforeEach } from 'vitest';
import { render, screen, fireEvent, waitFor } from '@testing-library/svelte';

vi.mock('$lib/utils/logger', () => ({
  createLogger: () => ({ debug: vi.fn(), info: vi.fn(), warn: vi.fn(), error: vi.fn() })
}));

const mockInvoke = vi.fn();
import { mockTauriCore } from '../../helpers/mock-tauri-core';

vi.mock('@tauri-apps/api/core', () =>
  mockTauriCore({ invoke: (...args: unknown[]) => mockInvoke(...args) })
);

import IntegrationsSettings from '$lib/components/settings/sections/integrations-settings.svelte';

const STATUS = { pathAlreadyConfigured: true, pathExportPresent: false };
const SKILL = {
  success: false,
  agentsInstalled: [],
  agentsSkipped: [],
  cliOnPath: true,
  cliWarning: null,
  error: null
};

describe('IntegrationsSettings status loading', () => {
  beforeEach(() => {
    vi.clearAllMocks();
  });

  it('does not claim no agent is installed when the skill status command fails', async () => {
    mockInvoke.mockImplementation((cmd: string) => {
      if (cmd === 'get_integrations_status') return Promise.resolve(STATUS);
      if (cmd === 'get_skill_setup_status') return Promise.reject(new Error('boom'));
      if (cmd === 'detect_agents')
        return Promise.resolve({ agents: ['claude-code'], detectionFailed: false });
      return Promise.resolve(undefined);
    });

    render(IntegrationsSettings);

    expect(await screen.findByText('Not installed')).toBeInTheDocument();
    expect(screen.queryByText(/No supported coding agent/i)).not.toBeInTheDocument();
    expect(screen.getByText('Configured')).toBeInTheDocument();
  });

  it('shows a retryable failed state instead of the no-agent message when detection fails to run', async () => {
    let detectCalls = 0;
    mockInvoke.mockImplementation((cmd: string) => {
      if (cmd === 'get_integrations_status') return Promise.resolve(STATUS);
      if (cmd === 'get_skill_setup_status') return Promise.resolve(SKILL);
      if (cmd === 'detect_agents') {
        detectCalls += 1;
        return detectCalls === 1
          ? Promise.reject(new Error('boom'))
          : Promise.resolve({ agents: ['claude-code'], detectionFailed: false });
      }
      return Promise.resolve(undefined);
    });

    render(IntegrationsSettings);

    expect(await screen.findByText('Could not check agents')).toBeInTheDocument();
    expect(screen.queryByText(/No supported coding agent/i)).not.toBeInTheDocument();
    expect(screen.queryByText('Checking…')).not.toBeInTheDocument();

    await fireEvent.click(screen.getByRole('button', { name: 'Retry' }));

    await waitFor(() => expect(screen.getByText('Not installed')).toBeInTheDocument());
    expect(screen.queryByText('Could not check agents')).not.toBeInTheDocument();
  });

  it('still reports no agent when detection ran and found none', async () => {
    mockInvoke.mockImplementation((cmd: string) => {
      if (cmd === 'get_integrations_status') return Promise.resolve(STATUS);
      if (cmd === 'get_skill_setup_status') return Promise.resolve(SKILL);
      if (cmd === 'detect_agents')
        return Promise.resolve({ agents: [], detectionFailed: false });
      return Promise.resolve(undefined);
    });

    render(IntegrationsSettings);

    expect(await screen.findByText(/No supported coding agent/i)).toBeInTheDocument();
  });
});
