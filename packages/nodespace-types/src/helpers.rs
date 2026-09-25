/// The complete set of supported node lifecycle states.
///
/// Local deletion is a hard delete, so there is deliberately no `"deleted"`
/// state: a node is either live (`"active"`) or hidden from default views and
/// search (`"archived"`). Persisting any other value would let hidden nodes leak
/// back into full-text and semantic search, so writes are validated against this
/// set at the storage boundary.
pub const LIFECYCLE_STATUSES: [&str; 2] = ["active", "archived"];

pub(crate) fn default_lifecycle_status() -> String {
    "active".to_string()
}

pub(crate) fn default_version() -> i64 {
    1
}

pub(crate) fn is_active_lifecycle(s: &str) -> bool {
    s == "active"
}

/// Returns `true` if `status` is one of the [`LIFECYCLE_STATUSES`].
pub fn is_valid_lifecycle_status(status: &str) -> bool {
    LIFECYCLE_STATUSES.contains(&status)
}

/// Deserialize a tri-state `Option<Option<String>>` field: absent → `None`
/// (via `#[serde(default)]`), `null` → `Some(None)` (clear), a string →
/// `Some(Some(s))` (set). A plain `Option<Option<T>>` collapses `null` into
/// `None`, which would turn every "clear this field" into a no-op.
pub(crate) fn deserialize_clearable<'de, D>(d: D) -> Result<Option<Option<String>>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    use serde::Deserialize;
    Option::<String>::deserialize(d).map(Some)
}
