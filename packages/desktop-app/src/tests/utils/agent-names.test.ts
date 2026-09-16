import { describe, it, expect } from 'vitest';
import { displayAgentName, formatAgentList } from '$lib/utils/agent-names';

describe('displayAgentName', () => {
  it('maps every installer agent id to its product name', () => {
    expect(displayAgentName('claude-code')).toBe('Claude Code');
    expect(displayAgentName('codex')).toBe('Codex');
    expect(displayAgentName('antigravity')).toBe('Antigravity CLI');
    expect(displayAgentName('opencode')).toBe('OpenCode');
    expect(displayAgentName('pi')).toBe('Pi');
  });

  /**
   * An agent added to packages/skill's AGENTS but not yet to the display map
   * must still render as something, not as blank or "undefined" — the raw id
   * is ugly but honest, and the wizard keeps working until the map catches up.
   */
  it('falls back to the raw id for an agent it does not know', () => {
    expect(displayAgentName('some-future-agent')).toBe('some-future-agent');
  });
});

describe('formatAgentList', () => {
  it('renders nothing for an empty list', () => {
    expect(formatAgentList([])).toBe('');
  });

  it('renders a single agent with no conjunction', () => {
    expect(formatAgentList(['claude-code'])).toBe('Claude Code');
  });

  it('joins two agents with "and", no comma', () => {
    expect(formatAgentList(['claude-code', 'antigravity'])).toBe('Claude Code and Antigravity CLI');
  });

  it('uses a serial comma for three or more agents', () => {
    expect(formatAgentList(['claude-code', 'antigravity', 'codex'])).toBe(
      'Claude Code, Antigravity CLI, and Codex'
    );
  });

  it('applies display names to every entry, not just the first', () => {
    expect(formatAgentList(['opencode', 'pi'])).toBe('OpenCode and Pi');
  });
});
