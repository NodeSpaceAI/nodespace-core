//! Playbook Engine Module
//!
//! The playbook engine evaluates playbook rules and executes graph operations.
//! It subscribes to the domain event broadcast channel, matches events against
//! active playbook triggers, evaluates CEL conditions, and executes actions.
//!
//! # Architecture
//!
//! - `PlaybookEngine`: Top-level struct, subscribes to events, manages lifecycle
//! - `PlaybookLifecycleManager`: Owns TriggerIndex, CronRegistry, ActivePlaybooks
//! - `TriggerKey` + `TriggerIndex`: O(1) event-to-rule matching
//!
//! # Implementation Phases
//!
//! - Phase 0: EventEnvelope + enriched NodeUpdated (done)
//! - Phase 1: Lifecycle Manager + Rule Matching (this module)
//! - Phase 2+: ExecutionQueue, CEL Evaluator, Action Executor, CronRunner

pub mod engine;
pub mod lifecycle;
#[cfg(test)]
mod tests;
pub mod types;

pub use engine::PlaybookEngine;
pub use lifecycle::PlaybookLifecycleManager;
pub use types::*;
