import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';

vi.mock('$lib/utils/logger', () => ({
  createLogger: () => ({
    debug: vi.fn(),
    info: vi.fn(),
    warn: vi.fn(),
    error: vi.fn()
  })
}));

import { mockTauriCore } from '../helpers/mock-tauri-core';

vi.mock('@tauri-apps/api/core', () => mockTauriCore());

import { collectionService } from '$lib/services/collection-service';

describe('Collection Service', () => {
  describe('MockCollectionService methods', () => {
    // The module-level `collectionService` singleton resolves to MockCollectionService
    // under NODE_ENV=test (see the `getCollectionService dispatcher` tests below for
    // coverage of the other two branches), so exercising its methods here doubles as
    // confirmation that the dispatcher picked the mock implementation.
    it('getAllCollections should return empty array', async () => {
      const result = await collectionService.getAllCollections();
      expect(result).toEqual([]);
    });

    it('getCollectionMembers should return empty array', async () => {
      const result = await collectionService.getCollectionMembers('any-id');
      expect(result).toEqual([]);
    });

    it('getCollectionMembersRecursive should return empty array', async () => {
      const result = await collectionService.getCollectionMembersRecursive('any-id');
      expect(result).toEqual([]);
    });

    it('getNodeCollections should return empty array', async () => {
      const result = await collectionService.getNodeCollections('any-id');
      expect(result).toEqual([]);
    });

    it('findCollectionByPath should return null', async () => {
      const result = await collectionService.findCollectionByPath('some:path');
      expect(result).toBeNull();
    });

    it('getCollectionByName should return null', async () => {
      const result = await collectionService.getCollectionByName('test');
      expect(result).toBeNull();
    });

    it('addNodeToCollection should not throw', async () => {
      await expect(collectionService.addNodeToCollection('n1', 'c1')).resolves.toBeUndefined();
    });

    it('addNodeToCollectionPath should return mock id', async () => {
      const result = await collectionService.addNodeToCollectionPath('n1', 'path');
      expect(result).toBe('mock-collection-id');
    });

    it('removeNodeFromCollection should not throw', async () => {
      await expect(collectionService.removeNodeFromCollection('n1', 'c1')).resolves.toBeUndefined();
    });

    it('createCollection should return mock id', async () => {
      const result = await collectionService.createCollection('Test Collection');
      expect(result).toBe('mock-collection-id');
    });

    it('renameCollection should return updated collection node', async () => {
      const result = await collectionService.renameCollection('c1', 1, 'New Name');
      expect(result.id).toBe('c1');
      expect(result.content).toBe('New Name');
      expect(result.nodeType).toBe('collection');
    });

    it('deleteCollection should not throw', async () => {
      await expect(collectionService.deleteCollection('c1', 1)).resolves.toBeUndefined();
    });
  });
});

describe('getCollectionService dispatcher', () => {
  const ORIGINAL_NODE_ENV = process.env.NODE_ENV;

  beforeEach(() => {
    vi.resetModules();
  });

  afterEach(() => {
    process.env.NODE_ENV = ORIGINAL_NODE_ENV;
    // Was unstubbed at the end of the test body, which a failing assertion skips —
    // leaving a stubbed fetch for the rest of the fork. The sibling describe already
    // does it here.
    vi.unstubAllGlobals();
    Reflect.deleteProperty(window, '__TAURI_INTERNALS__');
    Reflect.deleteProperty(window, '__TAURI__');
  });

  it('returns a MockCollectionService when isTestEnvironment() is true', async () => {
    process.env.NODE_ENV = 'test';

    const { collectionService: svc } = await import('$lib/services/collection-service');
    const result = await svc.getAllCollections();

    expect(result).toEqual([]);
  });

  it('returns a TauriCollectionService when not test-env and Tauri is detected', async () => {
    process.env.NODE_ENV = 'development';
    (window as unknown as Record<string, unknown>).__TAURI_INTERNALS__ = {};

    const { collectionService: svc } = await import('$lib/services/collection-service');
    const invokeMock = vi.mocked((await import('@tauri-apps/api/core')).invoke);
    invokeMock.mockResolvedValueOnce([]);

    await svc.getAllCollections();

    expect(invokeMock).toHaveBeenCalledWith('get_all_collections');
  });

  it('returns an HttpCollectionService when not test-env and Tauri is not detected', async () => {
    process.env.NODE_ENV = 'development';

    vi.stubGlobal(
      'fetch',
      vi.fn().mockResolvedValue({
        ok: true,
        status: 200,
        json: () => Promise.resolve([])
      })
    );

    const { collectionService: svc } = await import('$lib/services/collection-service');
    await svc.getAllCollections();

    expect(fetch).toHaveBeenCalledWith('http://localhost:3001/api/collections');
  });
});

