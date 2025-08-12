/**
 * Theme Management System
 *
 * Simplified theme switching using shadcn-svelte foundation.
 * Handles light/dark/system theme preferences and applies changes to DOM.
 */

import { writable, derived, get } from 'svelte/store';
import { browser } from '$app/environment';
import type { Theme } from './tokens.js';
import { getResolvedTheme } from './tokens.js';

// Theme preference store
export const themePreference = writable<Theme>('system');

// System theme detection store
export const systemTheme = writable<'light' | 'dark'>('light');

// Resolved theme store (combines preference and system detection)
export const currentTheme = derived(
  [themePreference, systemTheme],
  ([$themePreference, $systemTheme]) => {
    return getResolvedTheme($themePreference, $systemTheme);
  }
);

// Theme storage key
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

// Apply theme to DOM using shadcn-svelte approach
function applyThemeToDOM(theme: 'light' | 'dark') {
  if (!browser) return;

  // Set data-theme attribute on document root
  document.documentElement.setAttribute('data-theme', theme);

  // Apply shadcn-svelte theme class
  document.documentElement.classList.remove('light', 'dark');
  document.documentElement.classList.add(theme);

  // Dispatch custom event for theme change
  window.dispatchEvent(
    new CustomEvent('themechange', {
      detail: { theme }
    })
  );
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
