//! Property access helpers for namespaced node properties.
//!
//! Node properties may be stored flat (`properties.key`) or namespaced
//! under the node type (`properties.skill.key`, `properties.prompt.key`).
//! These helpers check both locations transparently.

use serde_json::Value;

/// Get a property value, checking both namespaced and flat locations.
///
/// Properties are stored namespaced under the node type key after normalization
/// (`properties.{namespace}.key`). This helper checks the namespace first so
/// that MCP updates — which always normalize to namespaced format — take effect
/// even when an older flat value exists from a pre-normalization database.
///
/// # Arguments
/// * `properties` - The node's properties JSON value
/// * `namespace` - The node type namespace (e.g., "skill", "prompt")
/// * `key` - The property key to look up
pub fn get_prop<'a>(properties: &'a Value, namespace: &str, key: &str) -> Option<&'a Value> {
    properties
        .get(namespace)
        .and_then(|ns| ns.get(key))
        .or_else(|| properties.get(key))
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
    fn get_prop_namespace_takes_precedence() {
        // Namespaced value wins: MCP updates normalize to namespace, so they must
        // override any stale flat value left from a pre-normalization database.
        let props = json!({"description": "flat", "skill": {"description": "namespaced"}});
        assert_eq!(
            get_prop_str(&props, "skill", "description"),
            Some("namespaced")
        );
    }

    #[test]
    fn get_prop_missing() {
        let props = json!({"other": "value"});
        assert_eq!(get_prop_str(&props, "skill", "description"), None);
    }

    #[test]
    fn get_prop_namespace_overrides_stale_flat() {
        // Regression test for #1080: skill seeded with flat {"max_iterations": 2},
        // then MCP update normalizes to {"skill": {"max_iterations": 4}}.
        // Both coexist until the node is re-seeded; namespace must win.
        let props = json!({"max_iterations": 2, "skill": {"max_iterations": 4}});
        assert_eq!(
            get_prop(&props, "skill", "max_iterations").and_then(|v| v.as_u64()),
            Some(4)
        );
    }
}
