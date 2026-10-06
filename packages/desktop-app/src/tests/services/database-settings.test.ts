/**
 * The database's settings (ADR-095): one node, read and written through the
 * typed update.
 */

import { describe, it, expect, vi, beforeEach } from 'vitest';

vi.mock('$lib/services/backend-adapter', () => ({
  backendAdapter: { getNode: vi.fn(), updateDatabaseSettingsNode: vi.fn() }
}));

import { backendAdapter } from '$lib/services/backend-adapter';
import {
  DATABASE_SETTINGS_NODE_ID,
  readDatabaseSettings,
  updateDatabaseSettings
} from '$lib/services/database-settings';

const getNode = vi.mocked(backendAdapter.getNode);
const updateNode = vi.mocked(backendAdapter.updateDatabaseSettingsNode);

function settingsNode(version: number) {
  return {
    id: DATABASE_SETTINGS_NODE_ID,
    nodeType: 'database-settings',
    version,
    captureEnabled: false,
    captureContent: 'metadata_only',
    providers: [],
    requiredExtensions: []
  } as never;
}

const versionConflict = {
  code: 'VERSION_CONFLICT',
  conflictData: { node_id: DATABASE_SETTINGS_NODE_ID }
};

beforeEach(() => {
  vi.clearAllMocks();
});

describe('readDatabaseSettings', () => {
  it('reads the settings singleton', async () => {
    getNode.mockResolvedValue(settingsNode(3));

    const node = await readDatabaseSettings();

    expect(getNode).toHaveBeenCalledWith('database-settings-singleton');
    expect(node.captureContent).toBe('metadata_only');
  });

  it('refuses a node that is not the settings node', async () => {
    getNode.mockResolvedValue({ id: 'x', nodeType: 'text' } as never);

    await expect(readDatabaseSettings()).rejects.toThrow('settings node is missing');
  });

  it('refuses a database with no settings node', async () => {
    getNode.mockResolvedValue(null);

    await expect(readDatabaseSettings()).rejects.toThrow('settings node is missing');
  });
});

describe('updateDatabaseSettings', () => {
  it('writes the update at the version it read', async () => {
    getNode.mockResolvedValue(settingsNode(3));
    updateNode.mockResolvedValue(settingsNode(4));

    await updateDatabaseSettings({ captureEnabled: true });

    expect(updateNode).toHaveBeenCalledWith('database-settings-singleton', 3, {
      captureEnabled: true
    });
  });

  it('re-reads the winning version and writes once more after a lost race', async () => {
    getNode.mockResolvedValueOnce(settingsNode(3)).mockResolvedValueOnce(settingsNode(4));
    updateNode.mockRejectedValueOnce(versionConflict).mockResolvedValueOnce(settingsNode(5));

    await updateDatabaseSettings({ captureEnabled: true });

    expect(updateNode).toHaveBeenNthCalledWith(2, 'database-settings-singleton', 4, {
      captureEnabled: true
    });
  });

  it('gives up after a second lost race', async () => {
    getNode.mockResolvedValue(settingsNode(3));
    updateNode.mockRejectedValue(versionConflict);

    await expect(updateDatabaseSettings({ captureEnabled: true })).rejects.toBe(versionConflict);
    expect(updateNode).toHaveBeenCalledTimes(2);
  });

  it('does not retry any other failure', async () => {
    getNode.mockResolvedValue(settingsNode(3));
    const rejected = new Error('providers[0]: id is not a UUID');
    updateNode.mockRejectedValue(rejected);

    await expect(updateDatabaseSettings({ providers: [] })).rejects.toBe(rejected);
    expect(updateNode).toHaveBeenCalledTimes(1);
  });
});
