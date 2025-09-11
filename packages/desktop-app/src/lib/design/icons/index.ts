/**
 * NodeSpace Elegant Icon System
 *
 * Component-based icon registry that unifies design system with implementation.
 * Eliminates architectural inconsistency and provides semantic node-based API.
 *
 * New Semantic Usage (Recommended):
 * ```svelte
 * <script>
 *   import Icon, { type NodeType } from '$lib/design/icons';
 * </script>
 *
 * <Icon nodeType="text" hasChildren={true} size={20} />
 * <Icon nodeType="task" state="completed" />
 * <Icon nodeType="ai-chat" />
 * ```
 *
 * Legacy Usage (Backward Compatible):
 * ```svelte
 * <Icon name="circle" size={24} color="currentColor" />
 * ```
 */

// Export the main Icon component
export { default } from './icon.svelte';
export { default as Icon } from './icon.svelte';

// Export new semantic types (recommended)
export type { NodeType, NodeState, NodeIconProps, IconConfig } from './types';

// Export registry functions
export { getIconConfig, resolveNodeState } from './registry';

// Export legacy types for backward compatibility
export type { IconName } from './types';

// Export individual icon paths for direct usage if needed
export { textIcon } from './ui/text';
export { circleIcon } from './ui/circle';
export { circleRingIcon } from './ui/circleRing';
export { chevronRightIcon } from './ui/chevronRight';
export { taskCompleteIcon } from './ui/taskComplete';
export { taskIncompleteIcon } from './ui/taskIncomplete';
export { taskInProgressIcon } from './ui/taskInProgress';
export { aiSquareIcon } from './ui/aiSquare';
export { calendarIcon } from './ui/calendar';

/**
 * Available Icons:
 * - text: Document with pencil icon for text content and editing
 * - circle: Simple filled circle for node indicators
 * - circleRing: Circle with ring for parent node indicators
 * - chevronRight: Right-pointing chevron for collapse/expand controls
 * - taskComplete: Checkmark in circle for completed tasks
 * - taskIncomplete: Empty circle for uncompleted tasks
 * - taskInProgress: Half-filled circle for tasks in progress
 * - aiSquare: Rounded square with "AI" text for AI chat nodes
 * - calendar: Calendar grid icon for date/time functionality
 *
 * Future icons will be added to ./ui/ directory and exported here.
 */
