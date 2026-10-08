import type { ProviderConfig } from '$lib/types';

/** One selectable remote model: a configured provider's endpoint and model. */
export interface RemoteModelOption {
  /** The `openai-compat:<config-id>:<model>` id the chat node stores. */
  value: string;
  label: string;
}

const PREFIX = 'openai-compat:';

/**
 * One option per configured provider. A provider is an endpoint plus the one
 * model it targets, so a second model of the same endpoint is a second
 * provider. What an endpoint's `/models` lists is not offered here: it feeds
 * the provider form's model suggestions.
 */
export function remoteModelOptions(providers: ReadonlyArray<ProviderConfig>): RemoteModelOption[] {
  return providers
    .filter((p) => p.model)
    .map((p) => ({
      value: `${PREFIX}${p.id}:${p.model}`,
      label: `${p.name} · ${p.model}`,
    }));
}

/**
 * The model names the daemon discovered at one provider's endpoint, from its
 * `openai-compat:<config-id>:<model>` ids. A model name may contain colons
 * ("llama3.1:8b"); a config id never does.
 */
export function discoveredModelNames(
  discovered: ReadonlyArray<{ id: string }>,
  configId: string
): string[] {
  const prefix = `${PREFIX}${configId}:`;
  return discovered.filter((m) => m.id.startsWith(prefix)).map((m) => m.id.slice(prefix.length));
}
