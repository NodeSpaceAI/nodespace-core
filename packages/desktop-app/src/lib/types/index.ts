/**
 * Central Type Exports
 *
 * All types exported from this single location for consistency.
 */

// Node types - ONLY source of truth
export type { Node, NodeUpdate, NodeUIState } from './node';
export { isNode, createDefaultUIState } from './node';

// Error types
export type { CommandError } from './errors';
export { isCommandError, toError, DatabaseInitializationError, NodeOperationError } from './errors';

// Re-export existing types for convenience
export type { NodeViewerProps } from './node-viewers';
