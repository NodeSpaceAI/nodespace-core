// Generated from `packages/nodespace-types` by `bun run gen:types`. Do not edit.

/**
 * Where a native chat's inference runs.
 *
 * `openai-compat` covers every remotely served model, Ollama included: it is
 * reached through its OpenAI-compatible `/v1` endpoint.
 */
export type AiChatProvider = 'native' | 'openai-compat';
