/**
 * CSS Custom Properties Generator
 *
 * Generates CSS custom properties from design tokens for theme switching
 * and consistent styling across the application.
 */

import type { DesignTokens, NodeTokens, Theme } from './tokens.js';
import { getTokens, getNodeTokens } from './tokens.js';

// CSS variable naming conventions
const CSS_PREFIX = '--ns'; // NodeSpace prefix

// Convert nested token object to flat CSS custom properties
export function generateCSSCustomProperties(tokens: DesignTokens, nodeTokens: NodeTokens): string {
  const cssProperties: string[] = [];

  // Color tokens
  Object.entries(tokens.color.primary).forEach(([key, value]) => {
    cssProperties.push(`  ${CSS_PREFIX}-color-primary-${key}: ${value};`);
  });

  Object.entries(tokens.color.surface).forEach(([key, value]) => {
    cssProperties.push(`  ${CSS_PREFIX}-color-surface-${key}: ${value};`);
  });

  Object.entries(tokens.color.text).forEach(([key, value]) => {
    cssProperties.push(`  ${CSS_PREFIX}-color-text-${key}: ${value};`);
  });

  Object.entries(tokens.color.border).forEach(([key, value]) => {
    cssProperties.push(`  ${CSS_PREFIX}-color-border-${key}: ${value};`);
  });

  Object.entries(tokens.color.state).forEach(([key, value]) => {
    cssProperties.push(`  ${CSS_PREFIX}-color-state-${key}: ${value};`);
  });

  Object.entries(tokens.color.interactive).forEach(([key, value]) => {
    cssProperties.push(`  ${CSS_PREFIX}-color-interactive-${key}: ${value};`);
  });

  // Typography tokens
  Object.entries(tokens.typography.fontFamily).forEach(([key, value]) => {
    cssProperties.push(`  ${CSS_PREFIX}-font-family-${key}: ${value};`);
  });

  Object.entries(tokens.typography.fontSize).forEach(([key, value]) => {
    cssProperties.push(`  ${CSS_PREFIX}-font-size-${key}: ${value};`);
  });

  Object.entries(tokens.typography.fontWeight).forEach(([key, value]) => {
    cssProperties.push(`  ${CSS_PREFIX}-font-weight-${key}: ${value};`);
  });

  Object.entries(tokens.typography.lineHeight).forEach(([key, value]) => {
    cssProperties.push(`  ${CSS_PREFIX}-line-height-${key}: ${value};`);
  });

  Object.entries(tokens.typography.letterSpacing).forEach(([key, value]) => {
    cssProperties.push(`  ${CSS_PREFIX}-letter-spacing-${key}: ${value};`);
  });

  // Spacing tokens
  Object.entries(tokens.spacing).forEach(([key, value]) => {
    cssProperties.push(`  ${CSS_PREFIX}-spacing-${key}: ${value};`);
  });

  // Shadow tokens
  Object.entries(tokens.shadow).forEach(([key, value]) => {
    cssProperties.push(`  ${CSS_PREFIX}-shadow-${key}: ${value};`);
  });

  // Radius tokens
  Object.entries(tokens.radius).forEach(([key, value]) => {
    cssProperties.push(`  ${CSS_PREFIX}-radius-${key}: ${value};`);
  });

  // Component size tokens
  Object.entries(tokens.componentSize).forEach(([componentName, sizes]) => {
    Object.entries(sizes).forEach(([sizeKey, value]) => {
      cssProperties.push(
        `  ${CSS_PREFIX}-${componentName.replace(/([A-Z])/g, '-$1').toLowerCase()}-${sizeKey}: ${value};`
      );
    });
  });

  // Animation tokens
  Object.entries(tokens.animation.duration).forEach(([key, value]) => {
    cssProperties.push(`  ${CSS_PREFIX}-duration-${key}: ${value};`);
  });

  Object.entries(tokens.animation.easing).forEach(([key, value]) => {
    cssProperties.push(`  ${CSS_PREFIX}-easing-${key}: ${value};`);
  });

  // Breakpoint tokens
  Object.entries(tokens.breakpoint).forEach(([key, value]) => {
    cssProperties.push(`  ${CSS_PREFIX}-breakpoint-${key}: ${value};`);
  });

  // Node-specific tokens
  Object.entries(nodeTokens.nodeType).forEach(([nodeType, colors]) => {
    Object.entries(colors).forEach(([property, value]) => {
      cssProperties.push(`  ${CSS_PREFIX}-node-${nodeType}-${property}: ${value};`);
    });
  });

  // Node state tokens
  Object.entries(nodeTokens.state).forEach(([state, values]) => {
    if (typeof values === 'object' && 'background' in values) {
      Object.entries(values).forEach(([property, value]) => {
        cssProperties.push(`  ${CSS_PREFIX}-node-state-${state}-${property}: ${value};`);
      });
    }
  });

  return cssProperties.join('\n');
}

