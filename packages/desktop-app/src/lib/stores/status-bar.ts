/**
 * Status Bar Store
 *
 * Simple store for displaying status messages at the bottom of the app.
 * Used for background operations like import progress and embedding queue status.
 */

import { writable, derived } from 'svelte/store';

export interface StatusBarState {
  /** Whether status bar is enabled (user preference via View menu) */
  enabled: boolean;
  /** Status message to display (right side) */
  message: string;
  /** Progress percentage 0-100 (right side, optional) */
  progress?: number;
  /** Message type for styling */
  type: 'info' | 'success' | 'error';
  /** Number of nodes queued for vector indexing (left side) */
  staleNodesCount: number;
}

const initialState: StatusBarState = {
  enabled: true, // Default to showing
  message: '',
  type: 'info',
  staleNodesCount: 0
};

const { subscribe, set: _set, update } = writable<StatusBarState>(initialState);

/** Derived store: whether status bar should be visible */
export const statusBarVisible = derived(
  { subscribe },
  ($state) => $state.enabled
);

export const statusBar = {
  subscribe,

  /** Toggle status bar visibility (for View menu) */
  toggle() {
    update((state) => ({ ...state, enabled: !state.enabled }));
  },

  /** Set status bar enabled state */
  setEnabled(enabled: boolean) {
    update((state) => ({ ...state, enabled }));
  },

  /** Show a status message */
  show(message: string, progress?: number) {
    update((state) => ({ ...state, message, progress, type: 'info' }));
  },

  /** Show a success message (auto-hides message after 3s, but bar stays) */
  success(message: string) {
    update((state) => ({ ...state, message, type: 'success', progress: undefined }));
    setTimeout(() => {
      update((state) => ({ ...state, message: '', type: 'info' }));
    }, 3000);
  },

  /** Show an error message (stays visible) */
  error(message: string) {
    update((state) => ({ ...state, message, type: 'error', progress: undefined }));
  },

  /** Update progress */
  updateProgress(current: number, total: number, message?: string) {
    update((state) => ({
      ...state,
      message: message ?? state.message,
      progress: Math.round((current / total) * 100)
    }));
  },

  /** Clear the status message (but keep bar visible if enabled) */
  clearMessage() {
    update((state) => ({ ...state, message: '', progress: undefined, type: 'info' }));
  },

  /** Update stale nodes count for embedding queue display */
  setStaleNodesCount(count: number) {
    update((state) => ({ ...state, staleNodesCount: count }));
  },

  /** Legacy hide method - now just clears the message */
  hide() {
    update((state) => ({ ...state, message: '', progress: undefined, type: 'info' }));
  }
};