describe('TauriCollectionService', () => {
  const ORIGINAL_NODE_ENV = process.env.NODE_ENV;

  beforeEach(() => {
    vi.resetModules();
  });

  afterEach(() => {
    process.env.NODE_ENV = ORIGINAL_NODE_ENV;
    Reflect.deleteProperty(window, '__TAURI_INTERNALS__');
  });

  async function loadTauriService() {
    process.env.NODE_ENV = 'development';
    (window as unknown as Record<string, unknown>).__TAURI_INTERNALS__ = {};
    const { collectionService: svc } = await import('$lib/services/collection-service');
    const invokeMock = vi.mocked((await import('@tauri-apps/api/core')).invoke);
    invokeMock.mockReset();
    return { svc, invokeMock };
  }

  it('getAllCollections invokes get_all_collections', async () => {
    const { svc, invokeMock } = await loadTauriService();
    invokeMock.mockResolvedValueOnce([]);

    await svc.getAllCollections();

    expect(invokeMock).toHaveBeenCalledWith('get_all_collections');
  });

  it('getCollectionMembers invokes get_collection_members with collectionId', async () => {
    const { svc, invokeMock } = await loadTauriService();
    invokeMock.mockResolvedValueOnce([]);

    await svc.getCollectionMembers('c1');

    expect(invokeMock).toHaveBeenCalledWith('get_collection_members', { collectionId: 'c1' });
  });

  it('getCollectionMembersRecursive invokes get_collection_members_recursive with collectionId', async () => {
    const { svc, invokeMock } = await loadTauriService();
    invokeMock.mockResolvedValueOnce([]);

    await svc.getCollectionMembersRecursive('c1');

    expect(invokeMock).toHaveBeenCalledWith('get_collection_members_recursive', { collectionId: 'c1' });
  });

  it('getNodeCollections invokes get_node_collections with nodeId', async () => {
    const { svc, invokeMock } = await loadTauriService();
    invokeMock.mockResolvedValueOnce([]);

    await svc.getNodeCollections('n1');

    expect(invokeMock).toHaveBeenCalledWith('get_node_collections', { nodeId: 'n1' });
  });

  it('findCollectionByPath invokes find_collection_by_path with collectionPath', async () => {
    const { svc, invokeMock } = await loadTauriService();
    invokeMock.mockResolvedValueOnce(null);

    await svc.findCollectionByPath('hr:policy');

    expect(invokeMock).toHaveBeenCalledWith('find_collection_by_path', { collectionPath: 'hr:policy' });
  });

  it('getCollectionByName invokes get_collection_by_name with name', async () => {
    const { svc, invokeMock } = await loadTauriService();
    invokeMock.mockResolvedValueOnce(null);

    await svc.getCollectionByName('Test');

    expect(invokeMock).toHaveBeenCalledWith('get_collection_by_name', { name: 'Test' });
  });

  it('addNodeToCollection invokes add_node_to_collection with nodeId and collectionId', async () => {
    const { svc, invokeMock } = await loadTauriService();
    invokeMock.mockResolvedValueOnce(undefined);

    await svc.addNodeToCollection('n1', 'c1');

    expect(invokeMock).toHaveBeenCalledWith('add_node_to_collection', { nodeId: 'n1', collectionId: 'c1' });
  });

  it('addNodeToCollectionPath invokes add_node_to_collection_path with nodeId and collectionPath', async () => {
    const { svc, invokeMock } = await loadTauriService();
    invokeMock.mockResolvedValueOnce('new-id');

    const result = await svc.addNodeToCollectionPath('n1', 'hr:policy');

    expect(invokeMock).toHaveBeenCalledWith('add_node_to_collection_path', {
      nodeId: 'n1',
      collectionPath: 'hr:policy'
    });
    expect(result).toBe('new-id');
  });

  it('removeNodeFromCollection invokes remove_node_from_collection with nodeId and collectionId', async () => {
    const { svc, invokeMock } = await loadTauriService();
    invokeMock.mockResolvedValueOnce(undefined);

    await svc.removeNodeFromCollection('n1', 'c1');

    expect(invokeMock).toHaveBeenCalledWith('remove_node_from_collection', { nodeId: 'n1', collectionId: 'c1' });
  });

  it('createCollection invokes create_collection with name and description', async () => {
    const { svc, invokeMock } = await loadTauriService();
    invokeMock.mockResolvedValueOnce('new-id');

    const result = await svc.createCollection('Test Collection', 'a description');

    expect(invokeMock).toHaveBeenCalledWith('create_collection', {
      name: 'Test Collection',
      description: 'a description'
    });
    expect(result).toBe('new-id');
  });

  it('renameCollection invokes rename_collection with collectionId, version, newName', async () => {
    const { svc, invokeMock } = await loadTauriService();
    const collectionNode = {
      id: 'c1',
      nodeType: 'collection',
      content: 'New Name',
      createdAt: new Date().toISOString(),
      modifiedAt: new Date().toISOString(),
      version: 2,
      properties: {}
    };
    invokeMock.mockResolvedValueOnce(collectionNode);

    const result = await svc.renameCollection('c1', 1, 'New Name');

    expect(invokeMock).toHaveBeenCalledWith('rename_collection', {
      collectionId: 'c1',
      version: 1,
      newName: 'New Name'
    });
    expect(result).toEqual(collectionNode);
  });

  it('deleteCollection invokes delete_collection with collectionId and version', async () => {
    const { svc, invokeMock } = await loadTauriService();
    invokeMock.mockResolvedValueOnce(undefined);

    await svc.deleteCollection('c1', 1);

    expect(invokeMock).toHaveBeenCalledWith('delete_collection', { collectionId: 'c1', version: 1 });
  });
});

