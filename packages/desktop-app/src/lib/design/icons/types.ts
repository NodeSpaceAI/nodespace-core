/**
 * Icon component types
 * 
 * This file contains both legacy types for backward compatibility
 * and imports the new semantic types from the registry system.
 */

// Legacy icon name type for backward compatibility
export type IconName = 'text' | 'circle' | 'circleRing' | 'chevronRight' | 'taskComplete' | 'taskIncomplete' | 'taskInProgress' | 'aiSquare';

// Re-export new semantic types from registry
export type { NodeType, NodeState, NodeIconProps, IconConfig } from './registry.js';
