/**
 * SchemaFormLoader tests — generic schema fallback gating.
 *
 * The loader must fetch the generic, schema-driven form for any type without a hardcoded
 * one. It previously gated that fetch on core/custom classification, which starved core
 * types that ship no frontend form (`project`) of any properties UI at all.
 */

import { describe, it, expect, beforeAll, beforeEach, vi } from 'vitest';
import { SchemaFormLoader } from '$lib/design/components/schema-form-loader.svelte';
import { backendAdapter } from '$lib/services/backend-adapter';
import { taskNodePlugin, personNodePlugin } from '$lib/plugins/core-plugins';
import type { SchemaNode } from '$lib/types/schema-node';

vi.mock('$lib/services/backend-adapter', () => ({
  backendAdapter: {
    getSchema: vi.fn()
  }
}));

const getSchema = vi.mocked(backendAdapter.getSchema);

function schemaFor(id: string): SchemaNode {
  return {
    lifecycleStatus: 'active' as const,
    properties: {},
    id,
    nodeType: 'schema',
    content: id,
    createdAt: new Date().toISOString(),
    modifiedAt: new Date().toISOString(),
    version: 1,
    isCore: true,
    schemaVersion: 1,
    relationships: [],
    fields: []
  };
}

describe('SchemaFormLoader', () => {
  // task-schema-form.svelte and person-schema-form.svelte are both large
  // components with sizable import subtrees of their own; the FIRST dynamic
  // import() of either pays a one-time transform cost (~2-5s, more under CPU
  // load) that has no headroom against vitest's 5s default testTimeout —
  // every import() after that resolves from Vite's module cache in well
  // under a millisecond. Warm both here (same pattern as
  // core-plugins.test.ts's viewer warm-up) so loadForm('task')/('person')
  // below measure only the already-warm resolution, not a one-time compile
  // race. The 30s hook timeout is generous on purpose: this is explicitly
  // the place that absorbs the compile cost.
  beforeAll(async () => {
    await Promise.all([
      taskNodePlugin.schemaForm?.lazyLoad?.(),
      personNodePlugin.schemaForm?.lazyLoad?.()
    ]);
  }, 30000);

  beforeEach(() => {
    vi.clearAllMocks();
  });

  it('loads the generic schema for a core type with no hardcoded form', async () => {
    getSchema.mockResolvedValue(schemaFor('project'));
    const loader = new SchemaFormLoader();

    // `project` registers no schema form, so the generic fallback must be fetched.
    expect(await loader.loadForm('project')).toBe(false);
    await vi.waitFor(() => expect(loader.genericSchema?.id).toBe('project'));
    expect(getSchema).toHaveBeenCalledWith('project');
  });

  it('does not fetch a generic schema when a hardcoded form is registered', async () => {
    const loader = new SchemaFormLoader();

    // `task` registers TaskSchemaForm — the hardcoded form wins, no generic fetch.
    expect(await loader.loadForm('task')).toBe(true);
    expect(getSchema).not.toHaveBeenCalled();
    expect(loader.genericSchema).toBeNull();
  });

  describe('hasTitleTemplate — hardcoded-form types', () => {
    /**
     * `person` (like `task`) registers a hardcoded form, so it never populates
     * `genericSchema` — the only thing `hasTitleTemplate` used to consult. It's the
     * first core type to ship a title_template, so these pin the plugin-registry
     * fallback that makes its header (and any other title_template-driven hardcoded
     * form) correctly read-only.
     */

    it('is true for a hardcoded-form type whose plugin declares hasTitleTemplate', async () => {
      const loader = new SchemaFormLoader();

      await loader.loadForm('person');

      expect(loader.hasTitleTemplate).toBe(true);
      // Confirms this comes from the plugin registry, not a generic-schema fetch.
      expect(getSchema).not.toHaveBeenCalled();
    });

    it('is false for a hardcoded-form type with no title_template (task)', async () => {
      const loader = new SchemaFormLoader();

      await loader.loadForm('task');

      expect(loader.hasTitleTemplate).toBe(false);
    });

    it('is false before any type has been loaded', () => {
      const loader = new SchemaFormLoader();

      expect(loader.hasTitleTemplate).toBe(false);
    });

    it('flips correctly across navigation between a title_template type and a plain one', async () => {
      const loader = new SchemaFormLoader();

      await loader.loadForm('person');
      expect(loader.hasTitleTemplate).toBe(true);

      // Navigating away resets currentNodeType before the next loadForm sets it —
      // exactly the sequence base-node-viewer.svelte calls on every node change.
      loader.resetGenericSchema();
      await loader.loadForm('task');
      expect(loader.hasTitleTemplate).toBe(false);

      loader.resetGenericSchema();
      await loader.loadForm('person');
      expect(loader.hasTitleTemplate).toBe(true);
    });

    it('is false immediately after resetGenericSchema, before the next loadForm', async () => {
      const loader = new SchemaFormLoader();

      await loader.loadForm('person');
      expect(loader.hasTitleTemplate).toBe(true);

      loader.resetGenericSchema();

      // Between reset and the next loadForm — the state a render could observe if a
      // navigation boundary is reached mid-transition — the previous type's answer
      // must not leak through.
      expect(loader.hasTitleTemplate).toBe(false);
    });
  });

  it('repopulates the generic schema when revisiting a type after a reset', async () => {
    getSchema.mockResolvedValue(schemaFor('project'));
    const loader = new SchemaFormLoader();

    await loader.loadForm('project');
    await vi.waitFor(() => expect(loader.genericSchema).not.toBeNull());

    // Navigating away clears the schema; the form-component lookup stays cached. Revisiting
    // must still repopulate the schema, or the properties panel renders nothing.
    loader.resetGenericSchema();
    expect(loader.genericSchema).toBeNull();

    await loader.loadForm('project');
    await vi.waitFor(() => expect(loader.genericSchema?.id).toBe('project'));

    // Served from cache — schema definitions are immutable for the session.
    expect(getSchema).toHaveBeenCalledTimes(1);
  });

  it('does not re-fetch a type already known to have no schema', async () => {
    getSchema.mockRejectedValue(new Error('schema not found'));
    const loader = new SchemaFormLoader();

    // Structural types (text, header, …) have no schema. Navigating between them must not
    // issue a backend round trip per navigation.
    await loader.loadForm('horizontal-line');
    await vi.waitFor(() => expect(getSchema).toHaveBeenCalledTimes(1));

    loader.resetGenericSchema();
    await loader.loadForm('horizontal-line');
    await loader.loadForm('horizontal-line');

    expect(getSchema).toHaveBeenCalledTimes(1);
    expect(loader.genericSchema).toBeNull();
  });

  it('leaves genericSchema null when the backend has no schema for the type', async () => {
    getSchema.mockRejectedValue(new Error('schema not found'));
    const loader = new SchemaFormLoader();

    // A type with no schema at all must not render an empty form shell — the lookup is
    // swallowed and genericSchema stays null, so the viewer renders no properties panel.
    expect(await loader.loadForm('horizontal-line')).toBe(false);
    await vi.waitFor(() => expect(getSchema).toHaveBeenCalledWith('horizontal-line'));
    expect(loader.genericSchema).toBeNull();
  });

  it('ignores a response that is not a schema node', async () => {
    getSchema.mockResolvedValue({ id: 'text', nodeType: 'text' } as unknown as SchemaNode);
    const loader = new SchemaFormLoader();

    await loader.loadForm('text');
    await vi.waitFor(() => expect(getSchema).toHaveBeenCalledWith('text'));
    expect(loader.genericSchema).toBeNull();
  });

  describe('stale-response guard', () => {
    /**
     * Fetches resolve after the viewer may have navigated away, and `genericSchema` feeds
     * `hasTitleTemplate`, which drives the header's readonly state. A superseded response
     * must never be published — these pin the ordering that guarantees it.
     */

    /** A getSchema mock whose per-type promises resolve only when told to. */
    function deferredSchemas() {
      const resolvers = new Map<string, (schema: SchemaNode) => void>();
      getSchema.mockImplementation(
        (nodeType: string) =>
          new Promise((resolve) => {
            resolvers.set(nodeType, resolve as (schema: SchemaNode) => void);
          })
      );
      return (nodeType: string) => resolvers.get(nodeType)?.(schemaFor(nodeType));
    }

    it('drops a response for a type the viewer has navigated away from', async () => {
      const resolve = deferredSchemas();
      const loader = new SchemaFormLoader();

      loader.loadForm('project');
      await vi.waitFor(() => expect(getSchema).toHaveBeenCalledWith('project'));

      // Navigate to a second generic type before the first fetch settles.
      loader.resetGenericSchema();
      loader.loadForm('invoice');
      await vi.waitFor(() => expect(getSchema).toHaveBeenCalledWith('invoice'));

      resolve('project');
      await vi.waitFor(() => expect(getSchema).toHaveBeenCalledTimes(2));
      expect(loader.genericSchema).toBeNull();

      resolve('invoice');
      await vi.waitFor(() => expect(loader.genericSchema?.id).toBe('invoice'));
    });

    it('drops a response after navigating to a type that has a hardcoded form', async () => {
      const resolve = deferredSchemas();
      const loader = new SchemaFormLoader();

      loader.loadForm('project');
      await vi.waitFor(() => expect(getSchema).toHaveBeenCalledWith('project'));

      // `task` ships a hardcoded form, so this navigation never fetches a generic schema.
      // The reset is the only thing marking the in-flight `project` fetch as superseded.
      loader.resetGenericSchema();

      resolve('project');
      await vi.waitFor(() => expect(getSchema).toHaveBeenCalledTimes(1));
      expect(loader.genericSchema).toBeNull();
    });
  });
});
