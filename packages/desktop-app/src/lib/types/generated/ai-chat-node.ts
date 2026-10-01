// Generated from `packages/nodespace-types` by `bun run gen:types`. Do not edit.
import type { AiChatMessage } from './ai-chat-message';
import type { NodeReference } from './node-reference';

/**
 * Wire shape for ai-chat nodes sent to the frontend.
 *
 * Produced by `node_to_typed_value` for `node_type == "ai-chat"`. Fields map
 * directly to the TypeScript `AiChatNode` interface.
 */
export type AiChatNode = {
  /**
   * Inference turn state (`"idle"` / `"processing"`), daemon-owned.
   */
  turnStatus: string;
  /**
   * Session lifecycle (`"active"` / `"archived"`), PTY-owned. Independent
   * of `turn_status` — see `nodespace_core::models::AiChatNode`'s module
   * docs for why these are two properties rather than one shared `status`.
   */
  sessionStatus: string;
  provider?: string;
  model?: string;
  messages: Array<AiChatMessage>;
  id: string;
  nodeType: string;
  content: string;
  version: number;
  createdAt: string;
  modifiedAt: string;
  properties: Record<string, unknown>;
  mentions?: Array<string>;
  mentionedIn?: Array<NodeReference>;
  title?: string | null;
  lifecycleStatus: string;
};
