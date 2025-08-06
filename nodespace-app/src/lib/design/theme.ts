/**
 * Theme Management System
 *
 * Provides theme switching functionality with Svelte stores,
 * system preference detection, and CSS custom property updates.
 */

import { writable, derived, get } from 'svelte/store';
import { browser } from '$app/environment';
import type { Theme } from './tokens.js';
import {
  getTokens,
  getNodeTokens,
  lightTokens,
  darkTokens,
  lightNodeTokens,
  darkNodeTokens
} from './tokens.js';
import { generateThemeCSS } from './css-generator.js';

// Theme preference store
export const themePreference = writable<Theme>('system');

// System theme detection store
export const systemTheme = writable<'light' | 'dark'>('light');

// Resolved theme store (combines preference and system detection)
export const currentTheme = derived(
  [themePreference, systemTheme],
  ([$themePreference, $systemTheme]) => {
    if ($themePreference === 'system') {
      return $systemTheme;
    }
    return $themePreference === 'dark' ? 'dark' : 'light';
  }
);

// Design tokens stores (reactive to theme changes)
export const designTokens = derived(currentTheme, ($currentTheme) => {
  return getTokens($currentTheme === 'dark' ? 'dark' : 'light');
});

export const nodeTokens = derived(currentTheme, ($currentTheme) => {
  return getNodeTokens($currentTheme === 'dark' ? 'dark' : 'light');
});

// Theme preferences storage key
const THEME_STORAGE_KEY = 'nodespace-theme-preference';

// Initialize theme management
export function initializeTheme() {
  if (!browser) return;

  // Load saved theme preference
  const savedTheme = localStorage.getItem(THEME_STORAGE_KEY) as Theme;
  if (savedTheme && ['light', 'dark', 'system'].includes(savedTheme)) {
    themePreference.set(savedTheme);
  }

  // Detect system theme preference
  const mediaQuery = window.matchMedia('(prefers-color-scheme: dark)');
  systemTheme.set(mediaQuery.matches ? 'dark' : 'light');

  // Listen for system theme changes
  const handleSystemThemeChange = (e: MediaQueryListEvent) => {
    systemTheme.set(e.matches ? 'dark' : 'light');
  };

  mediaQuery.addEventListener('change', handleSystemThemeChange);

  // Apply initial theme
  applyThemeToDOM(get(currentTheme));

  // Listen for theme changes and apply to DOM
  const unsubscribe = currentTheme.subscribe(applyThemeToDOM);

  // Save theme preference changes
  const unsubscribePreference = themePreference.subscribe((theme) => {
    localStorage.setItem(THEME_STORAGE_KEY, theme);
  });

  // Return cleanup function
  return () => {
    mediaQuery.removeEventListener('change', handleSystemThemeChange);
    unsubscribe();
    unsubscribePreference();
  };
}

// Apply theme to DOM by setting data attribute and injecting CSS
function applyThemeToDOM(theme: 'light' | 'dark') {
  if (!browser) return;

  // Set data-theme attribute on document root
  document.documentElement.setAttribute('data-theme', theme);

  // Add theme class to body for additional styling hooks
  document.body.classList.remove('theme-light', 'theme-dark');
  document.body.classList.add(`theme-${theme}`);

  // Inject or update design system CSS
  updateDesignSystemCSS();

  // Dispatch custom event for theme change
  window.dispatchEvent(
    new CustomEvent('themechange', {
      detail: { theme, tokens: getTokens(theme), nodeTokens: getNodeTokens(theme) }
    })
  );
}

// Update design system CSS in the DOM
function updateDesignSystemCSS() {
  if (!browser) return;

  const styleId = 'nodespace-design-system';
  let styleElement = document.getElementById(styleId) as HTMLStyleElement;

  if (!styleElement) {
    styleElement = document.createElement('style');
    styleElement.id = styleId;
    styleElement.setAttribute('data-source', 'nodespace-design-system');
    document.head.appendChild(styleElement);
  }

  // Generate complete theme CSS
  const css = generateThemeCSS(lightTokens, darkTokens, lightNodeTokens, darkNodeTokens);
  styleElement.textContent = css;
}

// Theme switching functions
export function setTheme(theme: Theme) {
  themePreference.set(theme);
}

