// Generated from `packages/nodespace-types` by `bun run gen:types`. Do not edit.
import type { CoreTypeEntry } from './core-type-entry';

/** Every core type, in registry order. */
export const CORE_NODE_TYPES = [
  { id: 'text', parent: null, abstract: false, mentionable: true },
  { id: 'header', parent: null, abstract: false, mentionable: true },
  { id: 'code-block', parent: null, abstract: false, mentionable: true },
  { id: 'quote-block', parent: null, abstract: false, mentionable: true },
  { id: 'ordered-list', parent: null, abstract: false, mentionable: true },
  { id: 'checkbox', parent: null, abstract: false, mentionable: true },
  { id: 'horizontal-line', parent: null, abstract: false, mentionable: true },
  { id: 'table', parent: null, abstract: false, mentionable: true },
  { id: 'date', parent: null, abstract: false, mentionable: true },
  { id: 'agent-guidance', parent: null, abstract: false, mentionable: true },
  { id: 'task', parent: null, abstract: false, mentionable: true },
  { id: 'project', parent: null, abstract: false, mentionable: true },
  { id: 'person', parent: null, abstract: false, mentionable: true },
  { id: 'collection', parent: null, abstract: false, mentionable: false },
  { id: 'skill', parent: null, abstract: false, mentionable: true },
  { id: 'database-settings', parent: null, abstract: false, mentionable: true },
  { id: 'query', parent: null, abstract: false, mentionable: true },
  { id: 'schema', parent: null, abstract: false, mentionable: false },
  { id: 'play', parent: null, abstract: false, mentionable: true },
  { id: 'ai-chat', parent: null, abstract: false, mentionable: false },
  { id: 'tool', parent: null, abstract: false, mentionable: true }
] as const satisfies readonly CoreTypeEntry[];
