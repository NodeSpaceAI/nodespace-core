//! CEL Path Extractor
//!
//! Walks `cel-parser` AST to extract dot-path references from compiled CEL programs.
//! This is the shared foundation for three features:
//! - **Runtime resolution**: GraphResolver pre-fetches paths before CEL evaluation
//! - **Save-time validation**: paths are validated against the schema graph
//! - **Schema drift detection**: paths are re-validated when schemas change
//!
//! # Path Types
//!
//! - **Flat paths**: `node.story.epic.status` → `["node", "story", "epic", "status"]`
//! - **Collection paths**: `node.tasks.any(t, t.status == "done")` →
//!   collection `["node", "tasks"]`, item paths `["t", "status"]` with iter var `t`

use cel_parser::ast::{ComprehensionExpr, Expr, IdedExpr};

/// A dot-path extracted from a CEL expression.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ExtractedPath {
    /// Full path segments, e.g. `["node", "story", "epic", "status"]`
    pub segments: Vec<String>,
    /// Root variable name (first segment), e.g. `"node"`
    pub root: String,
}

/// A collection path extracted from a CEL comprehension (macro like .any/.all/.where).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CollectionPath {
    /// Path to the collection itself, e.g. `["node", "tasks"]`
    pub collection: ExtractedPath,
    /// The iteration variable name (e.g. `"t"` in `node.tasks.any(t, t.status == "done")`)
    pub iter_var: String,
    /// Paths referenced inside the comprehension body, rooted at the iter var
    pub item_paths: Vec<ExtractedPath>,
}

/// All paths extracted from a CEL expression.
#[derive(Debug, Clone, Default)]
pub struct ExtractionResult {
    /// Simple dot-paths (not inside comprehensions)
    pub paths: Vec<ExtractedPath>,
    /// Collection paths from comprehension macros
    pub collections: Vec<CollectionPath>,
}

/// Extract all dot-path references from a CEL expression string.
///
/// Parses the expression using `cel-parser` and walks the AST to find
/// `Select` chains (dot-paths) and `Comprehension` nodes (collection macros).
pub fn extract_paths(expr: &str) -> Result<ExtractionResult, String> {
    let parsed = cel_parser::Parser::new()
        .parse(expr)
        .map_err(|e| format!("CEL parse error: {}", e))?;

    let mut result = ExtractionResult::default();
    // Comprehension iter vars that are in scope — paths rooted at these
    // are collected into the current CollectionPath, not into result.paths
    let active_iter_vars: Vec<String> = Vec::new();
    walk_expr(&parsed, &active_iter_vars, &mut result);
    Ok(result)
}

/// Recursively walk the AST, collecting paths.
fn walk_expr(expr: &IdedExpr, active_iter_vars: &[String], result: &mut ExtractionResult) {
    match &expr.expr {
        Expr::Select(_) => {
            // Try to collect a full Select chain into a path
            if let Some(path) = collect_select_chain(expr) {
                if active_iter_vars.contains(&path.root) {
                    // This path is rooted at an iteration variable — it belongs to
                    // a comprehension, but we're called from outside that context.
                    // This shouldn't normally happen since we handle comprehensions
                    // separately, but add it to flat paths as a fallback.
                    result.paths.push(path);
                } else {
                    result.paths.push(path);
                }
            }
        }
        Expr::Ident(name) => {
            // A bare identifier like `node` (no dot-path, just a variable reference).
            // Only interesting if it's a root variable (not an iter var).
            if !active_iter_vars.contains(name) {
                result.paths.push(ExtractedPath {
                    root: name.clone(),
                    segments: vec![name.clone()],
                });
            }
        }
        Expr::Call(call) => {
            // Walk the target (receiver) and all arguments
            if let Some(target) = &call.target {
                walk_expr(target, active_iter_vars, result);
            }
            for arg in &call.args {
                walk_expr(arg, active_iter_vars, result);
            }
        }
        Expr::Comprehension(comp) => {
            handle_comprehension(comp, active_iter_vars, result);
        }
        Expr::List(list) => {
            for elem in &list.elements {
                walk_expr(elem, active_iter_vars, result);
            }
        }
        Expr::Map(map) => {
            for entry in &map.entries {
                match &entry.expr {
                    cel_parser::ast::EntryExpr::MapEntry(me) => {
                        walk_expr(&me.key, active_iter_vars, result);
                        walk_expr(&me.value, active_iter_vars, result);
                    }
                    cel_parser::ast::EntryExpr::StructField(sf) => {
                        walk_expr(&sf.value, active_iter_vars, result);
                    }
                }
            }
        }
        Expr::Struct(s) => {
            for entry in &s.entries {
                match &entry.expr {
                    cel_parser::ast::EntryExpr::MapEntry(me) => {
                        walk_expr(&me.key, active_iter_vars, result);
                        walk_expr(&me.value, active_iter_vars, result);
                    }
                    cel_parser::ast::EntryExpr::StructField(sf) => {
                        walk_expr(&sf.value, active_iter_vars, result);
                    }
                }
            }
        }
        Expr::Literal(_) | Expr::Unspecified => {}
    }
}

