/**
 * EventTypes - Type-safe Event Schema for NodeSpace EventBus System
 *
 * Defines the event schema for dynamic coordination between services,
 * building on the existing decorateReference() architecture.
 *
 * Focus Areas:
 * - Dynamic State Coordination (real-time status updates)
 * - Interactive Decoration Events (click handling, updates)
 * - Cache & Reference Coordination (invalidation, updates)
 * - Phase 2+ Preparation (backlinking, AI, collaboration)
 */

// ============================================================================
// Base Event Interface
// ============================================================================

export interface BaseEvent {
  type: string;
  timestamp: number;
  source: string;
  namespace: string;
}

// ============================================================================
// Dynamic State Coordination Events
// ============================================================================

export interface NodeStatusChangedEvent extends BaseEvent {
  type: 'node:status-changed';
  namespace: 'coordination';
  nodeId: string;
  status: NodeStatus;
  previousStatus?: NodeStatus;
  metadata?: Record<string, unknown>;
}

export interface UserStatusChangedEvent extends BaseEvent {
  type: 'user:status-changed';
  namespace: 'coordination';
  userId: string;
  status: UserStatus;
  activeNode?: string;
  metadata?: Record<string, unknown>;
}

// Status types
export type NodeStatus =
  | 'active'
  | 'editing'
  | 'focused'
  | 'expanded'
  | 'collapsed'
  | 'selected'
  | 'processing'
  | 'error';

export type UserStatus = 'active' | 'typing' | 'idle' | 'away';

// ============================================================================
// Interactive Decoration Events
// ============================================================================

export interface DecorationClickedEvent extends BaseEvent {
  type: 'decoration:clicked';
  namespace: 'interaction';
  nodeId: string;
  decorationType: string;
  target: string;
  clickPosition: { x: number; y: number };
  metadata?: Record<string, unknown>;
}

export interface DecorationUpdateNeededEvent extends BaseEvent {
  type: 'decoration:update-needed';
  namespace: 'interaction';
  nodeId: string;
  decorationType: string;
  reason:
    | 'content-changed'
    | 'status-changed'
    | 'reference-updated'
    | 'cache-invalidated'
    | 'nodeType-changed';
  metadata?: Record<string, unknown>;
}

export interface DecorationHoverEvent extends BaseEvent {
  type: 'decoration:hover';
  namespace: 'interaction';
  nodeId: string;
  decorationType: string;
  target: string;
  hoverState: 'enter' | 'leave';
  metadata?: Record<string, unknown>;
}

// ============================================================================
// Cache & Reference Coordination Events
// ============================================================================

export interface CacheInvalidateEvent extends BaseEvent {
  type: 'cache:invalidate';
  namespace: 'coordination';
  cacheKey: string;
  scope: 'single' | 'node' | 'global';
  nodeId?: string;
  reason: string;
  metadata?: Record<string, unknown>;
}

export interface ReferencesUpdateNeededEvent extends BaseEvent {
  type: 'references:update-needed';
  namespace: 'coordination';
  nodeId: string;
  updateType: 'content' | 'status' | 'hierarchy' | 'deletion' | 'nodeType';
  affectedReferences?: string[];
  metadata?: Record<string, unknown>;
}

export interface ReferenceResolutionEvent extends BaseEvent {
  type: 'reference:resolved';
  namespace: 'coordination';
  referenceId: string;
  target: string;
  nodeId: string;
  resolutionResult: 'found' | 'not-found' | 'ambiguous';
  metadata?: Record<string, unknown>;
}

// ============================================================================
// Node Lifecycle Events (extending existing NodeManager events)
// ============================================================================

export interface NodeCreatedEvent extends BaseEvent {
  type: 'node:created';
  namespace: 'lifecycle';
  nodeId: string;
  nodeType: string;
  metadata?: Record<string, unknown>;
}

export interface NodeUpdatedEvent extends BaseEvent {
  type: 'node:updated';
  namespace: 'lifecycle';
  nodeId: string;
  updateType: 'content' | 'hierarchy' | 'status' | 'metadata' | 'nodeType';
  previousValue?: unknown;
  newValue?: unknown;
  metadata?: Record<string, unknown>;
}

export interface NodeDeletedEvent extends BaseEvent {
  type: 'node:deleted';
  namespace: 'lifecycle';
  nodeId: string;
  childrenTransferred?: string[];
  metadata?: Record<string, unknown>;
}