// Generate complete CSS with theme support
export function generateThemeCSS(
  lightTokens: DesignTokens,
  darkTokens: DesignTokens,
  lightNodeTokens: NodeTokens,
  darkNodeTokens: NodeTokens
): string {
  const lightProperties = generateCSSCustomProperties(lightTokens, lightNodeTokens);
  const darkProperties = generateCSSCustomProperties(darkTokens, darkNodeTokens);

  return `/* NodeSpace Design System CSS Custom Properties */

:root {
${lightProperties}
}

@media (prefers-color-scheme: dark) {
  :root {
${darkProperties}
  }
}

/* Light theme class override */
[data-theme="light"] {
${lightProperties}
}

/* Dark theme class override */
[data-theme="dark"] {
${darkProperties}
}

/* Base element styles using design tokens */
body {
  font-family: var(${CSS_PREFIX}-font-family-ui);
  font-size: var(${CSS_PREFIX}-font-size-sm);
  line-height: var(${CSS_PREFIX}-line-height-normal);
  color: var(${CSS_PREFIX}-color-text-primary);
  background-color: var(${CSS_PREFIX}-color-surface-background);
  margin: 0;
  padding: 0;
  transition: color var(${CSS_PREFIX}-duration-normal) var(${CSS_PREFIX}-easing-easeInOut),
              background-color var(${CSS_PREFIX}-duration-normal) var(${CSS_PREFIX}-easing-easeInOut);
}

/* Component base classes using design system tokens */

/* Button component base */
.ns-button {
  display: inline-flex;
  align-items: center;
  justify-content: center;
  gap: var(${CSS_PREFIX}-spacing-2);
  padding: var(${CSS_PREFIX}-spacing-2) var(${CSS_PREFIX}-spacing-4);
  font-family: var(${CSS_PREFIX}-font-family-ui);
  font-size: var(${CSS_PREFIX}-font-size-sm);
  font-weight: var(${CSS_PREFIX}-font-weight-medium);
  line-height: var(${CSS_PREFIX}-line-height-tight);
  border: 1px solid var(${CSS_PREFIX}-color-border-default);
  border-radius: var(${CSS_PREFIX}-radius-base);
  background-color: var(${CSS_PREFIX}-color-surface-background);
  color: var(${CSS_PREFIX}-color-text-primary);
  cursor: pointer;
  transition: all var(${CSS_PREFIX}-duration-fast) var(${CSS_PREFIX}-easing-easeInOut);
  text-decoration: none;
  outline: none;
}

.ns-button:hover {
  border-color: var(${CSS_PREFIX}-color-border-strong);
  background-color: var(${CSS_PREFIX}-color-surface-panel);
  box-shadow: var(${CSS_PREFIX}-shadow-sm);
}

.ns-button:focus {
  outline: 2px solid var(${CSS_PREFIX}-color-primary-500);
  outline-offset: 2px;
}

.ns-button:active {
  transform: translateY(1px);
  box-shadow: var(${CSS_PREFIX}-shadow-inner);
}

.ns-button:disabled {
  opacity: 0.6;
  cursor: not-allowed;
  pointer-events: none;
}

/* Button variants */
.ns-button--primary {
  background-color: var(${CSS_PREFIX}-color-interactive-idle);
  color: var(${CSS_PREFIX}-color-text-inverse);
  border-color: var(${CSS_PREFIX}-color-interactive-idle);
}

.ns-button--primary:hover {
  background-color: var(${CSS_PREFIX}-color-interactive-hover);
  border-color: var(${CSS_PREFIX}-color-interactive-hover);
}

.ns-button--primary:active {
  background-color: var(${CSS_PREFIX}-color-interactive-active);
  border-color: var(${CSS_PREFIX}-color-interactive-active);
}

/* Input component base */
.ns-input {
  display: block;
  width: 100%;
  padding: var(${CSS_PREFIX}-spacing-3) var(${CSS_PREFIX}-spacing-4);
  font-family: var(${CSS_PREFIX}-font-family-ui);
  font-size: var(${CSS_PREFIX}-font-size-sm);
  line-height: var(${CSS_PREFIX}-line-height-normal);
  color: var(${CSS_PREFIX}-color-text-primary);
  background-color: var(${CSS_PREFIX}-color-surface-input);
  border: 1px solid var(${CSS_PREFIX}-color-border-default);
  border-radius: var(${CSS_PREFIX}-radius-base);
  outline: none;
  transition: all var(${CSS_PREFIX}-duration-fast) var(${CSS_PREFIX}-easing-easeInOut);
}

.ns-input:hover {
  border-color: var(${CSS_PREFIX}-color-border-strong);
}

.ns-input:focus {
  border-color: var(${CSS_PREFIX}-color-primary-500);
  box-shadow: 0 0 0 3px var(${CSS_PREFIX}-color-primary-200);
}

.ns-input::placeholder {
  color: var(${CSS_PREFIX}-color-text-placeholder);
}

.ns-input:disabled {
  opacity: 0.6;
  background-color: var(${CSS_PREFIX}-color-surface-panel);
  cursor: not-allowed;
}

/* Panel component base */
.ns-panel {
  background-color: var(${CSS_PREFIX}-color-surface-elevated);
  border: 1px solid var(${CSS_PREFIX}-color-border-default);
  border-radius: var(${CSS_PREFIX}-radius-lg);
  box-shadow: var(${CSS_PREFIX}-shadow-sm);
  padding: var(${CSS_PREFIX}-spacing-6);
  transition: all var(${CSS_PREFIX}-duration-normal) var(${CSS_PREFIX}-easing-easeInOut);
}

.ns-panel:hover {
  box-shadow: var(${CSS_PREFIX}-shadow-md);
}

/* Node component base */
.ns-node {
  background-color: var(${CSS_PREFIX}-node-state-idle-background);
  border: 1px solid var(${CSS_PREFIX}-node-state-idle-border);
  border-radius: var(${CSS_PREFIX}-radius-lg);
  box-shadow: var(${CSS_PREFIX}-node-state-idle-shadow);
  padding: var(${CSS_PREFIX}-spacing-4);
  margin: var(${CSS_PREFIX}-spacing-2);
  transition: all var(${CSS_PREFIX}-duration-fast) var(${CSS_PREFIX}-easing-easeInOut);
  cursor: pointer;
  outline: none;
}

.ns-node:hover {
  background-color: var(${CSS_PREFIX}-node-state-hover-background);
  border-color: var(${CSS_PREFIX}-node-state-hover-border);
  box-shadow: var(${CSS_PREFIX}-node-state-hover-shadow);
}

.ns-node:focus {
  background-color: var(${CSS_PREFIX}-node-state-focus-background);
  border-color: var(${CSS_PREFIX}-node-state-focus-border);
  box-shadow: var(${CSS_PREFIX}-node-state-focus-shadow);
}

.ns-node:active {
  background-color: var(${CSS_PREFIX}-node-state-active-background);
  border-color: var(${CSS_PREFIX}-node-state-active-border);
  box-shadow: var(${CSS_PREFIX}-node-state-active-shadow);
}

.ns-node--selected {
  background-color: var(${CSS_PREFIX}-node-state-selected-background);
  border-color: var(${CSS_PREFIX}-node-state-selected-border);
  box-shadow: var(${CSS_PREFIX}-node-state-selected-shadow);
}

.ns-node:disabled,
.ns-node--disabled {
  background-color: var(${CSS_PREFIX}-node-state-disabled-background);
  border-color: var(${CSS_PREFIX}-node-state-disabled-border);
  opacity: var(${CSS_PREFIX}-node-state-disabled-opacity);
  cursor: not-allowed;
  pointer-events: none;
}

/* Node type variants */
.ns-node--text {
  border-left: 4px solid var(${CSS_PREFIX}-node-text-accent);
}

.ns-node--task {
  border-left: 4px solid var(${CSS_PREFIX}-node-task-accent);
}

.ns-node--ai-chat {
  border-left: 4px solid var(${CSS_PREFIX}-node-aiChat-accent);
}

.ns-node--entity {
  border-left: 4px solid var(${CSS_PREFIX}-node-entity-accent);
}

.ns-node--query {
  border-left: 4px solid var(${CSS_PREFIX}-node-query-accent);
}

/* Utility classes */
.ns-text-xs { font-size: var(${CSS_PREFIX}-font-size-xs); }
.ns-text-sm { font-size: var(${CSS_PREFIX}-font-size-sm); }
.ns-text-base { font-size: var(${CSS_PREFIX}-font-size-base); }
.ns-text-lg { font-size: var(${CSS_PREFIX}-font-size-lg); }
.ns-text-xl { font-size: var(${CSS_PREFIX}-font-size-xl); }

.ns-font-normal { font-weight: var(${CSS_PREFIX}-font-weight-normal); }
.ns-font-medium { font-weight: var(${CSS_PREFIX}-font-weight-medium); }
.ns-font-semibold { font-weight: var(${CSS_PREFIX}-font-weight-semibold); }
.ns-font-bold { font-weight: var(${CSS_PREFIX}-font-weight-bold); }

.ns-text-primary { color: var(${CSS_PREFIX}-color-text-primary); }
.ns-text-secondary { color: var(${CSS_PREFIX}-color-text-secondary); }
.ns-text-tertiary { color: var(${CSS_PREFIX}-color-text-tertiary); }

.ns-bg-surface { background-color: var(${CSS_PREFIX}-color-surface-background); }
.ns-bg-panel { background-color: var(${CSS_PREFIX}-color-surface-panel); }
.ns-bg-elevated { background-color: var(${CSS_PREFIX}-color-surface-elevated); }

/* Spacing utilities */
.ns-p-0 { padding: var(${CSS_PREFIX}-spacing-0); }
.ns-p-1 { padding: var(${CSS_PREFIX}-spacing-1); }
.ns-p-2 { padding: var(${CSS_PREFIX}-spacing-2); }
.ns-p-3 { padding: var(${CSS_PREFIX}-spacing-3); }
.ns-p-4 { padding: var(${CSS_PREFIX}-spacing-4); }
.ns-p-5 { padding: var(${CSS_PREFIX}-spacing-5); }
.ns-p-6 { padding: var(${CSS_PREFIX}-spacing-6); }

.ns-m-0 { margin: var(${CSS_PREFIX}-spacing-0); }
.ns-m-1 { margin: var(${CSS_PREFIX}-spacing-1); }
.ns-m-2 { margin: var(${CSS_PREFIX}-spacing-2); }
.ns-m-3 { margin: var(${CSS_PREFIX}-spacing-3); }
.ns-m-4 { margin: var(${CSS_PREFIX}-spacing-4); }
.ns-m-5 { margin: var(${CSS_PREFIX}-spacing-5); }
.ns-m-6 { margin: var(${CSS_PREFIX}-spacing-6); }

/* Responsive utilities */
@media (min-width: var(${CSS_PREFIX}-breakpoint-sm)) {
  .ns-sm\\:text-base { font-size: var(${CSS_PREFIX}-font-size-base); }
  .ns-sm\\:p-4 { padding: var(${CSS_PREFIX}-spacing-4); }
}

@media (min-width: var(${CSS_PREFIX}-breakpoint-md)) {
  .ns-md\\:text-lg { font-size: var(${CSS_PREFIX}-font-size-lg); }
  .ns-md\\:p-6 { padding: var(${CSS_PREFIX}-spacing-6); }
}

@media (min-width: var(${CSS_PREFIX}-breakpoint-lg)) {
  .ns-lg\\:text-xl { font-size: var(${CSS_PREFIX}-font-size-xl); }
  .ns-lg\\:p-8 { padding: var(${CSS_PREFIX}-spacing-8); }
}`;
}

