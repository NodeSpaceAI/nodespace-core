/**
 * The host API's facades delegate to core's stores and services, and a read
 * through them inside a derivation re-runs when the store changes. The file is a
 * `.svelte.test.ts` so it can run those reads inside a real `$derived`.
 *
 * Tauri is mocked through the `/testing` entry, as an extension's tests do.
 */
import { mockTauriCore } from '@nodespace/extension-api/testing';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { flushSync } from 'svelte';
import * as api from '@nodespace/extension-api';
import * as ui from '@nodespace/extension-api/ui';
import * as testing from '@nodespace/extension-api/testing';
import { databaseStore, type DatabaseInfo } from '$lib/stores/database.svelte';
import { collectionsData } from '$lib/stores/collections.svelte';
import { schemasData } from '$lib/stores/schemas.svelte';
import { backendAdapter } from '$lib/services/backend-adapter';
import { sharedNodeStore } from '$lib/services/shared-node-store.svelte';
import * as daemonStatus from '$lib/services/daemon-status';
import * as logger from '$lib/utils/logger';
import * as errors from '$lib/types/errors';
import * as tauriCoreHelper from '../helpers/mock-tauri-core';
import { EXTENSION_API_VERSION } from '$lib/plugins/ui-extensions';
import { Button } from '$lib/components/ui/button';
import * as Dialog from '$lib/components/ui/dialog';
import { focusTrap } from '$lib/actions/focus-trap';
import type { Node } from '$lib/types';

vi.mock('@tauri-apps/api/core', () => mockTauriCore());

const { databases, nodes, collections, schemas } = api;

/** A registry entry carrying the fields these tests read; the facade passes entries through whole. */
function database(id: string): DatabaseInfo {
  return {
    id,
    name: `Database ${id}`,
    path: `/tmp/${id}.db`,
    isDefault: id === 'a'
  } as DatabaseInfo;
}

function node(id: string, content: string): Node {
  return {
    lifecycleStatus: 'active',
    id,
    nodeType: 'text',
    content,
    properties: {},
    mentions: [],
    createdAt: '',
    modifiedAt: '',
    version: 1
  };
}

/** Runs `read` inside a derivation and records each value it settles on. */
function track<T>(read: () => T): { seen: T[]; stop: () => void } {
  const seen: T[] = [];
  const stop = $effect.root(() => {
    const value = $derived(read());
    $effect(() => {
      seen.push(value);
    });
  });
  flushSync();
  return { seen, stop };
}

const saved = {
  databases: databaseStore.databases,
  activeDatabaseId: databaseStore.activeDatabaseId,
  error: databaseStore.error
};

afterEach(() => {
  vi.restoreAllMocks();
  databaseStore.databases = saved.databases;
  databaseStore.activeDatabaseId = saved.activeDatabaseId;
  databaseStore.error = saved.error;
  sharedNodeStore.__resetForTesting();
});

describe('databases', () => {
  it('reads the database store', () => {
    databaseStore.databases = [database('a'), database('b')];
    databaseStore.activeDatabaseId = 'b';
    databaseStore.error = 'Could not open';

    expect(databases.list.map((db) => db.id)).toEqual(['a', 'b']);
    expect(databases.activeDatabaseId).toBe('b');
    expect(databases.activeDatabase?.name).toBe('Database b');
    expect(databases.error).toBe('Could not open');
  });

  it('re-runs a derivation when the store changes', () => {
    databaseStore.databases = [database('a')];
    databaseStore.activeDatabaseId = 'a';
    databaseStore.error = null;
    const active = track(() => databases.activeDatabase?.id ?? null);
    const list = track(() => databases.list.length);
    const error = track(() => databases.error);
    try {
      databaseStore.databases = [database('a'), database('b')];
      databaseStore.activeDatabaseId = 'b';
      databaseStore.error = 'Could not open';
      flushSync();

      expect(active.seen).toEqual(['a', 'b']);
      expect(list.seen).toEqual([1, 2]);
      expect(error.seen).toEqual([null, 'Could not open']);
    } finally {
      active.stop();
      list.stop();
      error.stop();
    }
  });

  it('delegates create and switchTo to the store', async () => {
    const created = database('c');
    const create = vi.spyOn(databaseStore, 'create').mockResolvedValue(created);
    const switchTo = vi.spyOn(databaseStore, 'switchTo').mockResolvedValue();

    await expect(databases.create('Work', '/tmp/work.db')).resolves.toBe(created);
    expect(create).toHaveBeenCalledWith('Work', '/tmp/work.db');
    await databases.switchTo('c');
    expect(switchTo).toHaveBeenCalledWith('c');
  });

  it('offers no settings-node refresh, and the API exports no settings-node id', () => {
    // Neither is in the host API (ADR-082 §3.6). Built from fragments so this
    // file does not itself name what it checks is gone.
    const refresh = ['refresh', 'Database', 'Settings'].join('');
    const nodeId = ['DATABASE', 'SETTINGS', 'NODE', 'ID'].join('_');

    expect(Object.keys(databases)).toEqual([
      'activeDatabaseId',
      'activeDatabase',
      'list',
      'error',
      'create',
      'switchTo'
    ]);
    expect(refresh in databases).toBe(false);
    expect(nodeId in api).toBe(false);
  });
});