export interface HierarchyChangedEvent extends BaseEvent {
  type: 'hierarchy:changed';
  namespace: 'lifecycle';
  affectedNodes: string[];
  changeType: 'indent' | 'outdent' | 'move' | 'expand' | 'collapse' | 'create' | 'delete';
  metadata?: Record<string, unknown>;
}

export interface ConflictResolvedEvent extends BaseEvent {
  type: 'node:conflict-resolved';
  namespace: 'lifecycle';
  nodeId: string;
  conflictType: 'concurrent-edit' | 'version-mismatch' | 'deleted-node';
  strategy: 'last-write-wins' | 'field-merge' | 'manual' | 'operational-transform';
  discardedUpdate?: unknown;
  metadata?: Record<string, unknown>;
}

export interface UpdateRolledBackEvent extends BaseEvent {
  type: 'node:update-rolled-back';
  namespace: 'lifecycle';
  nodeId: string;
  reason: string;
  failedUpdate: unknown;
  metadata?: Record<string, unknown>;
}

// ============================================================================
// Phase 2+ Preparation Events (Backlinking, AI, Collaboration)
// ============================================================================

export interface BacklinkDetectedEvent extends BaseEvent {
  type: 'backlink:detected';
  namespace: 'phase2';
  sourceNodeId: string;
  targetNodeId: string;
  linkType: string;
  linkText: string;
  metadata?: Record<string, unknown>;
}

export interface AISuggestionEvent extends BaseEvent {
  type: 'ai:suggestion';
  namespace: 'phase2';
  nodeId: string;
  suggestionType: string;
  suggestion: unknown;
  confidence: number;
  metadata?: Record<string, unknown>;
}

export interface CollabNodeUpdatedEvent extends BaseEvent {
  type: 'collab:node-updated';
  namespace: 'phase2';
  nodeId: string;
  userId: string;
  updateType: string;
  conflictResolution?: string;
  metadata?: Record<string, unknown>;
}

// ============================================================================
// Focus and Navigation Events
// ============================================================================

export interface FocusRequestedEvent extends BaseEvent {
  type: 'focus:requested';
  namespace: 'navigation';
  nodeId: string;
  position?: number;
  reason: string;
  metadata?: Record<string, unknown>;
}

export interface NavigationEvent extends BaseEvent {
  type: 'navigation:changed';
  namespace: 'navigation';
  fromNodeId?: string;
  toNodeId: string;
  navigationType: 'click' | 'keyboard' | 'programmatic';
  metadata?: Record<string, unknown>;
}

// ============================================================================
// Performance and Debug Events
// ============================================================================

export interface PerformanceMetricEvent extends BaseEvent {
  type: 'performance:metric';
  namespace: 'debug';
  metricName: string;
  value: number;
  unit: string;
  metadata?: Record<string, unknown>;
}

export interface DebugEvent extends BaseEvent {
  type: 'debug:log';
  namespace: 'debug';
  level: 'info' | 'warn' | 'error' | 'debug';
  message: string;
  metadata?: Record<string, unknown>;
}

// ============================================================================
// Backend/Processing Events (for reactive services)
// ============================================================================

export interface NodeReferenceUpdateEvent extends BaseEvent {
  type: 'node:reference-update-needed';
  namespace: 'content';
  nodeId: string;
  content: string;
  metadata?: Record<string, unknown>;
}

export interface NodePersistenceEvent extends BaseEvent {
  type: 'node:persistence-needed';
  namespace: 'backend';
  nodeId: string;
  content: string;
  metadata?: Record<string, unknown>;
}

export interface NodeEmbeddingEvent extends BaseEvent {
  type: 'node:embedding-needed';
  namespace: 'ai';
  nodeId: string;
  content: string;
  metadata?: Record<string, unknown>;
}

export interface NodeReferencePropagationEvent extends BaseEvent {
  type: 'node:reference-propagation-needed';
  namespace: 'references';
  nodeId: string;
  content: string;
  metadata?: Record<string, unknown>;
}

// ============================================================================
// Event Union Type
// ============================================================================

// Error Events
export interface PersistenceFailedEvent extends BaseEvent {
  type: 'error:persistence-failed';
  namespace: 'error';
  message: string;
  failedNodeIds: string[];
  failureReason: 'timeout' | 'foreign-key-constraint' | 'database-locked' | 'unknown';
  canRetry: boolean; // Whether retry might succeed
  affectedOperations: Array<{
    nodeId: string;
    operation: 'create' | 'update' | 'delete';
    error?: string;
  }>;
  metadata?: Record<string, unknown>;
}

