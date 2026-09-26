/**
 * ModelManager default-model selection encoding tests.
 *
 * Follows the project pattern of testing extracted logic functions directly
 * (not rendering Svelte components) using Happy-DOM.
 *
 * These pin the encode/decode round-trip for the settings "default model"
 * picker. The <select> binds its value to encodeSelection(defaultModel), so if
 * encoding a stored selection produces a string that matches no <option>, the
 * select falls back to "" — the saved default silently reads as "None" with no
 * error anywhere. That makes encodeSelection's totality over ModelSelection a
 * correctness property, not a style preference.
 */

import { describe, it, expect } from 'vitest';
import type { ModelSelection } from '$lib/stores/settings.svelte';

/** Mirrors encodeSelection() from settings/model-manager.svelte. */
function encodeSelection(s: ModelSelection): string {
  if (s.provider === 'openai-compat') {
    return s.modelId.startsWith('openai-compat:')
      ? s.modelId
      : `openai-compat:${s.configId ?? s.modelId}`;
  }
  return `native:${s.modelId}`;
}

/** Mirrors decodeSelection() from settings/model-manager.svelte. */
function decodeSelection(v: string): ModelSelection | null {
  if (v.startsWith('native:')) {
    return { provider: 'native', modelId: v.slice('native:'.length) };
  }
  if (v.startsWith('openai-compat:')) {
    const configId = v.slice('openai-compat:'.length).split(':')[0];
    return { provider: 'openai-compat', modelId: v, configId };
  }
  return null;
}

describe('ModelManager — default model selection encoding', () => {
  it('round-trips a discovered model whose name contains colons', () => {
    // The daemon advertises "openai-compat:<uuid>:<model>", and real model
    // names carry colons ("llama3.1:8b") — so the config UUID is the segment up
    // to the FIRST colon, not the last and not the whole remainder.
    const value = 'openai-compat:abc-123:llama3.1:8b';

    const decoded = decodeSelection(value);
    expect(decoded).toEqual({
      provider: 'openai-compat',
      modelId: 'openai-compat:abc-123:llama3.1:8b',
      configId: 'abc-123',
    });
    expect(encodeSelection(decoded as ModelSelection)).toBe(value);
  });

  it('round-trips a config that contributed no discovered models', () => {
    const value = 'openai-compat:abc-123';

    const decoded = decodeSelection(value);
    expect(decoded).toEqual({
      provider: 'openai-compat',
      modelId: 'openai-compat:abc-123',
      configId: 'abc-123',
    });
    expect(encodeSelection(decoded as ModelSelection)).toBe(value);
  });

  it('qualifies a bare config UUID rather than emitting an unmatchable value', () => {
    // A ModelSelection can legitimately carry a bare UUID as modelId — older
    // persisted defaults stored one. Passing it through unqualified would
    // match no <option> and silently reset the picker to "None".
    expect(
      encodeSelection({
        provider: 'openai-compat',
        modelId: 'abc-123',
        configId: 'abc-123',
      })
    ).toBe('openai-compat:abc-123');
  });

  it('falls back to modelId when a bare selection carries no configId', () => {
    expect(
      encodeSelection({
        provider: 'openai-compat',
        modelId: 'abc-123',
      })
    ).toBe('openai-compat:abc-123');
  });

  it('never double-prefixes an already-qualified id', () => {
    // Normalization must be idempotent: encode(decode(encode(x))) === encode(x).
    const once = encodeSelection({
      provider: 'openai-compat',
      modelId: 'abc-123',
      configId: 'abc-123',
    });
    const twice = encodeSelection(decodeSelection(once) as ModelSelection);
    expect(twice).toBe(once);
    expect(twice.startsWith('openai-compat:openai-compat:')).toBe(false);
  });

  it('round-trips a native model', () => {
    const value = 'native:gemma-4-e4b-q4km';
    const decoded = decodeSelection(value);
    expect(decoded).toEqual({ provider: 'native', modelId: 'gemma-4-e4b-q4km' });
    expect(encodeSelection(decoded as ModelSelection)).toBe(value);
  });
});
