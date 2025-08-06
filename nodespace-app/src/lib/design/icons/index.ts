/**
 * NodeSpace SVG Icon System
 *
 * A modular, tree-shakable icon system with TypeScript support.
 * Supports theme integration via currentColor and follows Material Design specs.
 *
 * Usage:
 * ```svelte
 * <script>
 *   import Icon, { type IconName } from '$lib/design/icons';
 * </script>
 *
 * <Icon name="text" size={24} color="var(--ns-color-text-primary)" />
 * ```
 */

// Export the main Icon component
export { default as default } from './Icon.svelte';
export { default as Icon } from './Icon.svelte';

// Export icon types for TypeScript autocomplete
export type { IconName } from './Icon.svelte';

// Export individual icon paths for direct usage if needed
export { textIcon } from './ui/text.js';

/**
 * Available Icons:
 * - text: Document with pencil icon for text content and editing
 *
 * Future icons will be added to ./ui/ directory and exported here.
 */
