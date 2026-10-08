import { describe, it, expect } from 'vitest';
import { remoteModelOptions } from '$lib/utils/remote-model-options';
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
  it('lists a configured provider model when discovery returns nothing', () => {
    expect(remoteModelOptions([], [provider()])).toEqual([
      {
        value: 'openai-compat:cfg-1:qwen/qwen3.8-flash',
        label: 'OpenRouter · qwen/qwen3.8-flash',
      },
    ]);
  });

  it('does not duplicate a configured model that discovery also returned', () => {
    const id = 'openai-compat:cfg-1:qwen/qwen3.8-flash';
    expect(remoteModelOptions([{ id, name: 'qwen/qwen3.8-flash' }], [provider()])).toEqual([
      { value: id, label: 'qwen/qwen3.8-flash' },
    ]);
  });

  it('keeps discovered models alongside the configured one', () => {
    const options = remoteModelOptions(
      [{ id: 'openai-compat:cfg-1:other', name: 'other' }],
      [provider()]
    );
    expect(options.map((o) => o.value)).toEqual([
      'openai-compat:cfg-1:other',
      'openai-compat:cfg-1:qwen/qwen3.8-flash',
    ]);
  });

  it('lists nothing for a provider with no configured model', () => {
    expect(remoteModelOptions([], [provider({ model: '' })])).toEqual([]);
  });

  it('keeps a model name containing colons whole', () => {
    const [option] = remoteModelOptions([], [provider({ model: 'llama3.1:8b' })]);
    expect(option.value).toBe('openai-compat:cfg-1:llama3.1:8b');
  });
});
