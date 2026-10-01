/**
 * Schemas store — sidenav built-in filter.
 *
 * `builtInSchemas` surfaces core schemas whose id is in the private
 * SIDENAV_CORE_TYPES set. `project` is a built-in core node type and must
 * appear alongside `task`, while non-core schemas and core schemas outside
 * the set stay excluded. `person` and `agent-guidance` are genuine entity
 * types with no other sidebar presence and are included alongside
 * `task`/`project`/`skill`; every other core type is a
 * primitive/structural block type, app config, or already has a dedicated
 * nav affordance, so it stays excluded.
 */

import { describe, it, expect, beforeEach, vi } from 'vitest';
import type { SchemaNode } from '$lib/types/schema-node';

vi.mock('$lib/utils/logger', () => ({
  createLogger: () => ({ debug: vi.fn(), info: vi.fn(), warn: vi.fn(), error: vi.fn() })
}));

const { mockGetAllSchemas } = vi.hoisted(() => ({
  mockGetAllSchemas: vi.fn(async () => [] as SchemaNode[])
}));
vi.mock('$lib/services/backend-adapter', () => ({
  backendAdapter: {
    getAllSchemas: mockGetAllSchemas
  }
}));

// Neutralize the module-load reconnect registration so importing the store
// does not touch the real daemon-status service.
vi.mock('$lib/services/daemon-status', () => ({
  onDaemonReconnect: vi.fn(() => () => {})
}));

import { schemasStore } from '$lib/stores/schemas.svelte';

function makeSchema(id: string, isCore: boolean): SchemaNode {
  return {
    nodeType: 'schema' as const,
    lifecycleStatus: 'active' as const,
    properties: {},
    id,
    content: id,
    createdAt: '2026-07-28T00:00:00Z',
    modifiedAt: '2026-07-28T00:00:00Z',
    version: 1,
    isCore,
    schemaVersion: 1,
    description: '',
    relationships: [],
    fields: []
  };
}

describe('schemasStore.builtInSchemas — sidenav core types', () => {
  beforeEach(() => {
    schemasStore.schemas = [];
  });

  it('includes the core project schema alongside task and skill', () => {
    schemasStore.schemas = [
      makeSchema('task', true),
      makeSchema('project', true),
      makeSchema('skill', true),
      makeSchema('text', true), // core, but not a sidenav core type
      makeSchema('project', false) // custom schema that happens to share the id
    ];

    const ids = schemasStore.builtInSchemas.map((s) => s.id);

    expect(ids).toContain('project');
    expect(ids).toContain('task');
    expect(ids).toContain('skill');
    // Structural core types and non-core schemas are excluded.
    expect(ids).not.toContain('text');
    // The custom (non-core) project schema is routed to customSchemas instead.
    expect(schemasStore.builtInSchemas.filter((s) => s.id === 'project')).toHaveLength(1);
    expect(schemasStore.customSchemas.map((s) => s.id)).toContain('project');
  });

  it('includes core person and agent-guidance schemas, excludes other core types', () => {
    schemasStore.schemas = [
      makeSchema('task', true),
      makeSchema('project', true),
      makeSchema('skill', true),
      makeSchema('person', true),
      makeSchema('agent-guidance', true),
      // Everything else in the backend's isCore:true list either has its own
      // dedicated nav affordance or is a primitive/app-config type — none of
      // these belong in the sidenav schema-type list.
      makeSchema('text', true),
      makeSchema('date', true),
      makeSchema('header', true),
      makeSchema('code-block', true),
      makeSchema('quote-block', true),
      makeSchema('ordered-list', true),
      makeSchema('horizontal-line', true),
      makeSchema('table', true),
      makeSchema('collection', true),
      makeSchema('checkbox', true),
      makeSchema('ai-chat', true),
      makeSchema('query', true),
      makeSchema('database-settings', true)
    ];

    const ids = schemasStore.builtInSchemas.map((s) => s.id);

    expect(ids).toEqual(
      expect.arrayContaining(['task', 'project', 'skill', 'person', 'agent-guidance'])
    );
    expect(ids).toHaveLength(5);
  });
});

// Unlike every other cross-switch read in the codebase
// (loadChildrenForParent, doLoadChildrenTree, createChat, createCollection),
// loadSchemas committed its fetched array into state without re-checking the
// store generation after the await — a response issued against the previous
// database could land after a switch and get committed as if it belonged to
// the new one. schemasStore now carries its own private `#generation` counter
// (mirroring collectionsData/aiChatsData) rather than the cross-store
// `sharedNodeStore` epoch, so this store stays free of the heavy shared-node-store/
// reactive-structure-tree import chain — it is loaded in lightweight,
// non-Tauri test contexts (e.g. the daemon-readiness e2e harness) that never
// construct that machinery.
describe('schemasStore.loadSchemas stale-response guard across a database switch', () => {
  beforeEach(() => {
    schemasStore.schemas = [];
    mockGetAllSchemas.mockReset();
  });

  it('discards a load that resolves after invalidateForDatabaseSwitch, without writing into the store', async () => {
    let resolveLoad: (schemas: SchemaNode[]) => void = () => {};
    mockGetAllSchemas.mockReturnValue(
      new Promise<SchemaNode[]>((resolve) => {
        resolveLoad = resolve;
      })
    );

    const loadPromise = schemasStore.loadSchemas();

    // The active database switches while the load is still in flight.
    schemasStore.invalidateForDatabaseSwitch();

    resolveLoad([makeSchema('stale-db-schema', false)]);
    await loadPromise;

    expect(schemasStore.schemas).toEqual([]);
  });

  it('a late-resolving load from the previous database does not clobber a fresh load for the new one', async () => {
    // Reproduces the exact failure scenario: DB A's loadSchemas is still in
    // flight when the user switches to DB B. B's fresh load resolves first;
    // A's late response must not then overwrite it.
    let resolveFirst: (schemas: SchemaNode[]) => void = () => {};
    mockGetAllSchemas.mockReturnValueOnce(
      new Promise<SchemaNode[]>((resolve) => {
        resolveFirst = resolve;
      })
    );

    const firstLoad = schemasStore.loadSchemas(); // issued against DB A

    schemasStore.invalidateForDatabaseSwitch(); // switch to DB B
    mockGetAllSchemas.mockResolvedValueOnce([makeSchema('b-schema', false)]);
    const secondLoad = schemasStore.loadSchemas(); // issued against DB B
    await secondLoad;

    expect(schemasStore.schemas.map((s) => s.id)).toEqual(['b-schema']);

    // DB A's stale response finally lands.
    resolveFirst([makeSchema('a-schema', false)]);
    await firstLoad;

    // Still DB B's data — the stale DB A response was dropped, not merged or
    // committed over it.
    expect(schemasStore.schemas.map((s) => s.id)).toEqual(['b-schema']);
  });

  it('a load with no intervening switch still commits normally', async () => {
    mockGetAllSchemas.mockResolvedValue([makeSchema('task', true)]);

    await schemasStore.loadSchemas();

    expect(schemasStore.schemas.map((s) => s.id)).toEqual(['task']);
  });
});
