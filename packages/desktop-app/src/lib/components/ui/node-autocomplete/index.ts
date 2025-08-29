export { default as NodeAutocomplete } from './node-autocomplete.svelte';

export interface NodeResult {
  id: string;
  title: string;
  type: 'text' | 'task' | 'ai-chat' | 'entity' | 'query' | 'user' | 'date' | 'document';
  subtitle?: string;
  metadata?: string;
}
