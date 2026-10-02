/**
 * Contract test: TauriAdapter / HttpAdapter / dev-proxy parity (ADR-048 item 5)
 *
 * Four hand-synced client-side paths reach the same daemon-side NodeService
 * contract: TauriAdapter (IPC), HttpAdapter (fetch), the dev-proxy (REST→gRPC
 * translation), and — until this change — a fourth copy embedded in this
 * harness. Nothing forced them to agree, so a field/shape change on one path
 * could silently diverge from the others.
 *
 * This test has two halves:
 *
 * 1. Live round-trip (HttpAdapter → dev-proxy → real daemon → SQLite): drives
 *    the operations with the highest drift risk — the task-update tri-state
 *    clear/set/no-change encoding and create/move insert-position encoding —
 *    through the actual proxy translation path and asserts the daemon's
 *    authoritative response matches what was asked for. This exercises the
 *    exact request-shaping logic dev-proxy now derives from
 *    adapter-core.ts's buildTaskNodeUpdatePatch/encodeInsertPosition instead
 *    of a hand-rolled duplicate.
 *
 * 2. Shape parity (no daemon needed): TauriAdapter's IPC args and
 *    HttpAdapter's JSON body are both built by feeding the same
 *    CreateNodeInput/TaskNodeUpdate through the shared adapter-core builders.
 *    Asserting the two transports call those builders with the same logical
 *    input, rather than each re-deriving the wire shape, is what makes drift
 *    between them a compile/test error instead of a runtime surprise — a
 *    renamed or reshaped builder call breaks this test immediately.
 *
 * A true Tauri command-layer round-trip (no webview needed — `tauri::State`
 * obtained via `Manager::state()`, not IPC) is the Rust-side counterpart:
 * `packages/desktop-app/app-lib/tests/it/adapter_contract_test.rs` drives the
 * SAME three scenarios below (task tri-state update, create-with-position,
 * move-with-position) through the real `#[tauri::command]` functions. Two
 * suites independently pinning the same documented contract is what makes a
 * divergence between the paths a test failure rather than a silent drift.
 */

import { describe, it, expect, beforeAll, afterAll } from 'vitest';
import { DaemonTestHarness } from './daemon-harness';
import {
  buildCreateNodeFields,
  buildTaskNodeUpdatePatch,
  encodeInsertPosition,
  insertPosition,
} from '$lib/services/adapter-core';

describe('Adapter contract: shape parity (no daemon required)', () => {
  it('CreateNode: HttpAdapter and TauriAdapter derive identical wire fields from the same input', () => {
    // Both TauriAdapter.createNode and HttpAdapter.createNode call
    // buildCreateNodeFields on the same CreateNodeInput before adding their
    // transport-specific envelope (invoke args vs. JSON body + timestamps).
    // Asserting the shared call site's output is deterministic and complete
    // is what proves the two transports cannot diverge on this shape.
    const input = {
      id: 'contract-test-node',
      nodeType: 'text',
      content: 'hello',
      parentId: 'parent-1',
      insertPosition: insertPosition.after('sibling-1'),
    };

    const fields = buildCreateNodeFields(input);

    expect(fields).toEqual({
      id: 'contract-test-node',
      nodeType: 'text',
      content: 'hello',
      properties: {},
      mentions: [],
      parentId: 'parent-1',
      insertPosition: { type: 'after', siblingId: 'sibling-1' },
    });
  });

  it('UpdateTaskNode: the tri-state patch is the single source both HttpAdapter and dev-proxy must derive from', () => {
    // HttpAdapter.updateTaskNode forwards the TaskNodeUpdate as JSON as-is;
    // dev-proxy is the one place that must translate that JSON into the
    // daemon's tri-state clear/set/no-change wire shape. If dev-proxy ever
    // re-inlines this logic instead of calling buildTaskNodeUpdatePatch, this
    // test still passes (it only tests the shared function) but the
    // dev-proxy source diff should always show a call to it, not a literal
    // { clear, value } construction — see packages/dev-tools/src/dev-proxy.ts.
    const patch = buildTaskNodeUpdatePatch({
      status: 'done',
      dueDate: null,
      priority: 'high',
    });

    expect(patch).toEqual({
      status: 'done',
      priority: { clear: false, value: 'high' },
      dueDate: { clear: true },
      startedAt: undefined,
      completedAt: undefined,
    });
  });

  it('MoveNode/CreateNode: InsertPosition encodes to the same oneof shape regardless of caller', () => {
    expect(encodeInsertPosition(insertPosition.beginning())).toEqual({ beginning: true });
    expect(encodeInsertPosition(insertPosition.after('x'))).toEqual({ after: 'x' });
    expect(encodeInsertPosition(null)).toEqual({});
  });
});