/// Collect a chain of `Select` expressions into a single path.
///
/// `node.story.epic.status` is parsed as:
/// ```text
/// Select(Select(Select(Ident("node"), "story"), "epic"), "status")
/// ```
/// This function unwinds that recursion into `["node", "story", "epic", "status"]`.
fn collect_select_chain(expr: &IdedExpr) -> Option<ExtractedPath> {
    let mut segments = Vec::new();
    let mut current = expr;

    loop {
        match &current.expr {
            Expr::Select(sel) => {
                segments.push(sel.field.clone());
                current = &sel.operand;
            }
            Expr::Ident(name) => {
                segments.push(name.clone());
                segments.reverse();
                let root = segments[0].clone();
                return Some(ExtractedPath { segments, root });
            }
            _ => {
                // The chain doesn't end in an Ident — it's something like
                // `func().field` or `list[0].field`. We can't represent this
                // as a simple dot-path.
                return None;
            }
        }
    }
}

/// Handle a comprehension node (desugared from macros like `.any()`, `.all()`, `.exists()`).
///
/// CEL macros like `node.tasks.any(t, t.status == "done")` are desugared by cel-parser
/// into a `ComprehensionExpr` with:
/// - `iter_range`: the collection expression (`node.tasks`)
/// - `iter_var`: the iteration variable name (`t` or `@it0` for the shorthand form)
/// - `loop_cond` / `loop_step` / `result`: the comprehension body
fn handle_comprehension(
    comp: &ComprehensionExpr,
    active_iter_vars: &[String],
    result: &mut ExtractionResult,
) {
    // Extract the collection path from iter_range
    let collection_path = collect_select_chain(&comp.iter_range);

    // Build the set of iter vars that are in scope inside this comprehension
    let mut inner_vars = active_iter_vars.to_vec();
    inner_vars.push(comp.iter_var.clone());
    if let Some(ref v2) = comp.iter_var2 {
        inner_vars.push(v2.clone());
    }
    // Also add the accumulator variable so we don't treat it as a path
    inner_vars.push(comp.accu_var.clone());

    // Walk the comprehension body to find item paths
    let mut body_result = ExtractionResult::default();
    walk_expr(&comp.loop_cond, &inner_vars, &mut body_result);
    walk_expr(&comp.loop_step, &inner_vars, &mut body_result);
    walk_expr(&comp.result, &inner_vars, &mut body_result);
    walk_expr(&comp.accu_init, &inner_vars, &mut body_result);

    if let Some(coll_path) = collection_path {
        // Separate item paths (rooted at iter_var) from other paths
        let mut item_paths = Vec::new();
        let iter_var = &comp.iter_var;

        for path in &body_result.paths {
            if path.root == *iter_var {
                item_paths.push(path.clone());
            } else {
                // Path is not rooted at the iter var — it's a reference to an
                // outer variable (e.g., `node.threshold` inside `.any(t, t.value > node.threshold)`)
                result.paths.push(path.clone());
            }
        }

        // Propagate nested collections from the body
        result.collections.extend(body_result.collections);

        result.collections.push(CollectionPath {
            collection: coll_path,
            iter_var: iter_var.clone(),
            item_paths,
        });
    } else {
        // iter_range isn't a simple path (unusual) — just merge body paths
        walk_expr(&comp.iter_range, active_iter_vars, result);
        result.paths.extend(body_result.paths);
        result.collections.extend(body_result.collections);
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_simple_property_path() {
        let result = extract_paths("node.status == 'open'").unwrap();
        assert!(result
            .paths
            .iter()
            .any(|p| p.segments == vec!["node", "status"]));
    }

    #[test]
    fn extract_multi_hop_path() {
        let result = extract_paths("node.story.epic.status == 'active'").unwrap();
        assert!(result
            .paths
            .iter()
            .any(|p| p.segments == vec!["node", "story", "epic", "status"]));
    }

    #[test]
    fn extract_multiple_paths() {
        let result = extract_paths("node.status == 'open' && node.priority == 'high'").unwrap();
        assert!(result
            .paths
            .iter()
            .any(|p| p.segments == vec!["node", "status"]));
        assert!(result
            .paths
            .iter()
            .any(|p| p.segments == vec!["node", "priority"]));
    }

    #[test]
    fn extract_trigger_paths() {
        let result = extract_paths("trigger.property.new_value == 'done'").unwrap();
        assert!(result
            .paths
            .iter()
            .any(|p| p.segments == vec!["trigger", "property", "new_value"]));
    }

    #[test]
    fn extract_bare_identifier() {
        let result = extract_paths("node == true").unwrap();
        assert!(result.paths.iter().any(|p| p.segments == vec!["node"]));
    }

    #[test]
    fn extract_function_call_args() {
        let result = extract_paths("days_since(node.created_date) > 7").unwrap();
        assert!(result
            .paths
            .iter()
            .any(|p| p.segments == vec!["node", "created_date"]));
    }

    #[test]
    fn no_paths_in_literal_expression() {
        let result = extract_paths("1 + 2 == 3").unwrap();
        // No variable paths — only literals
        assert!(result.paths.is_empty());
        assert!(result.collections.is_empty());
    }

    #[test]
    fn extract_comprehension_collection_path() {
        // .exists() is a macro that creates a comprehension
        let result = extract_paths("node.tasks.exists(t, t.status == 'done')").unwrap();
        assert!(
            !result.collections.is_empty(),
            "should find collection path"
        );
        let coll = &result.collections[0];
        assert_eq!(coll.collection.segments, vec!["node", "tasks"]);
        assert_eq!(coll.iter_var, "t");
        assert!(coll
            .item_paths
            .iter()
            .any(|p| p.segments == vec!["t", "status"]));
    }

    #[test]
    fn extract_comprehension_with_outer_reference() {
        // References to outer variables inside a comprehension
        let result = extract_paths("node.items.exists(t, t.value > node.threshold)").unwrap();
        // node.items → collection
        assert!(result
            .collections
            .iter()
            .any(|c| c.collection.segments == vec!["node", "items"]));
        // node.threshold → outer path (not an item path)
        assert!(result
            .paths
            .iter()
            .any(|p| p.segments == vec!["node", "threshold"]));
    }

    #[test]
    fn extract_nested_select_in_comparison() {
        let result = extract_paths("node.story.status != node.expected_status").unwrap();
        assert!(result
            .paths
            .iter()
            .any(|p| p.segments == vec!["node", "story", "status"]));
        assert!(result
            .paths
            .iter()
            .any(|p| p.segments == vec!["node", "expected_status"]));
    }

    #[test]
    fn parse_error_returns_err() {
        let result = extract_paths("1 + + 2");
        assert!(result.is_err());
    }

    #[test]
    fn extract_paths_with_root() {
        let result = extract_paths("node.story.epic.status == 'done'").unwrap();
        let path = result.paths.iter().find(|p| p.segments.len() == 4).unwrap();
        assert_eq!(path.root, "node");
    }

    #[test]
    fn extract_deeply_nested_path() {
        let result = extract_paths("node.a.b.c.d.e == true").unwrap();
        assert!(result
            .paths
            .iter()
            .any(|p| p.segments == vec!["node", "a", "b", "c", "d", "e"]));
    }

    #[test]
    fn extract_paths_from_conditional_and() {
        let result = extract_paths(
            "node.status == 'open' && node.story.epic.priority == 'high' && node.amount > 1000",
        )
        .unwrap();
        assert!(result
            .paths
            .iter()
            .any(|p| p.segments == vec!["node", "status"]));
        assert!(result
            .paths
            .iter()
            .any(|p| p.segments == vec!["node", "story", "epic", "priority"]));
        assert!(result
            .paths
            .iter()
            .any(|p| p.segments == vec!["node", "amount"]));
    }

    #[test]
    fn size_function_on_path() {
        let result = extract_paths("size(node.tasks) > 0").unwrap();
        assert!(result
            .paths
            .iter()
            .any(|p| p.segments == vec!["node", "tasks"]));
    }
}
