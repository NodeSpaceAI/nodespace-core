//! Agent subsystem: local inference, ACP transport, tool execution.
//!
//! This crate contains the business logic for the agent layer, decoupled
//! from Tauri. The desktop-app crate provides thin Tauri command bindings
//! that delegate to types defined here.

// Shared types, traits, and interface contracts for agent subsystems
pub mod agent_types;
pub use agent_types::*;

// Local agent subsystem: model management, inference, tool execution
pub mod local_agent;

// Prompt assembly: hardcoded base + graph-stored overrides
pub mod prompt_assembler;

// Intent extraction: pattern matching + filler stripping for skill discovery
pub mod intent;

// Pre-turn skill discovery pipeline: intent → semantic search → threshold
pub mod skill_pipeline;

// ACP (Agent Communication Protocol) subsystem
pub mod acp;
