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
//! - Phase 1: Lifecycle Manager + Rule Matching (done)
//! - Phase 2: ExecutionQueue + RuleProcessor (done)
//! - Phase 3: CEL Evaluator — property-level condition evaluation (done)
//! - Phase 4: Action Executor — binding context, sequential execution (done)
//! - Phase 5: CronRunner — 60-second polling loop (done)
//! - Phase 6: Cycle Detection + Log Node Deduplication (done)
//! - Phase 7: Save-Time Validation (done)

pub mod actions;
pub mod cel;
pub mod cron_runner;
pub mod engine;
pub mod lifecycle;
pub mod logging;
#[cfg(test)]
mod tests;
pub mod types;
pub mod validation;

pub use engine::PlaybookEngine;
pub use lifecycle::PlaybookLifecycleManager;
pub use types::*;
