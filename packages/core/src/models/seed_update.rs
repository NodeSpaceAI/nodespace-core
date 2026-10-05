//! A shipped change to a seeded node that the user has to decide on
//! (ADR-094 §8).
//!
//! Seed reconciliation never replaces an aspect the user edited (ADR-072).
//! When the shipped version of such an aspect changes, reconciliation records
//! it here instead, and the user keeps their own or takes the shipped one.
//!
//! The record is local bookkeeping in its own table, not a node and not a
//! property of one: nothing that reads nodes (search, queries, an agent's
//! tools) can see it.

use chrono::{DateTime, Utc};
use std::str::FromStr;

/// The parts of a seeded node that are reconciled, and edited, independently
/// (ADR-072).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SeedAspect {
    /// The root: its name and its fields.
    Config,
    /// The body: the Markdown children beneath the root.
    Guidance,
    /// A core schema's context paths (ADR-094 §2): the one part of a core
    /// schema that ships with a value a user may change.
    ContextPaths,
}

impl SeedAspect {
    pub fn as_str(&self) -> &'static str {
        match self {
            SeedAspect::Config => "config",
            SeedAspect::Guidance => "guidance",
            SeedAspect::ContextPaths => "context_paths",
        }
    }

    /// The `_seed` key holding the shipped version this aspect was last
    /// brought up to date with, or kept against.
    pub fn version_key(&self) -> &'static str {
        match self {
            SeedAspect::Config => "config_version",
            SeedAspect::Guidance => "guidance_version",
            SeedAspect::ContextPaths => "context_paths_version",
        }
    }

    /// The `_seed` key set once the user edits this aspect.
    pub fn modified_key(&self) -> &'static str {
        match self {
            SeedAspect::Config => "config_modified",
            SeedAspect::Guidance => "guidance_modified",
            SeedAspect::ContextPaths => "context_paths_modified",
        }
    }
}

impl FromStr for SeedAspect {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "config" => Ok(SeedAspect::Config),
            "guidance" => Ok(SeedAspect::Guidance),
            "context_paths" => Ok(SeedAspect::ContextPaths),
            other => Err(format!(
                "unknown seed aspect '{other}' (expected 'config', 'guidance' or 'context_paths')"
            )),
        }
    }
}

impl std::fmt::Display for SeedAspect {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// One row of the pending table: the aspect of a seeded node whose shipped
/// version changed while the user's edit was kept.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingSeedUpdateRow {
    pub node_id: String,
    pub aspect: SeedAspect,
    /// Fingerprint of the shipped version that was held back.
    pub shipped_version: String,
    pub recorded_at: DateTime<Utc>,
}

/// A pending update as shown to the user: the row, with what names the node.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingSeedUpdate {
    pub node_id: String,
    /// The seeded node's type: the kind of item (`skill`, `play`, `query`, …).
    pub node_type: String,
    pub title: String,
    pub aspect: SeedAspect,
    pub shipped_version: String,
    pub recorded_at: DateTime<Utc>,
    /// When the user's version of this aspect was last written. For display
    /// only: whether an aspect is user-modified comes from its `_seed` flag.
    pub last_edited_at: DateTime<Utc>,
}
