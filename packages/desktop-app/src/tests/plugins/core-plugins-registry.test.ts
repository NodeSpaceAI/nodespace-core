/**
 * Core plugins and the core node type registry (ADR-086 §3) must agree: the
 * frontend defines no plugin for a type that exists nowhere else, and every
 * core type either has a plugin or is on the list of types that deliberately
 * have none.
 */

import { describe, it, expect } from 'vitest';
import { corePlugins } from '$lib/plugins/core-plugins';
import { CORE_NODE_TYPES, isCoreNodeType } from '$lib/types/core-node-types';

/**
 * Core types with no core plugin, and why. Each renders as a read-only entity
 * row (no inline node component) and is opened through its schema-driven
 * properties form, or is created and edited only through a surface of its own.
 */
const CORE_TYPES_WITHOUT_PLUGIN: Record<string, string> = {
  'agent-guidance': 'a named primitive the agent reads; shown as an entity row',
  project: 'a container set up deliberately, not typed in ad hoc; shown as an entity row',
  skill: 'agent-owned content with a schema-driven form; shown as an entity row',
  'database-settings': 'a singleton edited through Settings; shown as an entity row',
  schema: 'edited through the schema surfaces, never as an outline node',
  play: 'authored through the playbook surfaces; shown as an entity row',
  tool: 'registered by the agent runtime, never typed in; shown as an entity row'
};

/**
 * Abstract core types never have a node of exactly their type, so they have no
 * plugin; each concrete subtype carries its own.
 */
const ABSTRACT_CORE_TYPES_WITHOUT_PLUGIN = CORE_NODE_TYPES.filter((t) => t.abstract).map(
  (t) => t.id as string
);

describe('core plugins against the core node type registry', () => {
  it('defines a plugin only for a type in the registry', () => {
    const outside = corePlugins.map((p) => p.id).filter((id) => !isCoreNodeType(id));
    expect(outside).toEqual([]);
  });

  it('defines one plugin per type', () => {
    const ids = corePlugins.map((p) => p.id);
    expect(new Set(ids).size).toBe(ids.length);
  });

  it('gives every other core type a plugin, naming each type that has none', () => {
    const pluginIds = new Set(corePlugins.map((p) => p.id));
    const withoutPlugin = CORE_NODE_TYPES.filter((t) => !t.abstract)
      .map((t) => t.id as string)
      .filter((id) => !pluginIds.has(id));
    expect(withoutPlugin.sort()).toEqual(Object.keys(CORE_TYPES_WITHOUT_PLUGIN).sort());
  });

  it('defines no plugin for an abstract type, and every abstract type is covered by its subtypes', () => {
    const pluginIds = new Set(corePlugins.map((p) => p.id));
    expect(ABSTRACT_CORE_TYPES_WITHOUT_PLUGIN).toContain('ai-chat');
    for (const abstractId of ABSTRACT_CORE_TYPES_WITHOUT_PLUGIN) {
      expect(pluginIds.has(abstractId), `${abstractId} is abstract`).toBe(false);
      const subtypes = CORE_NODE_TYPES.filter((t) => t.parent === abstractId);
      expect(subtypes.length, `${abstractId} has no concrete subtype`).toBeGreaterThan(0);
    }
  });

  it('keeps the list of pluginless types to types in the registry', () => {
    for (const id of Object.keys(CORE_TYPES_WITHOUT_PLUGIN)) {
      expect(isCoreNodeType(id)).toBe(true);
    }
  });
});
