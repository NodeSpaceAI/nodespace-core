/**
 * Type Guards for EventBus Event Types
 *
 * Provides runtime type validation for event assertions in tests.
 * Safer than type assertions as they validate structure at runtime.
 */

import type {
  NodeSpaceEvent,
  NodeCreatedEvent,
  NodeUpdatedEvent,
  NodeDeletedEvent,
  NodeStatusChangedEvent,
  CacheInvalidateEvent,
  DecorationClickedEvent,
  DecorationUpdateNeededEvent,
  DecorationHoverEvent,
  HierarchyChangedEvent,
  ReferencesUpdateNeededEvent
} from '../../lib/services/event-types';

export function isNodeCreatedEvent(event: NodeSpaceEvent): event is NodeCreatedEvent {
  return (
    event.type === 'node:created' &&
    'nodeId' in event &&
    'nodeType' in event &&
    event.namespace === 'lifecycle'
  );
}

export function isNodeUpdatedEvent(event: NodeSpaceEvent): event is NodeUpdatedEvent {
  return (
    event.type === 'node:updated' &&
    'nodeId' in event &&
    'updateType' in event &&
    event.namespace === 'lifecycle'
  );
}

export function isNodeDeletedEvent(event: NodeSpaceEvent): event is NodeDeletedEvent {
  return event.type === 'node:deleted' && 'nodeId' in event && event.namespace === 'lifecycle';
}

export function isNodeStatusChangedEvent(event: NodeSpaceEvent): event is NodeStatusChangedEvent {
  return (
    event.type === 'node:status-changed' &&
    'nodeId' in event &&
    'status' in event &&
    event.namespace === 'coordination'
  );
}

export function isCacheInvalidateEvent(event: NodeSpaceEvent): event is CacheInvalidateEvent {
  return (
    event.type === 'cache:invalidate' &&
    'cacheKey' in event &&
    'scope' in event &&
    'reason' in event &&
    event.namespace === 'coordination'
  );
}

export function isDecorationClickedEvent(event: NodeSpaceEvent): event is DecorationClickedEvent {
  return (
    event.type === 'decoration:clicked' &&
    'nodeId' in event &&
    'decorationType' in event &&
    'target' in event &&
    event.namespace === 'interaction'
  );
}

export function isDecorationUpdateNeededEvent(
  event: NodeSpaceEvent
): event is DecorationUpdateNeededEvent {
  return (
    event.type === 'decoration:update-needed' &&
    'nodeId' in event &&
    'decorationType' in event &&
    'reason' in event &&
    event.namespace === 'interaction'
  );
}

export function isDecorationHoverEvent(event: NodeSpaceEvent): event is DecorationHoverEvent {
  return (
    event.type === 'decoration:hover' &&
    'nodeId' in event &&
    'decorationType' in event &&
    'target' in event &&
    'hoverState' in event &&
    event.namespace === 'interaction'
  );
}

export function isHierarchyChangedEvent(event: NodeSpaceEvent): event is HierarchyChangedEvent {
  return (
    event.type === 'hierarchy:changed' &&
    'affectedNodes' in event &&
    'changeType' in event &&
    event.namespace === 'lifecycle'
  );
}

export function isReferencesUpdateNeededEvent(
  event: NodeSpaceEvent
): event is ReferencesUpdateNeededEvent {
  return (
    event.type === 'references:update-needed' &&
    'nodeId' in event &&
    'updateType' in event &&
    event.namespace === 'coordination'
  );
}