describe('HttpCollectionService', () => {
  const ORIGINAL_NODE_ENV = process.env.NODE_ENV;

  beforeEach(() => {
    vi.resetModules();
    process.env.NODE_ENV = 'development';
    Reflect.deleteProperty(window, '__TAURI_INTERNALS__');
    Reflect.deleteProperty(window, '__TAURI__');
  });

  afterEach(() => {
    process.env.NODE_ENV = ORIGINAL_NODE_ENV;
    vi.unstubAllGlobals();
  });

  async function loadHttpService() {
    const { collectionService: svc } = await import('$lib/services/collection-service');
    return svc;
  }

  it('getAllCollections fetches from the dev-proxy and returns parsed JSON', async () => {
    const collections = [{ id: 'c1' }];
    vi.stubGlobal(
      'fetch',
      vi.fn().mockResolvedValue({
        ok: true,
        status: 200,
        statusText: 'OK',
        json: () => Promise.resolve(collections)
      })
    );

    const svc = await loadHttpService();
    const result = await svc.getAllCollections();

    expect(fetch).toHaveBeenCalledWith('http://localhost:3001/api/collections');
    expect(result).toEqual(collections);
  });

  it('getAllCollections throws a descriptive error on a non-ok response', async () => {
    vi.stubGlobal(
      'fetch',
      vi.fn().mockResolvedValue({
        ok: false,
        status: 500,
        statusText: 'Internal Server Error',
        json: () => Promise.resolve(null)
      })
    );

    const svc = await loadHttpService();

    await expect(svc.getAllCollections()).rejects.toThrow(
      'Failed to fetch collections: Internal Server Error'
    );
  });

  it('getCollectionMembers fetches from the dev-proxy with the encoded collectionId', async () => {
    const members = [{ id: 'n1', name: 'Node 1', nodeType: 'text' }];
    vi.stubGlobal(
      'fetch',
      vi.fn().mockResolvedValue({
        ok: true,
        status: 200,
        statusText: 'OK',
        json: () => Promise.resolve(members)
      })
    );

    const svc = await loadHttpService();
    const result = await svc.getCollectionMembers('c 1');

    expect(fetch).toHaveBeenCalledWith('http://localhost:3001/api/collections/c%201/members');
    expect(result).toEqual(members);
  });

  it('getCollectionMembers throws a descriptive error on a non-ok response', async () => {
    vi.stubGlobal(
      'fetch',
      vi.fn().mockResolvedValue({
        ok: false,
        status: 404,
        statusText: 'Not Found',
        json: () => Promise.resolve(null)
      })
    );

    const svc = await loadHttpService();

    await expect(svc.getCollectionMembers('missing')).rejects.toThrow(
      'Failed to fetch collection members: Not Found'
    );
  });

  it('getCollectionMembersRecursive delegates to getCollectionMembers', async () => {
    const members = [{ id: 'n1', name: 'Node 1', nodeType: 'text' }];
    vi.stubGlobal(
      'fetch',
      vi.fn().mockResolvedValue({
        ok: true,
        status: 200,
        statusText: 'OK',
        json: () => Promise.resolve(members)
      })
    );

    const svc = await loadHttpService();
    const result = await svc.getCollectionMembersRecursive('c1');

    expect(fetch).toHaveBeenCalledWith('http://localhost:3001/api/collections/c1/members');
    expect(result).toEqual(members);
  });

  it('addNodeToCollection sends the collection id on a node update', async () => {
    vi.stubGlobal('fetch', vi.fn().mockResolvedValue({ ok: true, status: 200, statusText: 'OK' }));

    const svc = await loadHttpService();
    await svc.addNodeToCollection('n 1', 'c1');

    expect(fetch).toHaveBeenCalledWith('http://localhost:3001/api/nodes/n%201', {
      method: 'PATCH',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ addToCollectionIds: ['c1'] })
    });
  });

  it('removeNodeFromCollection sends the collection id on a node update', async () => {
    vi.stubGlobal('fetch', vi.fn().mockResolvedValue({ ok: true, status: 200, statusText: 'OK' }));

    const svc = await loadHttpService();
    await svc.removeNodeFromCollection('n1', 'c1');

    expect(fetch).toHaveBeenCalledWith('http://localhost:3001/api/nodes/n1', {
      method: 'PATCH',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ removeFromCollectionIds: ['c1'] })
    });
  });

  it('a membership change throws a descriptive error on a non-ok response', async () => {
    vi.stubGlobal(
      'fetch',
      vi.fn().mockResolvedValue({ ok: false, status: 404, statusText: 'Not Found' })
    );

    const svc = await loadHttpService();

    await expect(svc.addNodeToCollection('n1', 'c1')).rejects.toThrow(
      'Failed to add node to collection: Not Found'
    );
    await expect(svc.removeNodeFromCollection('n1', 'c1')).rejects.toThrow(
      'Failed to remove node from collection: Not Found'
    );
  });

  it('addNodeToCollectionPath posts the path and returns the leaf collection id', async () => {
    vi.stubGlobal(
      'fetch',
      vi.fn().mockResolvedValue({
        ok: true,
        status: 200,
        statusText: 'OK',
        json: () => Promise.resolve('leaf-id')
      })
    );

    const svc = await loadHttpService();
    const result = await svc.addNodeToCollectionPath('n 1', 'hr:policy');

    expect(fetch).toHaveBeenCalledWith('http://localhost:3001/api/nodes/n%201/collections', {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ collectionPath: 'hr:policy' })
    });
    expect(result).toBe('leaf-id');
  });

  it('addNodeToCollectionPath throws a descriptive error on a non-ok response', async () => {
    vi.stubGlobal(
      'fetch',
      vi.fn().mockResolvedValue({ ok: false, status: 404, statusText: 'Not Found' })
    );

    const svc = await loadHttpService();

    await expect(svc.addNodeToCollectionPath('missing', 'hr:policy')).rejects.toThrow(
      'Failed to add node to collection path: Not Found'
    );
  });

  it('stub methods return their documented placeholder values without throwing', async () => {
    const svc = await loadHttpService();

    await expect(svc.getNodeCollections('n1')).resolves.toEqual([]);
    await expect(svc.findCollectionByPath('some:path')).resolves.toBeNull();
    await expect(svc.getCollectionByName('Test')).resolves.toBeNull();
    await expect(svc.createCollection('Test')).resolves.toBe('');
    await expect(svc.deleteCollection('c1', 1)).resolves.toBeUndefined();

    const renamed = await svc.renameCollection('c1', 1, 'New Name');
    expect(renamed.id).toBe('c1');
    expect(renamed.content).toBe('New Name');
    expect(renamed.nodeType).toBe('collection');
  });
});
