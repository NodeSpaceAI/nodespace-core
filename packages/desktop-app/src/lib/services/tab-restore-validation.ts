/**
 * Restore-time validation of persisted tabs.
 *
 * Persisted tabs name nodes by id, and the database they were saved against may
 * have been replaced since. A tab is checked against the active database before
 * it is rendered so a missing node never shows as a "Loading..." tab.
 */

import { backendAdapter } from '$lib/services/backend-adapter';
import { isValidDateId } from '$lib/types/date-node';
import type { Tab } from '$lib/stores/navigation.svelte';
import { createLogger } from '$lib/utils/logger';

const log = createLogger('TabRestoreValidation');

/**
 * Whether a persisted tab should be restored.
 *
 * Tabs that are not backed by a node (settings, search, conflicts, placeholders) and
 * date pages (which exist virtually in every database) are kept. A node tab, including
 * a type's view tab whose id is the type's id, is kept only when the daemon resolves
 * that id. When the daemon cannot answer, the tab is kept: the pane's own hydration
 * still closes it if the node turns out to be missing.
 */
export async function isTabResolvable(tab: Tab): Promise<boolean> {
  if (tab.type !== 'node') return true;
  const nodeId = tab.content?.nodeId;
  if (!nodeId) return true;
  if (isValidDateId(nodeId)) return true;
  try {
    const node = await backendAdapter.getNode(nodeId);
    return node !== null && node !== undefined;
  } catch (error) {
    log.warn(`Could not verify tab ${tab.id} against the database; keeping it:`, error);
    return true;
  }
}