// Generate CSS for specific theme
export function generateThemeCSSForTheme(theme: Theme): string {
  const tokens = getTokens(theme);
  const nodeTokens = getNodeTokens(theme);
  return generateCSSCustomProperties(tokens, nodeTokens);
}

// Utility function to get CSS custom property name
export function cssVar(path: string): string {
  return `var(${CSS_PREFIX}-${path})`;
}

// CSS custom property name mapper for easy access
export const cssVars = {
  // Colors
  color: {
    primary: (shade: string) => cssVar(`color-primary-${shade}`),
    surface: (type: string) => cssVar(`color-surface-${type}`),
    text: (type: string) => cssVar(`color-text-${type}`),
    border: (type: string) => cssVar(`color-border-${type}`),
    state: (state: string) => cssVar(`color-state-${state}`),
    interactive: (state: string) => cssVar(`color-interactive-${state}`)
  },

  // Typography
  typography: {
    fontFamily: (type: string) => cssVar(`font-family-${type}`),
    fontSize: (size: string) => cssVar(`font-size-${size}`),
    fontWeight: (weight: string) => cssVar(`font-weight-${weight}`),
    lineHeight: (height: string) => cssVar(`line-height-${height}`),
    letterSpacing: (spacing: string) => cssVar(`letter-spacing-${spacing}`)
  },

  // Spacing
  spacing: (size: string) => cssVar(`spacing-${size}`),

  // Component sizes
  componentSize: {
    nodeIndicator: (property: string) => cssVar(`node-indicator-${property}`)
  },

  // Other tokens
  shadow: (size: string) => cssVar(`shadow-${size}`),
  radius: (size: string) => cssVar(`radius-${size}`),
  duration: (speed: string) => cssVar(`duration-${speed}`),
  easing: (type: string) => cssVar(`easing-${type}`),

  // Node-specific
  node: {
    type: (nodeType: string, property: string) => cssVar(`node-${nodeType}-${property}`),
    state: (state: string, property: string) => cssVar(`node-state-${state}-${property}`)
  }
};
