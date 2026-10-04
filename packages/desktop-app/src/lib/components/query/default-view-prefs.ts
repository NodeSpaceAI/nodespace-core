/**
 * Per-type view preferences for the type's Default view.
 *
 * The Default view has no backing node, so its chosen view (List/Table/Kanban)
 * and Kanban group-by are remembered per type in browser storage rather than by
 * creating a query node. A saved query stores the same shape on its own node.
 */

import { createLogger } from '$lib/utils/logger';
import {
  DEFAULT_VIEW_CONFIG,
  parseViewConfigObject,
  type QueryViewConfigState
} from './query-node-model';

const log = createLogger('DefaultViewPrefs');

const STORAGE_PREFIX = 'nodespace:default-view:';

/** Read the remembered view for a type; the built-in default when none/invalid. */
export function loadDefaultViewPrefs(typeId: string): QueryViewConfigState {
  try {
    const raw = localStorage.getItem(STORAGE_PREFIX + typeId);
    if (!raw) return { ...DEFAULT_VIEW_CONFIG };
    return parseViewConfigObject(JSON.parse(raw));
  } catch (e) {
    log.debug('Default view prefs unavailable, using defaults', { typeId, error: String(e) });
    return { ...DEFAULT_VIEW_CONFIG };
  }
}

/** Remember the view for a type. Storage failures are non-fatal. */
export function saveDefaultViewPrefs(typeId: string, config: QueryViewConfigState): void {
  try {
    localStorage.setItem(STORAGE_PREFIX + typeId, JSON.stringify(config));
  } catch (e) {
    log.warn('Could not persist default view prefs', { typeId, error: String(e) });
  }
}
