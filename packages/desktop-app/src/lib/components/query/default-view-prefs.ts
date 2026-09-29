/**
 * Per-type view preferences for the type's Default view.
 *
 * The Default view has no backing node, so its chosen view (List/Table/Kanban)
 * and Kanban group-by are remembered per type in browser storage rather than by
 * creating a query node. A saved query stores the same shape on its own node.
 */

import { createLogger } from '$lib/utils/logger';
import { DEFAULT_VIEW_CONFIG, type QueryViewConfigState } from './query-node-model';

const log = createLogger('DefaultViewPrefs');

const STORAGE_PREFIX = 'nodespace:default-view:';

function isViewKind(value: unknown): value is QueryViewConfigState['lastView'] {
  return value === 'list' || value === 'table' || value === 'kanban';
}

/** Read the remembered view for a type; the built-in default when none/invalid. */
export function loadDefaultViewPrefs(typeId: string): QueryViewConfigState {
  try {
    const raw = localStorage.getItem(STORAGE_PREFIX + typeId);
    if (!raw) return { ...DEFAULT_VIEW_CONFIG };
    const parsed: unknown = JSON.parse(raw);
    if (!parsed || typeof parsed !== 'object') return { ...DEFAULT_VIEW_CONFIG };
    const obj = parsed as Record<string, unknown>;
    const result: QueryViewConfigState = {
      lastView: isViewKind(obj.lastView) ? obj.lastView : DEFAULT_VIEW_CONFIG.lastView
    };
    const kanban = obj.kanban;
    if (kanban && typeof kanban === 'object') {
      const groupBy = (kanban as Record<string, unknown>).groupBy;
      if (typeof groupBy === 'string') result.kanban = { groupBy };
    }
    return result;
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
