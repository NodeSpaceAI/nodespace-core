/**
 * NodeSpace Design System Tokens
 *
 * Simplified design token system using shadcn-svelte as foundation
 * with minimal NodeSpace-specific extensions.
 */

import type { NodeType as RegistryNodeType } from './icons/registry.js';
import { createLogger } from '$lib/utils/logger';

const log = createLogger('Tokens');

// Re-export NodeType from registry for consistency
export type NodeType = RegistryNodeType;

// Node-specific colors - Subtle Tint System (Scheme 3)
export const nodeTypeColors = {
  text: 'hsl(200 40% 45%)', // Blue-gray (Scheme 3)
  document: 'hsl(200 40% 45%)', // Blue-gray (Scheme 3)
  task: 'hsl(200 40% 45%)', // Blue-gray (Scheme 3)
  'ai-chat': 'hsl(200 40% 45%)', // Blue-gray (Scheme 3)
  ai_chat: 'hsl(200 40% 45%)', // Blue-gray (Scheme 3)
  user: 'hsl(200 40% 45%)', // Blue-gray (Scheme 3)
  entity: 'hsl(200 40% 45%)', // Blue-gray (Scheme 3)
  query: 'hsl(200 40% 45%)' // Blue-gray (Scheme 3)
} as const;

// Theme types for runtime theme switching
export type Theme = 'light' | 'dark' | 'system';

// Processing state opacity for animations
export const processingOpacity = 0.7;

// Get theme-aware node color
export function getNodeTypeColor(nodeType: NodeType): string {
  return nodeTypeColors[nodeType as keyof typeof nodeTypeColors] || nodeTypeColors.text;
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
  log.warn('getTokens() is deprecated. Use shadcn-svelte CSS variables instead.');
  return null;
}

export function getNodeTokens() {
  log.warn('getNodeTokens() is deprecated. Use nodeTypeColors and CSS variables instead.');
  return null;
}