export function toggleTheme() {
  const current = get(themePreference);

  if (current === 'system') {
    // If system, switch to opposite of current system theme
    const system = get(systemTheme);
    setTheme(system === 'dark' ? 'light' : 'dark');
  } else if (current === 'light') {
    setTheme('dark');
  } else {
    setTheme('light');
  }
}

export function resetThemeToSystem() {
  setTheme('system');
}

// Utility functions for components
export function getThemeIcon(theme: Theme, currentResolvedTheme: 'light' | 'dark'): string {
  switch (theme) {
    case 'light':
      return '☀️';
    case 'dark':
      return '🌙';
    case 'system':
      return currentResolvedTheme === 'dark' ? '🌙' : '☀️';
  }
}

export function getThemeLabel(theme: Theme): string {
  switch (theme) {
    case 'light':
      return 'Light';
    case 'dark':
      return 'Dark';
    case 'system':
      return 'System';
  }
}

// CSS custom property utilities for components
export function getCSSVar(property: string): string {
  if (!browser) return '';
  return getComputedStyle(document.documentElement).getPropertyValue(`--ns-${property}`);
}

export function setCSSVar(property: string, value: string): void {
  if (!browser) return;
  document.documentElement.style.setProperty(`--ns-${property}`, value);
}

// Theme-aware color utilities
export function getThemeAwareColor(lightColor: string, darkColor: string): string {
  const theme = get(currentTheme);
  return theme === 'dark' ? darkColor : lightColor;
}

// Reactive theme context for components
export interface ThemeContext {
  theme: 'light' | 'dark';
  preference: Theme;
  tokens: ReturnType<typeof getTokens>;
  nodeTokens: ReturnType<typeof getNodeTokens>;
  setTheme: typeof setTheme;
  toggleTheme: typeof toggleTheme;
  resetThemeToSystem: typeof resetThemeToSystem;
}

export const createThemeContext = (): ThemeContext => {
  return {
    theme: get(currentTheme),
    preference: get(themePreference),
    tokens: get(designTokens),
    nodeTokens: get(nodeTokens),
    setTheme,
    toggleTheme,
    resetThemeToSystem
  };
};

// Animation utilities for theme transitions
export function createThemeTransition(
  element: HTMLElement,
  options: {
    property?: string;
    duration?: number;
    easing?: string;
  } = {}
) {
  const { property = 'all', duration = 300, easing = 'cubic-bezier(0.4, 0, 0.2, 1)' } = options;

  const transitionValue = `${property} ${duration}ms ${easing}`;

  element.style.transition = transitionValue;

  return {
    destroy() {
      element.style.transition = '';
    }
  };
}

// Svelte action for theme transitions
export function themeTransition(
  element: HTMLElement,
  options?: {
    property?: string;
    duration?: number;
    easing?: string;
  }
) {
  return createThemeTransition(element, options);
}

// Theme persistence utilities
export function exportThemeSettings(): string {
  return JSON.stringify({
    theme: get(themePreference),
    timestamp: new Date().toISOString(),
    version: '1.0'
  });
}

export function importThemeSettings(settingsJSON: string): boolean {
  try {
    const settings = JSON.parse(settingsJSON);
    if (settings.theme && ['light', 'dark', 'system'].includes(settings.theme)) {
      setTheme(settings.theme);
      return true;
    }
  } catch (error) {
    console.error('Failed to import theme settings:', error);
  }
  return false;
}

// Theme debugging utilities (development only)
export function debugTheme() {
  if (typeof window === 'undefined') return;

  // Development debugging only
  if (import.meta.env.DEV) {
    console.group('NodeSpace Theme Debug');
    console.log('Theme Preference:', get(themePreference));
    console.log('System Theme:', get(systemTheme));
    console.log('Current Theme:', get(currentTheme));
    console.log('Design Tokens:', get(designTokens));
    console.log('Node Tokens:', get(nodeTokens));
    console.log(
      'CSS Variables:',
      Array.from(document.documentElement.style).filter((prop) => prop.startsWith('--ns-'))
    );
    console.groupEnd();
  }
}

// Performance monitoring for theme switches
let themeChangeStart: number = 0;

export function startThemeChangePerformanceMonitoring() {
  themeChangeStart = performance.now();
}

export function endThemeChangePerformanceMonitoring() {
  if (themeChangeStart > 0) {
    const duration = performance.now() - themeChangeStart;
    if (import.meta.env.DEV) {
      console.log(`Theme change completed in ${duration.toFixed(2)}ms`);
    }
    themeChangeStart = 0;
  }
}