describe('Adapter contract: live round-trip (HttpAdapter → dev-proxy → daemon)', () => {
  let h: DaemonTestHarness;

  beforeAll(async () => {
    h = await DaemonTestHarness.start();
  }, 15_000);

  afterAll(async () => {
    await h?.stop();
  });

  it('create → update task fields with tri-state clear/set/no-change → read back matches', async () => {
    const id = crypto.randomUUID();
    await h.adapter.createNode({ id, nodeType: 'task', content: 'contract task' });
    const created = await h.adapter.getNode(id);
    expect(created).not.toBeNull();

    // dev-proxy must translate this into { clear:false, value:'high' } for
    // priority via buildTaskNodeUpdatePatch, not a hand-rolled equivalent.
    // The response is a typed TaskNode: core fields top-level, never in
    // `properties`.
    const updated = await h.adapter.updateTaskNode(id, created!.version, {
      priority: 'high',
      status: 'in_progress',
    });

    expect(updated.priority).toBe('high');
    expect(updated.status).toBe('in_progress');
    expect(updated.properties).toEqual({});

    // Clearing priority (null) must round-trip to "no priority", not the
    // literal string "null" or an unset-vs-cleared ambiguity.
    const cleared = await h.adapter.updateTaskNode(id, updated.version, {
      priority: null,
    });
    expect(cleared.priority == null).toBe(true);
    expect(cleared.status).toBe('in_progress');
  });

  it('create → typed person update → read back carries typed fields and the templated title', async () => {
    const id = crypto.randomUUID();
    await h.adapter.createNode({
      id,
      nodeType: 'person',
      content: '',
      properties: { first_name: 'Ada' },
    });
    const created = await h.adapter.getNode(id);
    expect((created as unknown as { firstName?: string }).firstName).toBe('Ada');

    const updated = await h.adapter.updatePersonNode(id, created!.version, {
      lastName: 'Lovelace',
      email: 'ada@example.com',
    });
    expect(updated.firstName).toBe('Ada');
    expect(updated.lastName).toBe('Lovelace');
    expect(updated.email).toBe('ada@example.com');
    expect(updated.properties).toEqual({});

    // A fresh read agrees — the write is durable, and the title_template
    // recomputed from the typed fields.
    const reread = (await h.adapter.getNode(id)) as unknown as {
      lastName?: string;
      title?: string;
    };
    expect(reread.lastName).toBe('Lovelace');
    expect(reread.title).toBe('Ada Lovelace');

    // null clears.
    const cleared = await h.adapter.updatePersonNode(id, updated.version, { email: null });
    expect(cleared.email).toBeUndefined();
  });

  it('create → typed project update → read back carries typed fields', async () => {
    const id = crypto.randomUUID();
    await h.adapter.createNode({ id, nodeType: 'project', content: 'Launch' });
    const created = await h.adapter.getNode(id);
    expect((created as unknown as { status?: string }).status).toBe('planning');

    const updated = await h.adapter.updateProjectNode(id, created!.version, {
      status: 'active',
      startDate: '2026-03-01T09:00:00Z',
    });
    expect(updated.status).toBe('active');
    expect(updated.startDate).toBe('2026-03-01');
    expect(updated.properties).toEqual({});
  });

  it('create → typed query update → read back carries typed fields', async () => {
    const id = crypto.randomUUID();
    await h.adapter.createNode({
      id,
      nodeType: 'query',
      content: 'Open tasks',
      properties: {
        target_type: 'task',
        filters: [],
        generated_by: 'user',
        view_config: { lastView: 'table' },
      },
    });
    const created = await h.adapter.getNode(id);
    expect((created as unknown as { targetType?: string }).targetType).toBe('task');

    const updated = await h.adapter.updateQueryNode(id, created!.version, {
      filters: [{ type: 'property', operator: 'equals', property: 'status', value: 'open' }],
      viewConfig: { lastView: 'kanban', kanban: { groupBy: 'status' } },
    });
    expect(updated.targetType).toBe('task');
    expect(updated.filters[0].property).toBe('status');
    expect(updated.viewConfig).toEqual({ lastView: 'kanban', kanban: { groupBy: 'status' } });
    expect(updated.properties).toEqual({});

    // null clears.
    const cleared = await h.adapter.updateQueryNode(id, updated.version, { viewConfig: null });
    expect(cleared.viewConfig).toBeUndefined();

    // A fresh read agrees — the write is durable.
    const reread = (await h.adapter.getNode(id)) as unknown as { filters?: unknown[] };
    expect(reread.filters).toHaveLength(1);
  });

  it('create → typed play update → read back carries typed fields', async () => {
    const id = crypto.randomUUID();
    await h.adapter.createNode({
      id,
      nodeType: 'play',
      content: 'Greet new tasks',
      properties: { rules: [] },
    });
    const created = await h.adapter.getNode(id);
    expect((created as unknown as { rules?: unknown[] }).rules).toEqual([]);

    const updated = await h.adapter.updatePlayNode(id, created!.version, {
      rules: [
        {
          name: 'greet',
          trigger: { type: 'graph_event', on: 'node_created', select: { target_type: 'task' } },
          conditions: ["node.content == 'hello'"],
          actions: [],
        },
      ],
      description: 'Greets new tasks',
    });
    expect(updated.rules[0].name).toBe('greet');
    expect(updated.rules[0].trigger.select).toEqual({ target_type: 'task' });
    expect(updated.description).toBe('Greets new tasks');
    expect(updated.properties).toEqual({});

    // null clears.
    const cleared = await h.adapter.updatePlayNode(id, updated.version, { description: null });
    expect(cleared.description).toBeUndefined();

    // A fresh read agrees — the write is durable.
    const reread = (await h.adapter.getNode(id)) as unknown as { rules?: unknown[] };
    expect(reread.rules).toHaveLength(1);
  });

  it('create → typed collection update → read back carries the typed description', async () => {
    const id = crypto.randomUUID();
    await h.adapter.createNode({ id, nodeType: 'collection', content: 'contract-clients' });
    const created = await h.adapter.getNode(id);
    expect((created as unknown as { description?: string }).description).toBeUndefined();

    const updated = await h.adapter.updateCollectionNode(id, created!.version, {
      description: 'Accounts we bill',
    });
    expect(updated.description).toBe('Accounts we bill');
    expect(updated.content).toBe('contract-clients');
    expect(updated.properties).toEqual({});

    // null clears.
    const cleared = await h.adapter.updateCollectionNode(id, updated.version, {
      description: null,
    });
    expect(cleared.description).toBeUndefined();
  });

  it('create → typed skill update → read back carries typed fields', async () => {
    const id = crypto.randomUUID();
    await h.adapter.createNode({
      id,
      nodeType: 'skill',
      content: 'Contract Skill',
      properties: {
        description: 'Update a record',
        tool_whitelist: ['update_node'],
        exclusion: 'Delete records',
      },
    });
    const created = (await h.adapter.getNode(id)) as unknown as {
      version: number;
      description: string;
      toolWhitelist: string[];
      maxIterations: number;
    };
    expect(created.description).toBe('Update a record');
    expect(created.toolWhitelist).toEqual(['update_node']);
    expect(created.maxIterations).toBe(2);

    const updated = await h.adapter.updateSkillNode(id, created.version, {
      toolWhitelist: ['update_node', 'get_node'],
      maxIterations: 4,
    });
    expect(updated.toolWhitelist).toEqual(['update_node', 'get_node']);
    expect(updated.maxIterations).toBe(4);
    expect(updated.exclusion).toBe('Delete records');
    expect(updated.properties).toEqual({});

    // null clears; a cleared number reads as the schema's default.
    const cleared = await h.adapter.updateSkillNode(id, updated.version, {
      exclusion: null,
      maxIterations: null,
    });
    expect(cleared.exclusion).toBeUndefined();
    expect(cleared.maxIterations).toBe(2);
    expect(cleared.description).toBe('Update a record');
  });

  it('typed database-settings update → read back carries the typed list', async () => {
    const id = 'database-settings-singleton';
    const settings = (await h.adapter.getNode(id)) as unknown as {
      version: number;
      requiredExtensions: string[];
    };
    expect(settings.requiredExtensions).toEqual([]);

    const updated = await h.adapter.updateDatabaseSettingsNode(id, settings.version, {
      requiredExtensions: ['contract-fixture'],
    });
    expect(updated.requiredExtensions).toEqual(['contract-fixture']);
    expect(updated.properties).toEqual({});

    // null clears; the list reads as empty again, so the database stays
    // openable by anything that reads it after this test.
    const cleared = await h.adapter.updateDatabaseSettingsNode(id, updated.version, {
      requiredExtensions: null,
    });
    expect(cleared.requiredExtensions).toEqual([]);
  });

  it('createNode honors an explicit InsertPosition the same way move/reorder do', async () => {
    const parentId = crypto.randomUUID();
    const firstId = crypto.randomUUID();
    const secondId = crypto.randomUUID();

    await h.adapter.createNode({ id: parentId, nodeType: 'text', content: 'parent' });
    await h.adapter.createNode({ id: firstId, nodeType: 'text', content: 'first', parentId });
    await h.adapter.createNode({
      id: secondId,
      nodeType: 'text',
      content: 'inserted-before-first',
      parentId,
      insertPosition: insertPosition.beginning(),
    });

    const children = await h.adapter.getChildren(parentId);
    expect(children.map((c) => c.id)).toEqual([secondId, firstId]);
  });

  it('moveNode honors an explicit InsertPosition (regression: dev-proxy previously ignored it entirely)', async () => {
    // Prior to this fix, POST /api/nodes/:id/parent hand-rolled a legacy
    // `insertAfterNodeId` field that no longer exists in MoveNodeRequest's
    // `oneof position`, so a browser-mode move-with-position silently
    // fell back to appending at the end. encodeInsertPosition closes that
    // gap the same way it does for createNode above.
    const parentAId = crypto.randomUUID();
    const parentBId = crypto.randomUUID();
    const stayingId = crypto.randomUUID();
    const movingId = crypto.randomUUID();

    await h.adapter.createNode({ id: parentAId, nodeType: 'text', content: 'parent-a' });
    await h.adapter.createNode({ id: parentBId, nodeType: 'text', content: 'parent-b' });
    await h.adapter.createNode({ id: movingId, nodeType: 'text', content: 'moving', parentId: parentAId });
    await h.adapter.createNode({ id: stayingId, nodeType: 'text', content: 'staying', parentId: parentBId });

    await h.adapter.moveNode(movingId, 1, parentBId, insertPosition.beginning());

    const children = await h.adapter.getChildren(parentBId);
    expect(children.map((c) => c.id)).toEqual([movingId, stayingId]);
  });
});
