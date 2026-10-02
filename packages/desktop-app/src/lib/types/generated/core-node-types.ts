// Generated from `packages/nodespace-types` by `bun run gen:types`. Do not edit.
import type { CoreTypeEntry } from './core-type-entry';

/** Every core type, in registry order. */
export const CORE_NODE_TYPES = [
  {
    id: 'text',
    parent: null,
    abstract: false,
    mentionable: true,
    typedUpdate: false,
    structure: { children: { rule: 'any' }, parent: { rule: 'any' } }
  },
  {
    id: 'header',
    parent: null,
    abstract: false,
    mentionable: true,
    typedUpdate: false,
    structure: { children: { rule: 'any' }, parent: { rule: 'any' } }
  },
  {
    id: 'code-block',
    parent: null,
    abstract: false,
    mentionable: true,
    typedUpdate: false,
    structure: { children: { rule: 'none' }, parent: { rule: 'any' } }
  },
  {
    id: 'quote-block',
    parent: null,
    abstract: false,
    mentionable: true,
    typedUpdate: false,
    structure: { children: { rule: 'any' }, parent: { rule: 'any' } }
  },
  {
    id: 'ordered-list',
    parent: null,
    abstract: false,
    mentionable: true,
    typedUpdate: false,
    structure: { children: { rule: 'none' }, parent: { rule: 'any' } }
  },
  {
    id: 'checkbox',
    parent: null,
    abstract: false,
    mentionable: true,
    typedUpdate: false,
    structure: { children: { rule: 'any' }, parent: { rule: 'any' } }
  },
  {
    id: 'horizontal-line',
    parent: null,
    abstract: false,
    mentionable: true,
    typedUpdate: false,
    structure: { children: { rule: 'none' }, parent: { rule: 'any' } }
  },
  {
    id: 'table',
    parent: null,
    abstract: false,
    mentionable: true,
    typedUpdate: false,
    structure: { children: { rule: 'none' }, parent: { rule: 'any' } }
  },
  {
    id: 'date',
    parent: null,
    abstract: false,
    mentionable: true,
    typedUpdate: false,
    structure: { children: { rule: 'any' }, parent: { rule: 'must_be_root' } }
  },
  {
    id: 'agent-guidance',
    parent: null,
    abstract: false,
    mentionable: true,
    typedUpdate: false,
    structure: { children: { rule: 'any' }, parent: { rule: 'any' } }
  },
  {
    id: 'task',
    parent: null,
    abstract: false,
    mentionable: true,
    typedUpdate: true,
    structure: { children: { rule: 'any' }, parent: { rule: 'any' } }
  },
  {
    id: 'project',
    parent: null,
    abstract: false,
    mentionable: true,
    typedUpdate: true,
    structure: { children: { rule: 'any' }, parent: { rule: 'any' } }
  },
  {
    id: 'person',
    parent: null,
    abstract: false,
    mentionable: true,
    typedUpdate: true,
    structure: { children: { rule: 'any' }, parent: { rule: 'any' } }
  },
  {
    id: 'collection',
    parent: null,
    abstract: false,
    mentionable: false,
    typedUpdate: true,
    structure: { children: { rule: 'any' }, parent: { rule: 'must_be_root' } }
  },
  {
    id: 'skill',
    parent: null,
    abstract: false,
    mentionable: true,
    typedUpdate: true,
    structure: { children: { rule: 'any' }, parent: { rule: 'any' } }
  },
  {
    id: 'database-settings',
    parent: null,
    abstract: false,
    mentionable: true,
    typedUpdate: true,
    structure: { children: { rule: 'none' }, parent: { rule: 'any' } }
  },
  {
    id: 'query',
    parent: null,
    abstract: false,
    mentionable: true,
    typedUpdate: true,
    structure: { children: { rule: 'none' }, parent: { rule: 'any' } }
  },
  {
    id: 'schema',
    parent: null,
    abstract: false,
    mentionable: false,
    typedUpdate: false,
    structure: { children: { rule: 'any' }, parent: { rule: 'must_be_root' } }
  },
  {
    id: 'play',
    parent: null,
    abstract: false,
    mentionable: true,
    typedUpdate: true,
    structure: { children: { rule: 'any' }, parent: { rule: 'any' } }
  },
  {
    id: 'ai-chat',
    parent: null,
    abstract: true,
    mentionable: false,
    typedUpdate: false,
    structure: { children: { rule: 'any' }, parent: { rule: 'any' } }
  },
  {
    id: 'ai-chat-native',
    parent: 'ai-chat',
    abstract: false,
    mentionable: false,
    typedUpdate: false,
    structure: { children: { rule: 'any' }, parent: { rule: 'any' } }
  },
  {
    id: 'ai-chat-pty',
    parent: 'ai-chat',
    abstract: false,
    mentionable: false,
    typedUpdate: false,
    structure: { children: { rule: 'any' }, parent: { rule: 'any' } }
  },
  {
    id: 'tool',
    parent: null,
    abstract: false,
    mentionable: true,
    typedUpdate: false,
    structure: { children: { rule: 'none' }, parent: { rule: 'any' } }
  }
] as const satisfies readonly CoreTypeEntry[];
