/**
 * NodeSpace SVG Icon System
 *
 * A modular, tree-shakable icon system with TypeScript support.
 * Supports theme integration via currentColor and follows shadcn-svelte design system.
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
export { textIcon } from './ui/text.ts';
export { circleIcon } from './ui/circle.ts';
export { circleRingIcon } from './ui/circle-ring.ts';
export { chevronRightIcon } from './ui/chevron-right.ts';

/**
 * Available Icons:
 * - text: Document with pencil icon for text content and editing
 * - circle: Simple filled circle for node indicators
 * - circle-ring: Circle with ring for parent node indicators
 * - chevron-right: Right-pointing chevron for collapse/expand controls
 *
 * Future icons will be added to ./ui/ directory and exported here.
 */