export type NodeSpaceEvent =
  // Dynamic State Coordination
  | NodeStatusChangedEvent
  | UserStatusChangedEvent
  // Interactive Decoration Events
  | DecorationClickedEvent
  | DecorationUpdateNeededEvent
  | DecorationHoverEvent
  // Cache & Reference Coordination
  | CacheInvalidateEvent
  | ReferencesUpdateNeededEvent
  | ReferenceResolutionEvent
  // Node Lifecycle Events
  | NodeCreatedEvent
  | NodeUpdatedEvent
  | NodeDeletedEvent
  | HierarchyChangedEvent
  | ConflictResolvedEvent
  | UpdateRolledBackEvent
  // Backend/Processing Events
  | NodeReferenceUpdateEvent
  | NodePersistenceEvent
  | NodeEmbeddingEvent
  | NodeReferencePropagationEvent
  // Phase 2+ Events
  | BacklinkDetectedEvent
  | AISuggestionEvent
  | CollabNodeUpdatedEvent
  // Focus and Navigation
  | FocusRequestedEvent
  | NavigationEvent
  // Performance and Debug
  | PerformanceMetricEvent
  | DebugEvent
  // Error Events
  | PersistenceFailedEvent;

// ============================================================================
// Event Handler Types
// ============================================================================

export type EventHandler<T extends NodeSpaceEvent = NodeSpaceEvent> = (
  event: T
) => void | Promise<void>;

export type EventHandlerMap = {
  [K in NodeSpaceEvent['type']]: EventHandler<Extract<NodeSpaceEvent, { type: K }>>;
};

// ============================================================================
// Event Filter Types
// ============================================================================

export interface EventFilter {
  type?: string | string[];
  namespace?: string | string[];
  source?: string | string[];
  nodeId?: string;
  userId?: string;
}

export interface EventSubscriptionOptions {
  filter?: EventFilter;
  priority?: number;
  debounceMs?: number;
  once?: boolean;
}

// ============================================================================
// Event Batching Types
// ============================================================================

export interface BatchedEvent {
  events: NodeSpaceEvent[];
  batchId: string;
  batchSize: number;
  timeWindow: number;
}

export interface BatchingConfig {
  maxBatchSize: number;
  timeWindowMs: number;
  enableForTypes: string[];
}

// ============================================================================
// Namespace Definitions
// ============================================================================

export const EventNamespaces = {
  COORDINATION: 'coordination',
  INTERACTION: 'interaction',
  LIFECYCLE: 'lifecycle',
  NAVIGATION: 'navigation',
  PHASE2: 'phase2',
  DEBUG: 'debug'
} as const;

// ============================================================================
// Event Type Constants
// ============================================================================

export const EventTypes = {
  // Dynamic State Coordination
  NODE_STATUS_CHANGED: 'node:status-changed',
  USER_STATUS_CHANGED: 'user:status-changed',

  // Interactive Decoration Events
  DECORATION_CLICKED: 'decoration:clicked',
  DECORATION_UPDATE_NEEDED: 'decoration:update-needed',
  DECORATION_HOVER: 'decoration:hover',

  // Cache & Reference Coordination
  CACHE_INVALIDATE: 'cache:invalidate',
  REFERENCES_UPDATE_NEEDED: 'references:update-needed',
  REFERENCE_RESOLVED: 'reference:resolved',

  // Node Lifecycle Events
  NODE_CREATED: 'node:created',
  NODE_UPDATED: 'node:updated',
  NODE_DELETED: 'node:deleted',
  HIERARCHY_CHANGED: 'hierarchy:changed',
  CONFLICT_RESOLVED: 'node:conflict-resolved',
  UPDATE_ROLLED_BACK: 'node:update-rolled-back',

  // Phase 2+ Events
  BACKLINK_DETECTED: 'backlink:detected',
  AI_SUGGESTION: 'ai:suggestion',
  COLLAB_NODE_UPDATED: 'collab:node-updated',

  // Focus and Navigation
  FOCUS_REQUESTED: 'focus:requested',
  NAVIGATION_CHANGED: 'navigation:changed',

  // Performance and Debug
  PERFORMANCE_METRIC: 'performance:metric',
  DEBUG_LOG: 'debug:log'
} as const;
