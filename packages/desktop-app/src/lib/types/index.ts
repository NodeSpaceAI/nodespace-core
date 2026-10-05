/**
 * Central Type Exports
 *
 * All types exported from this single location for consistency.
 */

// Node types - ONLY source of truth
export type { Node, NodeEnvelope, NodeUpdate, NodeUIState, NodeReference } from './node';
export { isNode, createDefaultUIState } from './node';

// Type-safe node wrappers - Simple types (extend Node with nodeType narrowing only)
export type { TextNode } from './text-node';
export { isTextNode, TextNodeHelpers } from './text-node';

export type { HeaderNode } from './header-node';
export { isHeaderNode, getHeaderLevel, getHeaderText, setHeaderLevel, HeaderNodeHelpers } from './header-node';

export type { DateNode } from './date-node';
export {
  isDateNode,
  getDate,
  getDateObject,
  isValidDateId,
  generateDateId,
  DateNodeHelpers
} from './date-node';

export type { CodeBlockNode } from './code-block-node';
export { isCodeBlockNode, getLanguage, setLanguage, CodeBlockNodeHelpers } from './code-block-node';

export type { QuoteBlockNode } from './quote-block-node';
export { isQuoteBlockNode, QuoteBlockNodeHelpers } from './quote-block-node';

export type { OrderedListNode } from './ordered-list-node';
export { isOrderedListNode, OrderedListNodeHelpers } from './ordered-list-node';

// Type-safe node wrappers - Type-specific node types (flat structure matching Rust serialization)
export type {
  TaskNode,
  TaskNodeUpdate,
  TaskStatus,
  Priority,
  CoreTaskStatus,
  CoreTaskPriority
} from './task-node';
export {
  isTaskNode,
  getTaskStatus,
  setTaskStatus,
  getTaskPriority,
  setTaskPriority,
  getTaskDueDate,
  setTaskDueDate,
  TaskNodeHelpers
} from './task-node';

export type { PersonNode, PersonNodeUpdate } from './person-node';
export { isPersonNode, nodeToPersonNode } from './person-node';

export type { ProjectNode, ProjectNodeUpdate, ProjectStatus } from './project-node';
export { isProjectNode, nodeToProjectNode } from './project-node';

export type { SpecNode, SpecNodeUpdate, SpecStatus } from './spec-node';
export { isSpecNode, nodeToSpecNode } from './spec-node';

export type { PlanNode, PlanNodeUpdate, PlanStatus } from './plan-node';
export { isPlanNode, nodeToPlanNode } from './plan-node';

export type { DecisionNode, DecisionNodeUpdate, DecisionStatus } from './decision-node';
export { isDecisionNode, nodeToDecisionNode } from './decision-node';

export type { QueryNode, QueryNodeUpdate, QueryGeneratedBy } from './query';
export { nodeToQueryNode } from './query';

// These types' wire shapes are generated from Rust's `nodespace-types`.
export type {
  CollectionNode,
  CollectionNodeUpdate,
  DatabaseSettingsNode,
  DatabaseSettingsNodeUpdate,
  PlayNode,
  PlayNodeUpdate,
  SkillNode,
  SkillNodeUpdate
} from './generated';

export type {
  SchemaNode,
  SchemaField,
  SchemaFieldType,
  SchemaProtectionLevel,
  EnumValue
} from './schema-node';
// Only isSchemaNode remains - type guard for runtime checking
// All other properties are typed top-level fields accessed directly (e.g., node.isCore, node.fields)
export { isSchemaNode } from './schema-node';

// Core node type registry
export type { CoreNodeTypeId, CoreTypeEntry } from './core-node-types';
export {
  CORE_NODE_TYPES,
  coreTypeEntry,
  isA,
  isCoreNodeType,
  isExactly,
  nearestCoreType,
  typeChain
} from './core-node-types';

// Error types
export type { CommandError } from './errors';
export { isCommandError, toError, DatabaseInitializationError, NodeOperationError } from './errors';

// Event types
export type { NodeEventData, HierarchyRelationship, NodeWithChildren, PersistenceFailedEvent } from './event-types';

// Re-export existing types for convenience
export type { NodeViewerProps } from './node-viewers';
