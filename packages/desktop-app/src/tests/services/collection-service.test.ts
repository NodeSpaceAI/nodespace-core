import { describe, it, expect, vi } from 'vitest';

vi.mock('$lib/utils/logger', () => ({
  createLogger: () => ({
    debug: vi.fn(),
    info: vi.fn(),
    warn: vi.fn(),
    error: vi.fn()
  })
}));

vi.mock('@tauri-apps/api/core', () => ({
  invoke: vi.fn()
}));

import { collectionService } from '$lib/services/collection-service';

describe('Collection Service', () => {
  describe('environment detection', () => {
    it('should use MockCollectionService in test environment', () => {
      // In test env, collectionService should be the mock implementation
      expect(collectionService).toBeDefined();
    });
  });

  describe('MockCollectionService methods', () => {
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
