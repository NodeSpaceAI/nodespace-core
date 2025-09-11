export { default as NodeAutocomplete } from './node-autocomplete.svelte';

import type { NodeType } from '$lib/design/icons/registry.js';

export interface NodeResult {
  id: string;
  title: string;
  type: NodeType;
  subtitle?: string;
  metadata?: string;
}
