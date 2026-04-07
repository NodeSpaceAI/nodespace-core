//! Property access helpers for namespaced node properties.
//!
//! Node properties may be stored flat (`properties.key`) or namespaced
//! under the node type (`properties.skill.key`, `properties.prompt.key`).
//! These helpers check both locations transparently.

use serde_json::Value;

/// Get a property value, checking both flat and namespaced locations.
///
/// Properties created via `create_node_with_parent` are namespaced under
/// the node type key. This helper checks `properties.key` first, then
/// falls back to `properties.{namespace}.key`.
///
/// # Arguments
/// * `properties` - The node's properties JSON value
/// * `namespace` - The node type namespace (e.g., "skill", "prompt")
/// * `key` - The property key to look up
pub fn get_prop<'a>(properties: &'a Value, namespace: &str, key: &str) -> Option<&'a Value> {
    properties
        .get(key)
        .or_else(|| properties.get(namespace).and_then(|ns| ns.get(key)))
}

/// Get a string property, checking both flat and namespaced locations.
pub fn get_prop_str<'a>(properties: &'a Value, namespace: &str, key: &str) -> Option<&'a str> {
    get_prop(properties, namespace, key).and_then(|v| v.as_str())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn get_prop_flat() {
        let props = json!({"description": "hello"});
        assert_eq!(get_prop_str(&props, "skill", "description"), Some("hello"));
    }

    #[test]
    fn get_prop_namespaced() {
        let props = json!({"skill": {"description": "hello"}});
        assert_eq!(get_prop_str(&props, "skill", "description"), Some("hello"));
    }

    #[test]
    fn get_prop_flat_takes_precedence() {
        let props = json!({"description": "flat", "skill": {"description": "namespaced"}});
        assert_eq!(get_prop_str(&props, "skill", "description"), Some("flat"));
    }

    #[test]
    fn get_prop_missing() {
        let props = json!({"other": "value"});
        assert_eq!(get_prop_str(&props, "skill", "description"), None);
    }
}
