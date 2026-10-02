//! Prints the core node type registry and the seeded core schemas as JSON, for
//! tooling that compares them with something outside the Rust workspace.
//!
//! `types` is each [`CoreNodeType`]'s id and kind, in registry order. `schemas` is
//! [`get_core_schemas`] as it is seeded: each schema's own fields, not the
//! ones it inherits through `extends`.
//!
//! Usage: `cargo run -q -p nodespace-core --example dump_core_schemas`

use nodespace_core::models::core_schemas::get_core_schemas;
use nodespace_types::CoreNodeType;
use serde_json::json;

fn main() {
    let types: Vec<_> = CoreNodeType::ALL
        .into_iter()
        .map(|t| json!({ "id": t.as_str(), "kind": t.kind() }))
        .collect();
    let dump = json!({ "types": types, "schemas": get_core_schemas() });
    println!(
        "{}",
        serde_json::to_string_pretty(&dump).expect("the registry and schemas serialize")
    );
}
