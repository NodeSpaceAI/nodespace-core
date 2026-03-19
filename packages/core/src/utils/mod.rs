//! Utility functions for NodeSpace Core
//!
//! This module provides common utility functions used across the codebase.

mod markdown;

pub use markdown::{
    interpolate_title_template, interpolate_title_template_with_schema, strip_markdown,
};
