/**
 * Resolves the user's default model (Settings → AI Models) into the
 * `provider` + `model` properties an ai-chat-native node stores, so a chat is
 * created already knowing which model it starts with.
 *
 * The mapping mirrors `handleModelSelect` in `ai-chat-native-node-viewer.svelte`:
 *  - native        → provider 'native', model = the model id
 *  - openai-compat → provider 'openai-compat', model = the daemon's full
 *                    "openai-compat:<config>[:<model>]" id
 *
 * `model` is always a model identifier. A terminal harness is a chat's `agent`,
 * never its model.
 */

import type { AiChatProvider } from '$lib/types/ai-chat-node';
import {
  getDefaultModelSelection,
  getOpenAiConfigs,
  type ModelSelection,
} from '$lib/stores/settings.svelte';

/** The `agent` of a chat NodeSpace's own agent loop answers. */
export const NATIVE_CHAT_AGENT = 'nodespace';

export interface AiChatModelProperties {
  provider: AiChatProvider;
  model: string;
}

/** Convert a selection into node properties; null if it names nothing usable. */
export function selectionToAiChatProperties(
  selection: ModelSelection
): AiChatModelProperties | null {
  if (!selection.modelId && !selection.configId) return null;

  if (selection.provider === 'openai-compat') {
    // A persisted default may carry a bare config UUID (older defaults did);
    // the node stores the daemon's qualified id.
    const model = selection.modelId.startsWith('openai-compat:')
      ? selection.modelId
      : `openai-compat:${selection.configId ?? selection.modelId}`;
    return { provider: 'openai-compat', model };
  }
  if (!selection.modelId) return null;
  return { provider: selection.provider, model: selection.modelId };
}

/**
 * The provider/model to seed a new ai-chat with, or null when there is no
 * default or it is stale (an openai-compat default whose config was removed).
 * A native default that is not downloaded yet is still returned; the send path
 * downloads it on demand.
 */
export function getDefaultAiChatModelProperties(): AiChatModelProperties | null {
  const selection = getDefaultModelSelection();
  if (!selection) return null;

  const props = selectionToAiChatProperties(selection);
  if (!props) return null;

  if (props.provider === 'openai-compat') {
    const configId = props.model.slice('openai-compat:'.length).split(':')[0];
    if (!getOpenAiConfigs().some((c) => c.id === configId)) return null;
  }
  return props;
}
