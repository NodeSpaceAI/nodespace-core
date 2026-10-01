// Generated from `packages/nodespace-types` by `bun run gen:types`. Do not edit.
import type { CoreTypeEntry } from './core-type-entry';

/** Every core type, in registry order. */
export const CORE_NODE_TYPES = [
  { id: 'text', parent: null, abstract: false, mentionable: true, typedUpdate: false },
  { id: 'header', parent: null, abstract: false, mentionable: true, typedUpdate: false },
  { id: 'code-block', parent: null, abstract: false, mentionable: true, typedUpdate: false },
  { id: 'quote-block', parent: null, abstract: false, mentionable: true, typedUpdate: false },
  { id: 'ordered-list', parent: null, abstract: false, mentionable: true, typedUpdate: false },
  { id: 'checkbox', parent: null, abstract: false, mentionable: true, typedUpdate: false },
  { id: 'horizontal-line', parent: null, abstract: false, mentionable: true, typedUpdate: false },
  { id: 'table', parent: null, abstract: false, mentionable: true, typedUpdate: false },
  { id: 'date', parent: null, abstract: false, mentionable: true, typedUpdate: false },
  { id: 'agent-guidance', parent: null, abstract: false, mentionable: true, typedUpdate: false },
  { id: 'task', parent: null, abstract: false, mentionable: true, typedUpdate: true },
  { id: 'project', parent: null, abstract: false, mentionable: true, typedUpdate: true },
  { id: 'person', parent: null, abstract: false, mentionable: true, typedUpdate: true },
  { id: 'collection', parent: null, abstract: false, mentionable: false, typedUpdate: false },
  { id: 'skill', parent: null, abstract: false, mentionable: true, typedUpdate: false },
  { id: 'database-settings', parent: null, abstract: false, mentionable: true, typedUpdate: false },
  { id: 'query', parent: null, abstract: false, mentionable: true, typedUpdate: true },
  { id: 'schema', parent: null, abstract: false, mentionable: false, typedUpdate: false },
  { id: 'play', parent: null, abstract: false, mentionable: true, typedUpdate: false },
  { id: 'ai-chat', parent: null, abstract: true, mentionable: false, typedUpdate: false },
  {
    id: 'ai-chat-native',
    parent: 'ai-chat',
    abstract: false,
    mentionable: false,
    typedUpdate: false
  },
  { id: 'ai-chat-pty', parent: 'ai-chat', abstract: false, mentionable: false, typedUpdate: false },
  { id: 'tool', parent: null, abstract: false, mentionable: true, typedUpdate: false }
] as const satisfies readonly CoreTypeEntry[];
