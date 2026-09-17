/**
 * Unit tests for the schema-authoring service — the "+ New" create-instance
 * logic behind QueryNodeViewer's header button.
 */

import { describe, it, expect, vi, beforeEach } from 'vitest';
import type { Node } from '$lib/types';

vi.mock('$lib/services/backend-adapter', () => ({
  backendAdapter: { createNode: vi.fn(), getNode: vi.fn() }
}));

import { backendAdapter } from '$lib/services/backend-adapter';
import { createSchemaInstance, shouldIntegrateInstance } from '$lib/services/schema-authoring';
import { UNTITLED_CHAT_TITLE } from '$lib/utils/ai-chat-title';

const createNodeMock = vi.mocked(backendAdapter.createNode);
const getNodeMock = vi.mocked(backendAdapter.getNode);

function makeNode(id: string, nodeType: string): Node {
  return {
    id,
    nodeType,
    content: '',
    createdAt: '2026-01-01T00:00:00.000Z',
    modifiedAt: '2026-01-01T00:00:00.000Z',
    version: 1,
    properties: {}
  };
}

beforeEach(() => {
  vi.clearAllMocks();
  // createNode returns the id (string); default getNode echoes a node for it.
  createNodeMock.mockImplementation(async (input) => (input as Node).id);
  getNodeMock.mockImplementation(async (id: string) => makeNode(id, 'invoice'));
});

describe('createSchemaInstance', () => {
  it('mints an empty root node of the given schema type', async () => {
    await createSchemaInstance('invoice');

    expect(createNodeMock).toHaveBeenCalledTimes(1);
    const createArg = createNodeMock.mock.calls[0][0];
    expect(createArg).toEqual(
      expect.objectContaining({ nodeType: 'invoice', content: '', parentId: null, properties: {} })
    );
    // A concrete id is generated so the caller can open the node immediately.
    expect(typeof createArg.id).toBe('string');
    expect(createArg.id.length).toBeGreaterThan(0);
  });

  it('seeds "Untitled {Type}" content for name-as-content Core types', async () => {
    // These types' behaviors reject empty content, so the sidebar "+New" default
    // of '' always failed validation; they must get a type-identifying placeholder.
    const cases: Array<[string, string]> = [
      ['project', 'Untitled Project'],
      ['skill', 'Untitled Skill'],
      ['collection', 'Untitled Collection'],
      ['agent-guidance', 'Untitled Agent Guidance'],
      ['tool', 'Untitled Tool'],
    ];
    for (const [typeId, expected] of cases) {
      createNodeMock.mockClear();
      await createSchemaInstance(typeId);
      expect(createNodeMock.mock.calls[0][0]).toEqual(
        expect.objectContaining({ nodeType: typeId, content: expected })
      );
    }
  });

  it('seeds ai-chat with the exact sentinel the daemon tests for', async () => {
    // Not "Untitled Ai Chat": the daemon's background titler only claims a
    // chat whose content is byte-identical to UNTITLED_CHAT_TITLE, so the
    // humanized "Untitled {Type}" form would read as a user-chosen title and
    // suppress automatic titling forever.
    await createSchemaInstance('ai-chat');

    expect(createNodeMock.mock.calls[0][0]).toEqual(
      expect.objectContaining({ nodeType: 'ai-chat', content: UNTITLED_CHAT_TITLE })
    );
    expect(createNodeMock.mock.calls[0][0].content).toBe('Untitled');
  });

  it('leaves body-content and custom types with empty content', async () => {
    // Primitives ("start typing" state) and titleTemplate/custom types must be
    // unaffected — empty content is correct for them.
    for (const typeId of ['text', 'header', 'task', 'code-block', 'invoice']) {
      createNodeMock.mockClear();
      await createSchemaInstance(typeId);
      expect(createNodeMock.mock.calls[0][0]).toEqual(
        expect.objectContaining({ nodeType: typeId, content: '' })
      );
    }
  });

  it('returns the hydrated node fetched by the generated id', async () => {
    const returned = await createSchemaInstance('invoice');

    const generatedId = createNodeMock.mock.calls[0][0].id;
    expect(getNodeMock).toHaveBeenCalledWith(generatedId);
    expect(returned.id).toBe(generatedId);
    expect(returned.nodeType).toBe('invoice');
  });

  it('creates the node before fetching it back', async () => {
    await createSchemaInstance('invoice');

    expect(createNodeMock.mock.invocationCallOrder[0]).toBeLessThan(
      getNodeMock.mock.invocationCallOrder[0]
    );
  });

  it('mints a distinct id per call', async () => {
    await createSchemaInstance('invoice');
    await createSchemaInstance('invoice');
    expect(createNodeMock.mock.calls[0][0].id).not.toBe(createNodeMock.mock.calls[1][0].id);
  });

  it('does not swallow a backend failure', async () => {
    createNodeMock.mockRejectedValueOnce(new Error('boom'));
    await expect(createSchemaInstance('invoice')).rejects.toThrow('boom');
    expect(getNodeMock).not.toHaveBeenCalled();
  });

  it('throws if the newly created node cannot be loaded back', async () => {
    getNodeMock.mockResolvedValueOnce(null);
    await expect(createSchemaInstance('invoice')).rejects.toThrow(/could not be loaded/);
  });
});

describe('shouldIntegrateInstance', () => {
  it('integrates when both the load generation and epoch are unchanged', () => {
    expect(shouldIntegrateInstance({ loadId: 3, epoch: 7 }, { loadId: 3, epoch: 7 })).toBe(true);
  });

  it('discards when the load generation changed mid-create (re-query)', () => {
    expect(shouldIntegrateInstance({ loadId: 3, epoch: 7 }, { loadId: 4, epoch: 7 })).toBe(false);
  });

  it('discards when the database epoch changed mid-create (db switch)', () => {
    expect(shouldIntegrateInstance({ loadId: 3, epoch: 7 }, { loadId: 3, epoch: 8 })).toBe(false);
  });

  it('discards when both changed', () => {
    expect(shouldIntegrateInstance({ loadId: 3, epoch: 7 }, { loadId: 4, epoch: 8 })).toBe(false);
  });
});
