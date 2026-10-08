import { describe, it, expect } from 'vitest';
import { discoveredModelNames, remoteModelOptions } from '$lib/utils/remote-model-options';
import type { ProviderConfig } from '$lib/types';

function provider(overrides: Partial<ProviderConfig> = {}): ProviderConfig {
  return {
    id: 'cfg-1',
    name: 'OpenRouter',
    base_url: 'https://openrouter.ai/api/v1',
    api_key: '',
    model: 'qwen/qwen3.8-flash',
    routing_ok: {},
    ...overrides,
  };
}

describe('remoteModelOptions', () => {
  it('lists one option per configured provider', () => {
    const options = remoteModelOptions([
      provider(),
      provider({ id: 'cfg-2', model: 'anthropic/claude-haiku' }),
    ]);
    expect(options).toEqual([
      {
        value: 'openai-compat:cfg-1:qwen/qwen3.8-flash',
        label: 'OpenRouter · qwen/qwen3.8-flash',
      },
      {
        value: 'openai-compat:cfg-2:anthropic/claude-haiku',
        label: 'OpenRouter · anthropic/claude-haiku',
      },
    ]);
  });

  it('lists nothing for a provider with no configured model', () => {
    expect(remoteModelOptions([provider({ model: '' })])).toEqual([]);
  });

  it('keeps a model name containing colons whole', () => {
    const [option] = remoteModelOptions([provider({ model: 'llama3.1:8b' })]);
    expect(option.value).toBe('openai-compat:cfg-1:llama3.1:8b');
  });
});

describe('discoveredModelNames', () => {
  const discovered = [
    { id: 'openai-compat:cfg-1:qwen/qwen3.8-flash' },
    { id: 'openai-compat:cfg-1:llama3.1:8b' },
    { id: 'openai-compat:cfg-2:other' },
  ];

  it("returns only the named provider's models, colons intact", () => {
    expect(discoveredModelNames(discovered, 'cfg-1')).toEqual([
      'qwen/qwen3.8-flash',
      'llama3.1:8b',
    ]);
  });

  it('returns nothing for a provider discovery did not reach', () => {
    expect(discoveredModelNames(discovered, 'cfg-9')).toEqual([]);
  });
});
