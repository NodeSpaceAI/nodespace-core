//! Type-Safe DateNode Wrapper
//!
//! Provides ergonomic, type-safe access to date node properties while maintaining
//! the universal Node storage model. Date nodes use deterministic IDs (YYYY-MM-DD format).
//!
//! # Examples
//!
//! ```rust
//! use nodespace_core::models::{Node, DateNode};
//! use chrono::NaiveDate;
//! use serde_json::json;
//!
//! // Create from existing node
//! let node = Node::new_with_id(
//!     "2025-01-15".to_string(),
//!     "date".to_string(),
//!     "2025-01-15".to_string(),
//!     None,
//!     json!({"timezone": "UTC"}),
//! );
//! let date = DateNode::from_node(node).unwrap();
//!
//! // Create for specific date
//! let date = DateNode::for_date(NaiveDate::from_ymd_opt(2025, 1, 15).unwrap()).build();
//! ```

use crate::models::{Node, ValidationError};
use chrono::NaiveDate;
use serde_json::json;

/// Type-safe wrapper for date nodes
///
/// Date nodes represent calendar dates with deterministic IDs in YYYY-MM-DD format.
/// They use the date string as both ID and content.
///
/// # Examples
///
/// ```rust
/// use nodespace_core::models::{Node, DateNode};
/// use chrono::NaiveDate;
/// use serde_json::json;
///
/// let node = Node::new_with_id(
///     "2025-01-15".to_string(),
///     "date".to_string(),
///     "2025-01-15".to_string(),
///     None,
///     json!({}),
/// );
/// let date = DateNode::from_node(node).unwrap();
/// assert_eq!(date.date().unwrap(), NaiveDate::from_ymd_opt(2025, 1, 15).unwrap());
/// ```
#[derive(Debug, Clone)]
pub struct DateNode {
    node: Node,
}

impl DateNode {
    /// Create a DateNode from an existing Node
    ///
    /// # Errors
    ///
    /// Returns `ValidationError::InvalidNodeType` if the node type is not "date".
    /// Returns `ValidationError::InvalidId` if the ID is not in YYYY-MM-DD format.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use nodespace_core::models::{Node, DateNode};
    /// use serde_json::json;
    ///
    /// let node = Node::new_with_id(
    ///     "2025-01-15".to_string(),
    ///     "date".to_string(),
    ///     "2025-01-15".to_string(),
    ///     None,
    ///     json!({}),
    /// );
    /// let date = DateNode::from_node(node).unwrap();
    /// ```
    pub fn from_node(node: Node) -> Result<Self, ValidationError> {
        if node.node_type != "date" {
            return Err(ValidationError::InvalidNodeType(format!(
                "Expected 'date', got '{}'",
                node.node_type
            )));
        }

        // Validate ID format (YYYY-MM-DD)
        if !Self::is_valid_date_id(&node.id) {
            return Err(ValidationError::InvalidId(format!(
                "Date node ID must be in YYYY-MM-DD format, got '{}'",
                node.id
            )));
        }

        Ok(Self { node })
    }

    /// Create a new DateNode for the given date
    ///
    /// Returns a builder for setting additional properties.
    ///
    /// # Note on Builder API Design
    ///
    /// DateNode uses `for_date()` instead of the standard `builder()` pattern used by
    /// TaskNode and TextNode. This is intentional because:
    ///
    /// - Date nodes have deterministic IDs derived from the date itself (YYYY-MM-DD format)
    /// - The date is the primary identifier and content, not a property like in other nodes
    /// - `DateNode::for_date(date)` reads more naturally in domain language than
    ///   `DateNode::builder(date_string)`
    /// - Emphasizes that date nodes are special: one node per date, with deterministic IDs
    ///
    /// # Examples
    ///
    /// ```rust
    /// use nodespace_core::models::DateNode;
    /// use chrono::NaiveDate;
    ///
    /// let date = DateNode::for_date(NaiveDate::from_ymd_opt(2025, 1, 15).unwrap())
    ///     .with_timezone("America/New_York".to_string())
    ///     .build();
    /// ```
    pub fn for_date(date: NaiveDate) -> DateNodeBuilder {
        DateNodeBuilder {
            date,
            timezone: None,
            is_holiday: None,
        }
    }

