//! Node Behavior System
//!
//! This module provides the trait-based behavior system for different node types:
//!
//! - `NodeBehavior` trait - Defines type-specific validation and processing
//! - Built-in behaviors (TextNodeBehavior, TaskNodeBehavior, etc.)
//! - `NodeBehaviorRegistry` - Dynamic behavior lookup and registration
//!
//! The behavior system enables extensibility while maintaining type safety
//! and consistent validation across all node operations.
