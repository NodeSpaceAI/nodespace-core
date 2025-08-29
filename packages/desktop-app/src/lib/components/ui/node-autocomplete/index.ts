export { default as NodeAutocomplete } from './NodeAutocomplete.svelte';

export interface NodeResult {
  id: string;
  title: string;
  type: 'text' | 'task' | 'ai-chat' | 'entity' | 'query' | 'user' | 'date' | 'document';
  subtitle?: string;
  metadata?: string;
}
