//! Agent subsystem: local inference, PTY agent catalog, tool execution.
//!
//! This crate contains the business logic for the agent layer, decoupled
//! from Tauri. The desktop-app crate provides thin Tauri command bindings
//! that delegate to types defined here.

// Shared types, traits, and interface contracts for agent subsystems
pub mod agent_types;
pub use agent_types::*;

// Local agent subsystem: model management, inference, tool execution
pub mod local_agent;

// Shared agent guidance rules: single source of truth for tool strategy,
// schema creation, and node reference guidance. Consumed by
// `prompt_assembler` (local agent) and by ADR-032 context-file
// writers in `agent_catalog`.
pub mod agent_guidance;

// Prompt assembly: hardcoded base + graph-stored overrides
pub mod prompt_assembler;

// Skill seeding templates: default skill nodes inserted on first run.
// Skill discovery itself is LLM-orchestrated via the `search_skills` tool
// there is no longer a pre-turn intent pipeline.
pub mod skill_pipeline;

// Shared source of truth for core-type rules referenced by both
// seed_skill_nodes() and the generated sections of packages/skill/SKILL.md
// (packages/cli/examples/gen_skill_md.rs). See module docs for the drift
// this prevents.
pub mod skill_rules;

// Golden-prompt tuning: case files as data, run against the real inference
// path with no NodeSpace plumbing in between. Driven by the `golden_runner`
// bin, which stays outside `cargo test` because it loads a ~5GB GGUF.
pub mod golden;

// PTY agent catalog: hardcoded catalog of external agent CLIs and their
// context-file assembly (ADR-032)
pub mod agent_catalog;

// PTY-based agent session engine (ADR-032)
pub mod pty;
