/**
 * AiChatModelSelector Logic Tests
 *
 * Unit tests for the PTY agent selection path, building on the
 * generic PTY entry point.
 * Follows the project pattern of testing extracted logic functions directly
 * (not rendering Svelte components) using Happy-DOM.
 */

import { describe, it, expect, vi } from 'vitest';
import type { ModelSelection } from '$lib/components/viewers/ai-chat-model-selector.svelte';
import { isLocalAgent } from '$lib/stores/agent-store.svelte';
import type { AcpAgentInfo } from '$lib/types/agent-types';

vi.mock('$lib/utils/logger', () => ({
  createLogger: () => ({
    debug: vi.fn(),
    info: vi.fn(),
    warn: vi.fn(),
    error: vi.fn(),
  }),
}));

const PTY_PREFIX = 'pty:';
const SETUP_SENTINEL = '__setup__';
const HEADER_SENTINEL_PREFIX = '__header__';

/** Mirrors handleChange() from ai-chat-model-selector.svelte. */
function handleChangeValue(
  value: string,
  onSelect: (selection: ModelSelection) => void
): void {
  if (!value || value.startsWith(HEADER_SENTINEL_PREFIX)) return;
  if (value === SETUP_SENTINEL) return;

  if (value.startsWith('native:')) {
    onSelect({ provider: 'native', modelId: value.slice('native:'.length) });
    return;
  }
  if (value.startsWith('openai-compat:')) {
    // The config UUID is the segment up to the FIRST colon; the remainder is
    // the discovered model name, which may itself contain colons.
    const rest = value.slice('openai-compat:'.length);
    onSelect({
      provider: 'openai-compat',
      modelId: value,
      configId: rest.split(':')[0],
    });
    return;
  }
  if (value.startsWith(PTY_PREFIX)) {
    onSelect({ provider: 'pty', modelId: value.slice(PTY_PREFIX.length) });
    return;
  }
}

/** Mirrors the ptyAgents/availablePtyAgents derivations in ai-chat-model-selector.svelte. */
function ptyAgents(agents: AcpAgentInfo[]): AcpAgentInfo[] {
  return agents.filter((a) => !isLocalAgent(a.id));
}

describe('AiChatModelSelector — PTY agent selection', () => {
  it('selecting a PTY agent invokes onSelect with provider "pty" and the agent id as modelId', () => {
    const onSelect = vi.fn();
    handleChangeValue(`${PTY_PREFIX}claude-code`, onSelect);

    expect(onSelect).toHaveBeenCalledTimes(1);
    expect(onSelect).toHaveBeenCalledWith({ provider: 'pty', modelId: 'claude-code' });
  });

  it('does not invoke onSelect for header or setup sentinels', () => {
    const onSelect = vi.fn();
    handleChangeValue(`${HEADER_SENTINEL_PREFIX}no-local`, onSelect);
    handleChangeValue(SETUP_SENTINEL, onSelect);

    expect(onSelect).not.toHaveBeenCalled();
  });

  it('other provider selections remain unaffected by the PTY addition', () => {
    const onSelect = vi.fn();
    handleChangeValue('native:gemma-4-e4b-q4km', onSelect);
    expect(onSelect).toHaveBeenCalledWith({ provider: 'native', modelId: 'gemma-4-e4b-q4km' });

    onSelect.mockClear();
    handleChangeValue('openai-compat:abc-123', onSelect);
    expect(onSelect).toHaveBeenCalledWith({
      provider: 'openai-compat',
      modelId: 'openai-compat:abc-123',
      configId: 'abc-123',
    });

    // A discovered model's own name routinely contains a colon
    // ("llama3.1:8b"), so only the segment before the FIRST one is the config
    // UUID — the full value stays the model id the daemon advertised.
    onSelect.mockClear();
    handleChangeValue('openai-compat:abc-123:llama3.1:8b', onSelect);
    expect(onSelect).toHaveBeenCalledWith({
      provider: 'openai-compat',
      modelId: 'openai-compat:abc-123:llama3.1:8b',
      configId: 'abc-123',
    });
  });
});

describe('AiChatModelSelector — PTY agent list derivation', () => {
  const agents: AcpAgentInfo[] = [
    {
      id: 'local:gemma-4-e4b-q4km',
      name: 'Gemma 4 E4B Instruct Q4_K_M',
      binary: 'local',
      args: [],
      auth_method: { method: 'agent_managed' },
      available: true,
    },
    {
      id: 'claude-code',
      name: 'Claude Code',
      binary: 'claude',
      args: [],
      auth_method: { method: 'agent_managed' },
      available: true,
    },
    {
      id: 'antigravity-cli',
      name: 'Antigravity CLI',
      binary: 'agy',
      args: [],
      auth_method: { method: 'env_api_key', var_name: 'GEMINI_API_KEY' },
      available: false,
    },
  ];

  it('excludes local model agents from the PTY Agents section', () => {
    const result = ptyAgents(agents);
    expect(result.map((a) => a.id)).toEqual(['claude-code', 'antigravity-cli']);
  });

  it('keeps unavailable PTY agents in the list (rendered disabled, not hidden)', () => {
    const result = ptyAgents(agents);
    const antigravity = result.find((a) => a.id === 'antigravity-cli');
    expect(antigravity?.available).toBe(false);
  });
});
