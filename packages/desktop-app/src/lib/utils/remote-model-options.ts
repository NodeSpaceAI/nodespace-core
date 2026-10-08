import type { ProviderConfig } from '$lib/types';

/** One entry of the model selector's "Remote endpoints" group. */
export interface RemoteModelOption {
  /** The `openai-compat:<config-id>:<model>` id the chat node stores. */
  value: string;
  label: string;
}

/**
 * The options of the "Remote endpoints" group: every model the daemon
 * discovered at a configured endpoint, plus each configured provider's own
 * model when discovery did not return it. A provider with an empty `model`
 * defers to the endpoint's default and has nothing to list without discovery.
 */
export function remoteModelOptions(
  discovered: ReadonlyArray<{ id: string; name: string }>,
  providers: ReadonlyArray<ProviderConfig>
): RemoteModelOption[] {
  const options: RemoteModelOption[] = discovered.map((m) => ({ value: m.id, label: m.name }));
  const seen = new Set(options.map((o) => o.value));

  for (const provider of providers) {
    if (!provider.model) continue;
    const value = `openai-compat:${provider.id}:${provider.model}`;
    if (seen.has(value)) continue;
    seen.add(value);
    options.push({ value, label: `${provider.name} · ${provider.model}` });
  }

  return options;
}
