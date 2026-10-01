/**
 * The sidebar reads `collectionsData.collectionsTree` inside a `$derived`. These
 * tests run that read inside a real derivation and check that it re-runs when the
 * reactive state an extension's `collectionTreeRoots` reads changes. The registry
 * holds no `$state` of its own, so this only works because it calls each
 * contributor on every lookup, inside the caller's derivation.
 */
import { describe, it, expect, afterEach } from 'vitest';
import { flushSync } from 'svelte';
import { collectionsData } from '$lib/stores/collections.svelte';
import { uiExtensionRegistry } from '$lib/plugins/ui-extensions';
import type { CollectionInfo } from '$lib/services/collection-service';
import {
  TEST_EXTENSION_ID,
  createTestExtension,
  resetTestExtension,
  testExtensionFlags
} from '../fixtures/test-extension';

function collection(id: string, name: string, parentCollectionIds: string[]): CollectionInfo {
  return {
    id,
    content: name,
    nodeType: 'collection',
    createdAt: '',
    modifiedAt: '',
    version: 1,
    properties: {},
    memberCount: 1,
    parentCollectionIds
  };
}

describe('collectionsTree inside a derivation', () => {
  afterEach(() => {
    uiExtensionRegistry.unregister(TEST_EXTENSION_ID);
    resetTestExtension();
  });

  it("re-runs when the state an extension's collectionTreeRoots reads changes", () => {
    collectionsData._setTestData(
      [
        collection('ext-root', 'Extension root', []),
        collection('engineering', 'Engineering', ['ext-root'])
      ],
      new Map()
    );
    uiExtensionRegistry.register(createTestExtension());

    const seen: string[][] = [];
    const stop = $effect.root(() => {
      const ids = $derived(collectionsData.collectionsTree.map((c) => c.id));
      $effect(() => {
        seen.push(ids);
      });
    });
    try {
      flushSync();
      expect(seen).toEqual([['ext-root']]);

      testExtensionFlags.collectionTreeRoots = ['ext-root'];
      flushSync();
      expect(seen).toEqual([['ext-root'], ['engineering']]);

      testExtensionFlags.collectionTreeRoots = [];
      flushSync();
      expect(seen).toEqual([['ext-root'], ['engineering'], ['ext-root']]);
    } finally {
      stop();
    }
  });
});
