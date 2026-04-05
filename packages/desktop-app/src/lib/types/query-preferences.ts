/**
 * Query Preferences Type Definitions
 *
 * Stores per-query view preferences (active view, view-specific configuration)
 * in localStorage via QueryPreferencesService. Each QueryNode can have
 * independent view preferences, identified by the node's ID.
 *
 * View configs are optional — a query can have a lastView without any stored
 * view-specific configuration (e.g., column widths, sort order).
 */

import type { ListViewConfig, TableViewConfig, KanbanViewConfig } from './query';

export type { ListViewConfig, TableViewConfig, KanbanViewConfig };

export interface QueryPreferences {
  /** The last active view type for this query */
  lastView: 'list' | 'table' | 'kanban';
  /** Stored configuration for each view type (optional per-view) */
  viewConfigs: {
    list?: ListViewConfig;
    table?: TableViewConfig;
    kanban?: KanbanViewConfig;
  };
}

/**
 * Default preferences applied when no stored preferences are found
 * or when stored data is corrupt/unreadable.
 *
 * Defaults to 'table' since TableView is the only currently implemented view.
 */
export const DEFAULT_QUERY_PREFERENCES: QueryPreferences = {
  lastView: 'table',
  viewConfigs: {}
};
