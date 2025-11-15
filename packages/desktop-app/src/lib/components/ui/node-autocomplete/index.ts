export { default as NodeAutocomplete } from './node-autocomplete.svelte';

import type { NodeType } from '$lib/design/icons/registry.js';

export interface NodeResult {
  id: string;
  title: string;
  nodeType: NodeType;
  subtitle?: string;
  metadata?: string;
  isShortcut?: boolean; // Flag for date shortcuts and other special items
  submenuPosition?: { x: number; y: number }; // For positioning submenus (e.g., date picker)
}
