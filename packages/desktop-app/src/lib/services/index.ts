/**
 * NodeSpace Services - Centralized Export Hub
 *
 * Provides organized exports for all NodeSpace services in logical groups.
 * This makes it easy to import services throughout the application.
 */

// ============================================================================
// Core Database and Storage
// ============================================================================
export * from './MockDatabaseService';

// ============================================================================
// Node Management
// ============================================================================
export * from './NodeManager';
export * from './EnhancedNodeManager';
export * from './NodeOperationsService';
export * from './HierarchyService';

// ============================================================================
// Event System and Coordination
// ============================================================================
export * from './EventBus';
export * from './EventTypes';
export * from './CacheCoordinator';
export * from './DecorationCoordinator';

// ============================================================================
// Node Reference System - Phase 2.1 & 2.2
// ============================================================================
export * from './NodeReferenceService';
export * from './BaseNodeDecoration';

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
export * from './PerformanceMonitor';

// ============================================================================
// Legacy and Utilities
// ============================================================================
export * from './mockTextService';
