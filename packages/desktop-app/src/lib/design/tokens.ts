/**
 * NodeSpace Design System Tokens
 *
 * Simplified design token system using shadcn-svelte as foundation
 * with minimal NodeSpace-specific extensions.
 */

// Node type colors are CSS-only: the --node-* custom properties in app.css are
// the single source of truth and all derive from --primary. There is
// deliberately no TS map of per-type colors — one would hardcode a single
// theme's literal and silently drift from the CSS.

// Theme types for runtime theme switching
export type Theme = 'light' | 'dark' | 'system';

// Processing state opacity for animations
export const processingOpacity = 0.7;

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
