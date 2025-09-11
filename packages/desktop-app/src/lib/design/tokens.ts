/**
 * NodeSpace Design System Tokens
 *
 * Simplified design token system using shadcn-svelte as foundation
 * with minimal NodeSpace-specific extensions.
 */

import type { NodeType as RegistryNodeType } from './icons/registry.js';

// Re-export NodeType from registry for consistency
export type NodeType = RegistryNodeType;

// Node-specific colors for different node types
export const nodeTypeColors = {
  text: 'hsl(142 71% 45%)', // Green
  document: 'hsl(142 71% 45%)', // Green (same as text)
  task: 'hsl(25 95% 53%)', // Orange
  'ai-chat': 'hsl(221 83% 53%)', // Blue
  ai_chat: 'hsl(221 83% 53%)', // Blue (alias)
  user: 'hsl(142 71% 45%)', // Green (same as text)
  entity: 'hsl(271 81% 56%)', // Purple
  query: 'hsl(330 81% 60%)' // Pink
} as const;

// Theme types for runtime theme switching
export type Theme = 'light' | 'dark' | 'system';

// Processing state opacity for animations
export const processingOpacity = 0.7;

// Get theme-aware node color
export function getNodeTypeColor(nodeType: NodeType): string {
  return nodeTypeColors[nodeType];
}

// Theme switching utility functions
export function getResolvedTheme(theme: Theme, systemTheme?: 'light' | 'dark'): 'light' | 'dark' {
  if (theme === 'system') {
    return (
      systemTheme ||
      (typeof window !== 'undefined' && window.matchMedia('(prefers-color-scheme: dark)').matches
        ? 'dark'
        : 'light')
    );
  }
  return theme as 'light' | 'dark';
}

// Legacy compatibility - these functions are deprecated but kept for migration
export function getTokens() {
  console.warn('getTokens() is deprecated. Use shadcn-svelte CSS variables instead.');
  return null;
}

export function getNodeTokens() {
  console.warn('getNodeTokens() is deprecated. Use nodeTypeColors and CSS variables instead.');
  return null;
}
