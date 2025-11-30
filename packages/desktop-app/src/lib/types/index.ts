/**
 * Central Type Exports
 *
 * All types exported from this single location for consistency.
 */

// Node types - ONLY source of truth
export type { Node, NodeUpdate, NodeUIState } from './node';
export { isNode, createDefaultUIState } from './node';

// Type-safe node wrappers
export type { CodeBlockNode } from './code-block-node';
export { isCodeBlockNode, getLanguage, setLanguage, CodeBlockNodeHelpers } from './code-block-node';

export type { QuoteBlockNode } from './quote-block-node';
export { isQuoteBlockNode, QuoteBlockNodeHelpers } from './quote-block-node';

export type { OrderedListNode } from './ordered-list-node';
export { isOrderedListNode, OrderedListNodeHelpers } from './ordered-list-node';

export type { SchemaNode, SchemaField, ProtectionLevel, EnumValue } from './schema-node';
// Only isSchemaNode remains - type guard for runtime checking
// All other properties are typed top-level fields accessed directly (e.g., node.isCore, node.fields)
export { isSchemaNode } from './schema-node';

// Error types
export type { CommandError } from './errors';
export { isCommandError, toError, DatabaseInitializationError, NodeOperationError } from './errors';

// Event types
export type { NodeEventData, HierarchyRelationship, NodeWithChildren, PersistenceFailedEvent } from './event-types';

// Re-export existing types for convenience
export type { NodeViewerProps } from './node-viewers';
