/**
 * Component-based Node Decoration Types
 *
 * Defines the structure for component-based node decorations in the
 * universal node reference system (Phase 2.2).
 */

import type { SvelteComponent } from 'svelte';

/**
 * Component decoration result containing Svelte component and props
 */
export interface ComponentDecoration {
  /** Svelte component class to render */
  component: new (...args: unknown[]) => SvelteComponent;

  /** Props to pass to the component (flexible JSON-like object) */
  props: Record<string, unknown>;

  /** Optional event handlers for the component */
  events?: Record<string, (event: CustomEvent) => void>;

  /** Additional metadata for the component */
  metadata?: Record<string, unknown>;
}

/**
 * Base props that all node reference components receive
 */
export interface BaseNodeReferenceProps {
  /** Unique node identifier */
  nodeId: string;

  /** Node content/text */
  content: string;

  /** Navigation URI for the node */
  href: string;

  /** Type of node (task, user, date, etc.) */
  nodeType: string;

  /** Additional CSS classes */
  className?: string;

  /** Inline styles */
  style?: string;

  /** ARIA label for accessibility */
  ariaLabel?: string;

  /** Optional icon override */
  icon?: string;

  /** Optional color override */
  color?: string;

  /** Node title/display name */
  title?: string;

  /** Additional metadata */
  metadata?: Record<string, unknown>;

  /** Display context for styling decisions */
  displayContext?: 'inline' | 'popup' | 'preview';

  /** Whether the component is disabled */
  disabled?: boolean;

  /** Index signature for additional unknown props */
  [key: string]: unknown;
}

// Note: Specific node reference prop interfaces (TaskNodeReferenceProps, UserNodeReferenceProps, etc.)
// will be added when those node types are properly specified and implemented.
// For now, all node types use BaseNodeReferenceProps.
