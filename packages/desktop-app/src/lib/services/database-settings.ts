/**
 * Reads and writes the database's settings (ADR-095).
 *
 * Every setting the daemon acts on is a field of the `database-settings` node
 * of the database a request is routed to: session capture, whether external
 * tools may be served, and the OpenAI-compatible providers. There is no other
 * store. A write goes through the node's typed update, validated against the
 * closed schema like any other write.
 */

import { backendAdapter } from '$lib/services/backend-adapter';
import { isVersionConflict } from '$lib/types/errors';
import type { DatabaseSettingsNode, DatabaseSettingsNodeUpdate } from '$lib/types';
import { isExactly } from '$lib/types/core-node-types';

/** The fixed id of the settings singleton every database seeds. */
export const DATABASE_SETTINGS_NODE_ID = 'database-settings-singleton';

function isDatabaseSettingsNode(node: unknown): node is DatabaseSettingsNode {
  return (
    typeof node === 'object' &&
    node !== null &&
    'nodeType' in node &&
    typeof node.nodeType === 'string' &&
    isExactly(node.nodeType, 'database-settings')
  );
}

/** Read the settings node of the current database. */
export async function readDatabaseSettings(): Promise<DatabaseSettingsNode> {
  const node = await backendAdapter.getNode(DATABASE_SETTINGS_NODE_ID);
  if (!isDatabaseSettingsNode(node)) {
    throw new Error("The database's settings node is missing");
  }
  return node;
}

/**
 * Write fields of the settings node. A write that loses a race with another
 * settings write re-reads the winning version and applies once more.
 */
export async function updateDatabaseSettings(
  update: DatabaseSettingsNodeUpdate
): Promise<DatabaseSettingsNode> {
  for (let attempt = 0; ; attempt++) {
    const current = await readDatabaseSettings();
    try {
      return await backendAdapter.updateDatabaseSettingsNode(
        DATABASE_SETTINGS_NODE_ID,
        current.version,
        update
      );
    } catch (error) {
      if (attempt > 0 || !isVersionConflict(error)) throw error;
    }
  }
}
