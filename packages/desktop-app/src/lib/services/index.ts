/**
 * NodeSpace Services - Centralized Export Hub
 *
 * Provides organized exports for all NodeSpace services in logical groups.
 * This makes it easy to import services throughout the application.
 */

// ============================================================================
// Node Management
// ============================================================================
export * from './reactiveNodeService.svelte.js';
export * from './nodeOperationsService';
export * from './hierarchyService';

// ============================================================================
// Event System and Coordination
// ============================================================================
export * from './eventBus';
export * from './eventTypes';
export * from './cacheCoordinator';
export * from './decorationCoordinator';

// ============================================================================
// Node Reference System - Phase 2.1 & 2.2
// ============================================================================
export * from './nodeReferenceService';
export * from './baseNodeDecoration';

// ============================================================================
// Content Processing
// ============================================================================
export * from './contentProcessor';
export * from './markdownPatternDetector';
export * from './markdownUtils';
export * from './wysiwygProcessor';

// ============================================================================
// Performance and Monitoring
// ============================================================================
export * from './performanceMonitor';