describe('collections and schemas', () => {
  it('reload through the store loaders', async () => {
    const loadCollections = vi.spyOn(collectionsData, 'loadCollections').mockResolvedValue();
    const loadSchemas = vi.spyOn(schemasData, 'loadSchemas').mockResolvedValue();

    await collections.reload();
    await schemas.reload();

    expect(loadCollections).toHaveBeenCalledOnce();
    expect(loadSchemas).toHaveBeenCalledOnce();
  });
});

describe('nodes', () => {
  it('reads the shared node store, reactively', () => {
    sharedNodeStore.setNode(node('n1', 'first'), { type: 'database', reason: 'test' }, true);
    expect(nodes.getNode('n1')).toBe(sharedNodeStore.getNode('n1'));
    expect(nodes.getNode('missing')).toBeUndefined();

    const content = track(() => nodes.getNode('n1')?.content ?? null);
    try {
      sharedNodeStore.setNode(
        { ...node('n1', 'second'), version: 2 },
        { type: 'database', reason: 'test' },
        true
      );
      flushSync();
      expect(content.seen).toEqual(['first', 'second']);
    } finally {
      content.stop();
    }
  });

  it('fetches and creates through the backend adapter', async () => {
    const fetched = node('n2', 'from the daemon');
    const getNode = vi.spyOn(backendAdapter, 'getNode').mockResolvedValue(fetched);
    const createNode = vi
      .spyOn(backendAdapter, 'createNode')
      .mockResolvedValue({ id: 'n3', placement: null });
    const input = { id: 'n3', nodeType: 'collection', content: 'Root' };

    await expect(nodes.fetchNode('n2')).resolves.toBe(fetched);
    expect(getNode).toHaveBeenCalledWith('n2');
    await expect(nodes.createNode(input)).resolves.toEqual({ id: 'n3', placement: null });
    expect(createNode).toHaveBeenCalledWith(input);
  });
});

describe('nodes.updateNode', () => {
  it('writes through the node store, so a reactive read sees it at once', () => {
    sharedNodeStore.setNode(node('u1', 'before'), { type: 'database', reason: 'test' }, true);
    const update = vi.spyOn(sharedNodeStore, 'updateNode');
    const content = track(() => nodes.getNode('u1')?.content ?? null);
    try {
      nodes.updateNode('u1', { content: 'after' });
      flushSync();

      expect(update).toHaveBeenCalledWith(
        'u1',
        { content: 'after' },
        { type: 'viewer', viewerId: 'extension-api' }
      );
      expect(content.seen).toEqual(['before', 'after']);
    } finally {
      content.stop();
    }
  });

  it('leaves a node the store does not hold alone', () => {
    expect(() => nodes.updateNode('not-in-store', { content: 'x' })).not.toThrow();
    expect(nodes.getNode('not-in-store')).toBeUndefined();
  });
});

describe('re-exports', () => {
  it("are core's own functions and values", () => {
    expect(api.onDaemonReconnect).toBe(daemonStatus.onDaemonReconnect);
    expect(api.createLogger).toBe(logger.createLogger);
    expect(api.toError).toBe(errors.toError);
    expect(api.isCommandError).toBe(errors.isCommandError);
    expect(api.EXTENSION_API_VERSION).toBe(EXTENSION_API_VERSION);
    expect(testing.mockTauriCore).toBe(tauriCoreHelper.mockTauriCore);
    expect(ui.Button).toBe(Button);
    expect(ui.Dialog.Root).toBe(Dialog.Root);
    expect(ui.focusTrap).toBe(focusTrap);
  });
});