    /// Validate if a string is a valid date ID (YYYY-MM-DD format)
    fn is_valid_date_id(id: &str) -> bool {
        // Check format and parse
        NaiveDate::parse_from_str(id, "%Y-%m-%d").is_ok()
    }

    /// Get the date value
    ///
    /// Parses the node ID as a date. Should always succeed if the node was
    /// properly validated on creation.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use nodespace_core::models::{Node, DateNode};
    /// use chrono::NaiveDate;
    /// use serde_json::json;
    ///
    /// let node = Node::new_with_id(
    ///     "2025-01-15".to_string(),
    ///     "date".to_string(),
    ///     "2025-01-15".to_string(),
    ///     None,
    ///     json!({}),
    /// );
    /// let date = DateNode::from_node(node).unwrap();
    /// assert_eq!(date.date().unwrap(), NaiveDate::from_ymd_opt(2025, 1, 15).unwrap());
    /// ```
    pub fn date(&self) -> Result<NaiveDate, String> {
        NaiveDate::parse_from_str(&self.node.id, "%Y-%m-%d")
            .map_err(|e| format!("Failed to parse date from ID '{}': {}", self.node.id, e))
    }

    /// Get the timezone property
    ///
    /// Returns `None` if no timezone is set.
    pub fn timezone(&self) -> Option<String> {
        self.node
            .properties
            .get("timezone")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
    }

    /// Set the timezone
    ///
    /// Pass `None` to clear the timezone.
    pub fn set_timezone(&mut self, timezone: Option<String>) {
        if let Some(obj) = self.node.properties.as_object_mut() {
            if let Some(tz) = timezone {
                obj.insert("timezone".to_string(), json!(tz));
            } else {
                obj.remove("timezone");
            }
        }
    }

    /// Get the is_holiday flag
    ///
    /// Returns `false` if not set.
    pub fn is_holiday(&self) -> bool {
        self.node
            .properties
            .get("is_holiday")
            .and_then(|v| v.as_bool())
            .unwrap_or(false)
    }

    /// Set the is_holiday flag
    pub fn set_is_holiday(&mut self, is_holiday: bool) {
        if let Some(obj) = self.node.properties.as_object_mut() {
            obj.insert("is_holiday".to_string(), json!(is_holiday));
        }
    }

    /// Get a reference to the underlying Node
    pub fn as_node(&self) -> &Node {
        &self.node
    }

    /// Get a mutable reference to the underlying Node
    pub fn as_node_mut(&mut self) -> &mut Node {
        &mut self.node
    }

    /// Convert back to universal Node (consumes wrapper)
    pub fn into_node(self) -> Node {
        self.node
    }
}

/// Builder for creating new DateNode instances
pub struct DateNodeBuilder {
    date: NaiveDate,
    timezone: Option<String>,
    is_holiday: Option<bool>,
}

impl DateNodeBuilder {
    /// Set the timezone
    pub fn with_timezone(mut self, timezone: String) -> Self {
        self.timezone = Some(timezone);
        self
    }

    /// Set the is_holiday flag
    pub fn with_is_holiday(mut self, is_holiday: bool) -> Self {
        self.is_holiday = Some(is_holiday);
        self
    }

    /// Build the DateNode
    pub fn build(self) -> DateNode {
        let date_str = self.date.format("%Y-%m-%d").to_string();
        let mut properties = serde_json::Map::new();

        // Set optional fields
        if let Some(timezone) = self.timezone {
            properties.insert("timezone".to_string(), json!(timezone));
        }

        if let Some(is_holiday) = self.is_holiday {
            properties.insert("is_holiday".to_string(), json!(is_holiday));
        }

        let node = Node::new_with_id(
            date_str.clone(),
            "date".to_string(),
            date_str, // Content is also the date string
            json!(properties),
        );

        DateNode { node }
    }
}
