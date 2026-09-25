//! Structural guard against a recurring bug shape (ADR-078): a function
//! reading `.relationships`/`.fields` directly off a single
//! `NodeService::get_schema_node(node_type)` call, instead of the
//! extends-chain-merged set via `NodeService::resolve_relationships`/
//! `resolve_field_owners`.
//!
//! `get_schema_node` returns a type's own directly-declared fields/
//! relationships only, by design — and that is the *correct* choice at a
//! handful of call sites (the chain-walkers themselves, which call it once
//! per schema in the chain; a handful of own-declarations-only checks that
//! are correct precisely because they must NOT see inherited declarations).
//! Nothing distinguishes those call sites from a new one that should have
//! gone through the chain-merged helper but didn't — both look identical at
//! a glance. The same shape has recurred independently at least seven times
//! (`workflow_state.rs`, `validation.rs`, `graph_resolver.rs`,
//! `rel_ops.rs` twice, `find_duplicate_for`, `detect_unique_field_collisions`,
//! `check_node_completeness`), each caught only by a human tracing a
//! specific bug report back to this exact pattern, after already shipping.
//!
//! This test turns that manual-review dependency into a structural one: it
//! scans `packages/core/src` for functions that bind a `get_schema_node`
//! (or `get_schema_with_relationships` — see below) result and then read
//! `.relationships`/`.fields` directly off it, and fails when it finds one
//! that is not on [`ALLOWLIST`] below.
//!
//! ## What to do when this test fails
//!
//! - If your new call site should see the extends-chain-merged set (almost
//!   always the right answer for anything touching a node *instance*, as
//!   opposed to validating one schema's own declaration list): use
//!   [`NodeService::resolve_relationships`]/[`NodeService::resolve_field_owners`]
//!   instead of reading `.relationships`/`.fields` off `get_schema_node`'s
//!   result, and this test will stop flagging it (nothing to allowlist).
//! - If your call site is genuinely, intentionally own-declarations-only
//!   (e.g. it walks the chain itself calling `get_schema_node` once per
//!   scope, or it specifically must not see inherited declarations), add an
//!   entry to [`ALLOWLIST`] with a one-line justification of *why* chain-
//!   merging doesn't apply here. That justification is what the next person
//!   reviewing this list — or the next bug report — checks first.
//!
//! ## Why `get_schema_with_relationships` counts too
//!
//! `NodeService::get_schema_with_relationships` is, despite its name, a
//! literal passthrough to `get_schema_node` (`self.get_schema_node(id).await`
//! — no chain-merging happens in it at all). It exists as the documented
//! *fallback* source `workflow_state.rs` reads when the primary
//! `resolve_field_owners`/`resolve_relationships` calls fail. Its name reads
//! as though it already returns the chain-merged set, which is exactly the
//! kind of look-alike this guard exists to catch, so it is tracked
//! identically to a direct `get_schema_node` call below.
//!
//! ## Scope
//!
//! Only `packages/core/src` (production code) is scanned. Files that are
//! test-only by this codebase's own naming convention (`*_test.rs`, or
//! `tests.rs` — always included via `#[cfg(test)] #[path = "..."] mod ...;`
//! in their parent module) are skipped entirely, and any inline
//! `#[cfg(test)] mod { ... }` block within an otherwise-production file is
//! stripped before scanning — a test exercising `get_schema_node`'s own
//! own-declarations-only contract directly is testing the primitive, not
//! doing chain-blind business logic.
//!
//! Detection is a straightforward, deliberately non-exhaustive text scan
//! (regex over each function's body, not a real parser) — see
//! [`find_hits_in_function`] for exactly what it does and does not catch.
//! It errs toward false negatives over false positives: missing a real hit
//! only preserves the status quo (still caught by review, same as before
//! this test existed), while a false positive would make the allowlist
//! noise nobody trusts. One known class of false negative, found and left
//! unaddressed during this guard's own construction: a conversion function
//! that receives an already-resolved `&SchemaNode` as a plain parameter
//! (rather than calling `get_schema_node` itself) and reads `.fields`/
//! `.relationships` off it is invisible here — this scanner only traces a
//! binding back to a `get_schema_node`-family call within the *same*
//! function body, not across a function boundary. A real instance of
//! exactly that shape was found by manual audit while building this guard
//! (`EntityTypeDescriptor::from_schema` in `ops/entity_types_block.rs`,
//! fed unmerged schemas from `ops/context_ops.rs` and `ops/skill_ops.rs`)
//! and filed as its own follow-up rather than fixed here, per this guard's
//! own non-goals; there is nothing to allowlist for it since the scanner
//! does not find it in the first place.
//!
//! A second known false negative, also found during construction:
//! `workflow_state.rs::walk_path_against_schema` reads
//! `current_schema.fields`/`.relationships` (a genuine, documented,
//! intentional degraded-fallback read — same pattern as
//! `get_workflow_state`, which IS on the allowlist below) via a
//! loop-carried `current_schema_owned` that is reassigned with plain
//! `current_schema_owned = ...get_schema_with_relationships(...)` (no
//! `let`, so outside the binding regex's reach), then re-derived into the
//! differently-named `current_schema` through `.as_ref()` before the
//! `.map(|s| s.fields...)` read this scanner does otherwise understand.
//! Tracing a reassigned, loop-carried, then re-bound-under-a-different-name
//! variable is real dataflow analysis, not a text scan — left as a second
//! documented gap rather than the scanner growing that complexity.
//!
//! Granularity is per (file, function, field kind) — not per line or per
//! exact call site. A function already on the allowlist for `.fields` is
//! not re-flagged if it grows a *second* `.fields` read reachable from a
//! *different* `get_schema_node` call; keeping the granularity coarser
//! keeps the allowlist small and the scanner simple, at that one known
//! cost. In practice a new bug-class instance shows up in a new function or
//! a new file, which this still catches.

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

/// One permitted (file, function, field kind) combination, with the
/// justification a reviewer needs to trust it's still correct.
///
/// `file` is relative to `packages/core/src/` using forward slashes.
/// `function` is the name of the `fn` whose body the hit was found in.
/// `field_kind` is `"fields"` or `"relationships"`.
struct Allowed {
    file: &'static str,
    function: &'static str,
    field_kind: &'static str,
    why: &'static str,
}

/// Every call site currently allowed to read `.relationships`/`.fields`
/// directly off a `get_schema_node`/`get_schema_with_relationships` result.
/// Seeded by manually reading every such call site in `packages/core/src`
/// (production code) at the time this guard was added. Add to this list
/// only with a real justification; removing an entry that no longer has a
/// matching call site is required, not optional (the test enforces that
/// direction too).
const ALLOWLIST: &[Allowed] = &[
    Allowed {
        file: "services/node_service/schema.rs",
        function: "resolve_field_owners",
        field_kind: "fields",
        why: "This IS the chain-merged helper: it calls get_schema_node once \
              per schema in the extends chain and merges the results. \
              Reading .fields off each per-hop result is the correct, \
              intentional implementation of chain-walking, not the bug this \
              guard watches for.",
    },
    Allowed {
        file: "services/node_service/schema.rs",
        function: "resolve_relationships",
        field_kind: "relationships",
        why: "The relationship counterpart to resolve_field_owners above — \
              same per-hop chain-walk, same reasoning.",
    },
    Allowed {
        file: "services/node_service/schema.rs",
        function: "rename_schema_field",
        field_kind: "fields",
        why: "Two own-only reads, both intentional. (1) The source field \
              being renamed must be OWN-declared on type_id — you can only \
              rename a field the type itself declares; renaming an inherited \
              field would mean renaming the ancestor's field, which is a \
              different operation on a different schema. (2) The descendant- \
              collision check walks type_id's subtype closure and checks \
              each descendant's own fields only, deliberately not its \
              resolved effective set — ADR-078 redeclaration checks already \
              guarantee no descendant redeclares anything type_id currently \
              owns, so only a descendant's OWN name can newly collide with \
              the rename target. (The destination name IS checked against \
              the chain-merged effective set, via resolve_field_owners, \
              elsewhere in this same function.)",
    },
    Allowed {
        file: "services/node_service/schema.rs",
        function: "update_schema_field_friendly_name",
        field_kind: "fields",
        why: "Same reasoning as rename_schema_field's source-field check: a \
              friendly_name relabel is a modification of a field definition, \
              only valid for a field type_id itself declares.",
    },
    Allowed {
        file: "schema/mod.rs",
        function: "resolve_effective_fields",
        field_kind: "fields",
        why: "A second, parallel implementation of resolve_field_owners' \
              chain-walk, living in the MCP schema-handler layer rather than \
              on NodeService directly (its own doc comment: 'This is what \
              validation, defaulting and schema comprehension read instead \
              of a schema's own directly-declared fields'). Same per-hop \
              get_schema_node-then-.fields shape, same intentional chain-walk.",
    },
    Allowed {
        file: "playbook/workflow_state.rs",
        function: "get_workflow_state",
        field_kind: "fields",
        why: "Documented degraded fallback ONLY: the primary source is an \
              independent resolve_field_owners call; this own-declarations- \
              only read off get_schema_with_relationships is used solely \
              when that primary call fails (a transient DB error), and the \
              failure is recorded in `degraded` so it's never silent.",
    },
    Allowed {
        file: "schema/mod.rs",
        function: "handle_create_schema",
        field_kind: "fields",
        why: "Write-confirmation echo, not a business-logic read: after \
              creating a brand-new schema, `persisted` is that same schema \
              read back via get_schema_node_verifying to confirm the write \
              actually landed (see the function's own doc comment on why a \
              read-back replaced echoing the request payload back \
              verbatim). Reports exactly what was just persisted for THIS \
              schema, not an effective/inherited view — there is nothing to \
              chain-merge for a type that was only just created.",
    },
    Allowed {
        file: "schema/mod.rs",
        function: "handle_create_schema",
        field_kind: "relationships",
        why: "Same write-confirmation echo as this function's `fields` \
              entry above, relationship counterpart.",
    },
    Allowed {
        file: "schema/mod.rs",
        function: "handle_update_schema",
        field_kind: "relationships",
        why: "Two own-only reads while mutating params.schema_id's OWN \
              declaration list, neither about inheritance. (1) \
              `schema_before.relationships` computes 'this schema's own \
              relationships as they'll stand once this same call's \
              removals apply', used only for the same-schema (sibling- \
              declaration) collision check on a rename destination — a \
              different, narrower check than the chain-merged redeclaration \
              check (`validate_no_relationship_redeclaration`, which \
              correctly uses resolve_relationships elsewhere). (2) Phase 2 \
              clones `schema.relationships` as the starting point for \
              applying THIS call's add/remove_relationships mutations \
              before writing the schema back — reading the inherited set \
              here would corrupt the write by persisting ancestor-owned \
              relationships as this schema's own declarations.",
    },
    Allowed {
        file: "schema/mod.rs",
        function: "handle_update_schema",
        field_kind: "fields",
        why: "Phase 2 clones `schema.fields` as the starting point for \
              applying this call's add/remove_fields mutations before \
              writing the schema back — the field counterpart of this \
              function's `relationships` entry's reason (2) above.",
    },
];

/// Directories skipped entirely — generated/vendored code, not source this
/// guard needs to reason about.
const EXCLUDE_DIR_NAMES: &[&str] = &["target", ".git"];

/// This codebase's own convention for a file that exists purely to hold
/// `#[cfg(test)]` content for a sibling module (included via
/// `#[cfg(test)] #[path = "..."] mod name;`) — see e.g. `schema/schema_test.rs`,
/// `playbook/tests.rs`, `models/task_node_test.rs`. Excluded by filename
/// rather than relying on cfg-stripping the `mod` declaration in the parent
/// file, which only tells us the *content* is test-only, not that the whole
/// file is.
fn is_test_only_file(file_name: &str) -> bool {
    file_name.ends_with("_test.rs") || file_name == "tests.rs"
}

/// Remove `//`/`///`/`//!` line-comment text from `source` (replacing it
/// with nothing but preserving the newline), tracking `"..."` string
/// literals so a `//` inside one (a URL in an error message, say) is left
/// alone. This runs before [`strip_cfg_test_blocks`]/[`split_functions`] for
/// two reasons found while building this guard against the real codebase:
///
/// - A doc comment's illustrative code example (`/// if let Some(schema) =
///   service.get_schema_node("task").await? { println!("{}",
///   schema.fields.len()); }`, which several real doc comments in this
///   codebase contain verbatim) would otherwise both (a) get mistaken by
///   [`split_functions`] for a real function when the example includes a
///   `# fn main() { ... }` doctest wrapper, and (b) trip the field-access
///   detection with a hit that isn't real code.
/// - A comment can name a variable-and-field pair in prose/backticks (e.g.
///   "checked against ... not `schema.fields` alone") right next to a real
///   `get_schema_node`-bound variable of the same name, which would
///   otherwise look identical to a real access to the regex.
///
/// Does not handle `/* */` block comments — this codebase uses none (a
/// silent audit at the time this guard was written found zero), so
/// handling them would be untested complexity for a case that doesn't
/// exist; a follow-up guard change is expected before one becomes load-
/// bearing for detection correctness.
///
/// Does not distinguish a char literal (`'/'`) from other single-quoted
/// tokens (lifetimes) — accepted as a known simplification, since none of
/// the files this guard cares about contain a char literal holding `/` or
/// `"` adjacent to a `get_schema_node`-family call.
///
/// Also does not recognize raw strings (`r"..."`, `r#"..."#`) as a distinct
/// form — a `"` anywhere, raw-string delimiter or not, toggles `in_string`.
/// A raw string containing an unescaped `"` (only possible in the `r#"..."#`
/// form) would desync this from the real lexical boundary. No such case
/// exists near a `get_schema_node`-family call in this codebase today (this
/// guard's own test suite scans the whole real codebase and passes), so
/// this is a documented latent gap rather than an observed failure.
fn strip_comments(source: &str) -> String {
    let mut out = String::with_capacity(source.len());
    let mut in_string = false;
    let mut escape = false;
    let mut chars = source.chars().peekable();

    while let Some(c) = chars.next() {
        if in_string {
            out.push(c);
            if escape {
                escape = false;
            } else if c == '\\' {
                escape = true;
            } else if c == '"' {
                in_string = false;
            }
            continue;
        }

        if c == '"' {
            in_string = true;
            out.push(c);
            continue;
        }

        if c == '/' && chars.peek() == Some(&'/') {
            // Line comment (covers `//`, `///`, `//!` alike): discard the
            // rest of the line, keep the newline itself so line-based
            // reasoning elsewhere stays meaningful.
            for next in chars.by_ref() {
                if next == '\n' {
                    out.push('\n');
                    break;
                }
            }
            continue;
        }

        out.push(c);
    }

    out
}

fn core_src_root() -> PathBuf {
    // `CARGO_MANIFEST_DIR` for an integration test in packages/core/tests is
    // packages/core itself.
    Path::new(env!("CARGO_MANIFEST_DIR")).join("src")
}

fn walk_rs_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if path.is_dir() {
            if !EXCLUDE_DIR_NAMES.contains(&name.as_ref()) {
                walk_rs_files(&path, out);
            }
        } else if name.ends_with(".rs") && !is_test_only_file(&name) {
            out.push(path);
        }
    }
}

/// Strip every `#[cfg(test)]`-gated item's body out of `source`, replacing
/// it with blank lines (to keep line numbers stable for any future
/// debugging, though this test doesn't currently report them).
///
/// Handles both shapes this codebase uses: a brace-delimited item
/// (`#[cfg(test)] mod tests { ... }`) is blanked from the attribute through
/// the matching close brace; a semicolon-terminated external-file module
/// declaration (`#[cfg(test)] #[path = "foo_test.rs"] mod foo_test;`) has
/// no body to strip and is blanked only through its own `;` — treating the
/// first `{` found *anywhere later in the file* as this item's body would
/// wrongly blank real production code in between.
///
/// Deliberately simple otherwise: brace-depth counting does not account for
/// braces inside string/char literals or comments (see module doc: false
/// negatives are the accepted failure mode here, not false positives — and
/// in practice this codebase's test modules don't do that inside their
/// outer module brace structure).
fn strip_cfg_test_blocks(source: &str) -> String {
    let bytes = source.as_bytes();
    let mut out = String::with_capacity(source.len());
    let mut i = 0;
    while i < bytes.len() {
        if source[i..].starts_with("#[cfg(test)]") {
            let rel_open = source[i..].find('{');
            let rel_semi = source[i..].find(';');
            let end = match (rel_open, rel_semi) {
                (Some(o), Some(s)) if s < o => i + s + 1,
                (Some(o), _) => {
                    let open = i + o;
                    let mut depth = 0usize;
                    let mut j = open;
                    let mut end = source.len();
                    while j < bytes.len() {
                        match bytes[j] {
                            b'{' => depth += 1,
                            b'}' => {
                                depth -= 1;
                                if depth == 0 {
                                    end = j + 1;
                                    break;
                                }
                            }
                            _ => {}
                        }
                        j += 1;
                    }
                    end
                }
                (None, Some(s)) => i + s + 1,
                (None, None) => source.len(),
            };
            for ch in source[i..end].chars() {
                if ch == '\n' {
                    out.push('\n');
                }
            }
            i = end;
            continue;
        }
        // Advance by one char (not byte) to stay on a UTF-8 boundary.
        let ch = source[i..].chars().next().unwrap();
        out.push(ch);
        i += ch.len_utf8();
    }
    out
}

/// Split `source` into (function_name, function_body) pairs by finding each
/// `fn <name>` signature and taking its body via brace counting from the
/// first `{` after the signature to the matching `}`.
///
/// A signature with no body (a trait method declaration, `fn foo(...);`)
/// is skipped — there's no body to read `.relationships`/`.fields` in.
fn split_functions(source: &str) -> Vec<(String, String)> {
    let fn_re = regex::Regex::new(r"\bfn\s+([A-Za-z_][A-Za-z0-9_]*)").unwrap();
    let mut out = Vec::new();
    let bytes = source.as_bytes();

    for cap in fn_re.captures_iter(source) {
        let name = cap.get(1).unwrap().as_str().to_string();
        let sig_end = cap.get(0).unwrap().end();

        // Find the first `{` or `;` after the signature, whichever comes
        // first (a `;` means a body-less trait declaration).
        let Some(rel) = source[sig_end..].find(['{', ';']) else {
            continue;
        };
        let marker = sig_end + rel;
        if bytes[marker] == b';' {
            continue;
        }

        let mut depth = 0usize;
        let mut j = marker;
        let mut end = source.len();
        while j < bytes.len() {
            match bytes[j] {
                b'{' => depth += 1,
                b'}' => {
                    depth -= 1;
                    if depth == 0 {
                        end = j + 1;
                        break;
                    }
                }
                _ => {}
            }
            j += 1;
        }

        out.push((name, source[marker..end].to_string()));
    }

    out
}

/// One detected direct read of `.relationships`/`.fields` off a
/// `get_schema_node`/`get_schema_with_relationships` result.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct Hit {
    file: String,
    function: String,
    field_kind: String,
}

/// The call-site names this guard treats identically — see the module doc's
/// "Why `get_schema_with_relationships` counts too".
const TRACKED_CALLS: &str = r"(?:get_schema_node(?:_verifying)?|get_schema_with_relationships)";

/// Scan one function body for a `get_schema_node`-family-bound variable
/// whose `.relationships`/`.fields` is read later in the same body —
/// directly, or through one level of `Option::map(|x| x.FIELD)`
/// indirection (the common `schema.as_ref().map(|s| s.fields.clone())`
/// idiom this codebase actually uses) — or an immediate chained read with
/// no intermediate binding.
///
/// Binding detection: `let NAME = ...get_schema_node(...)` or
/// `let Some(NAME) = ...get_schema_node(...)`, allowing the tracked call to
/// appear anywhere within a bounded window after the `let` (covers the
/// common multi-line `.method().await?` chain shape) but not across a
/// statement/block boundary (`;`, `{`, `}`), so an unrelated `let` earlier
/// in the function can't be mistaken for the binding.
fn find_hits_in_function(body: &str, file: &str, function: &str) -> Vec<Hit> {
    let mut kinds = BTreeSet::new();

    let binding_re = regex::Regex::new(&format!(
        r"let\s+(?:mut\s+)?(?:Some\(\s*([A-Za-z_][A-Za-z0-9_]*)\s*\)|([A-Za-z_][A-Za-z0-9_]*))\s*=\s*[^;{{}}]{{0,200}}?{TRACKED_CALLS}\s*\("
    ))
    .unwrap();

    for cap in binding_re.captures_iter(body) {
        let name = cap
            .get(1)
            .or_else(|| cap.get(2))
            .map(|m| m.as_str())
            .unwrap();
        let bind_end = cap.get(0).unwrap().end();
        let rest = &body[bind_end..];

        for field_kind in ["relationships", "fields"] {
            // Direct: `NAME.field_kind`.
            let access_re = regex::Regex::new(&format!(
                r"\b{}\s*\.\s*{}\b",
                regex::escape(name),
                field_kind
            ))
            .unwrap();
            if access_re.is_match(rest) {
                kinds.insert(field_kind.to_string());
            }
        }

        // One level of `.map(|x| ...)` indirection off NAME (e.g.
        // `schema.as_ref().map(|s| s.fields.clone())`): find the closure
        // parameter, then look for that parameter's own `.field_kind`
        // access within the map call's near vicinity. Two passes rather
        // than a backreference, since the `regex` crate doesn't support
        // backreferences.
        let map_re = regex::Regex::new(&format!(
            r"\b{}\b[^;{{}}]{{0,80}}?\.map\(\s*\|\s*([A-Za-z_][A-Za-z0-9_]*)\s*\|",
            regex::escape(name)
        ))
        .unwrap();
        for mcap in map_re.captures_iter(rest) {
            let closure_var = mcap.get(1).unwrap().as_str();
            let window_start = mcap.get(0).unwrap().end();
            let window = &rest[window_start..(window_start + 200).min(rest.len())];
            for field_kind in ["relationships", "fields"] {
                let access_re = regex::Regex::new(&format!(
                    r"\b{}\s*\.\s*{}\b",
                    regex::escape(closure_var),
                    field_kind
                ))
                .unwrap();
                if access_re.is_match(window) {
                    kinds.insert(field_kind.to_string());
                }
            }
        }
    }

    // No-binding form: an immediate chained read, e.g.
    // `...get_schema_node(id).await?.unwrap().relationships`, or
    // `...get_schema_node(id).await.unwrap().unwrap().fields` (the Result
    // AND the Option both unwrapped before the field access — two calls,
    // not one, since `get_schema_node` returns `Result<Option<SchemaNode>>`).
    let chained_re = regex::Regex::new(&format!(
        r"{TRACKED_CALLS}\s*\([^)]*\)(?:\s*\.\s*await\s*\??)?(?:\s*\.\s*(?:unwrap\(\)|expect\([^)]*\)))*\s*\.\s*(relationships|fields)\b"
    ))
    .unwrap();
    for cap in chained_re.captures_iter(body) {
        kinds.insert(cap.get(1).unwrap().as_str().to_string());
    }

    kinds
        .into_iter()
        .map(|field_kind| Hit {
            file: file.to_string(),
            function: function.to_string(),
            field_kind,
        })
        .collect()
}

/// Scan every `.rs` file under `packages/core/src` (production code only —
/// test-only files and `#[cfg(test)]` blocks are excluded, see module doc)
/// for the bug-class shape this guard watches for.
fn find_hits() -> BTreeSet<Hit> {
    let root = core_src_root();
    let mut files = Vec::new();
    walk_rs_files(&root, &mut files);

    let mut hits = BTreeSet::new();
    for path in files {
        let Ok(source) = fs::read_to_string(&path) else {
            continue;
        };
        let source = strip_comments(&source);
        let source = strip_cfg_test_blocks(&source);
        let rel = path
            .strip_prefix(&root)
            .unwrap_or(&path)
            .to_string_lossy()
            .replace('\\', "/");

        for (function, fn_body) in split_functions(&source) {
            hits.extend(find_hits_in_function(&fn_body, &rel, &function));
        }
    }
    hits
}

#[test]
fn get_schema_node_relationships_and_fields_reads_are_allowlisted() {
    let found = find_hits();
    let allowed: BTreeSet<Hit> = ALLOWLIST
        .iter()
        .map(|a| Hit {
            file: a.file.to_string(),
            function: a.function.to_string(),
            field_kind: a.field_kind.to_string(),
        })
        .collect();

    let unlisted: Vec<&Hit> = found.difference(&allowed).collect();
    let stale: Vec<&Hit> = allowed.difference(&found).collect();

    if !unlisted.is_empty() {
        let mut msg = String::from(
            "Found call site(s) reading `.relationships`/`.fields` directly off a \
             `get_schema_node`/`get_schema_with_relationships` result that are NOT on the \
             allowlist in packages/core/tests/schema_chain_blindness_guard_test.rs.\n\n\
             This is the extends-chain-blindness bug shape (ADR-078): `get_schema_node` \
             returns a type's own directly-declared fields/relationships only, not the \
             extends-chain-merged set. If your call site should see inherited \
             declarations (almost always true for anything touching a node instance), \
             use `NodeService::resolve_relationships`/`resolve_field_owners` instead. If \
             it's genuinely, intentionally own-declarations-only, add it to `ALLOWLIST` \
             with a justification.\n\nUnlisted call site(s):\n",
        );
        for hit in &unlisted {
            msg.push_str(&format!(
                "  - {} :: fn {} :: reads .{}\n",
                hit.file, hit.function, hit.field_kind
            ));
        }
        panic!("{msg}");
    }

    if !stale.is_empty() {
        let mut msg = String::from(
            "ALLOWLIST in packages/core/tests/schema_chain_blindness_guard_test.rs has \
             entrie(s) that no longer match any call site the scanner finds — the \
             function was renamed/removed/rewritten, or no longer reads that field this \
             way. Remove the stale entry so the allowlist stays an accurate record of \
             what's actually own-declarations-only today.\n\nStale entrie(s):\n",
        );
        for hit in &stale {
            let why = ALLOWLIST
                .iter()
                .find(|a| {
                    a.file == hit.file
                        && a.function == hit.function
                        && a.field_kind == hit.field_kind
                })
                .map(|a| a.why)
                .unwrap_or("");
            msg.push_str(&format!(
                "  - {} :: fn {} :: .{}\n    (was justified as: {})\n",
                hit.file, hit.function, hit.field_kind, why
            ));
        }
        panic!("{msg}");
    }
}

#[test]
fn scanner_detects_a_deliberately_bad_fixture() {
    // Self-test of the scanner's own detection logic, via an embedded
    // fixture string rather than modifying real source (see the issue this
    // test closes) — proves `find_hits_in_function`/`split_functions` catch
    // both the bound-variable and the direct-chain shape, without needing
    // to temporarily vandalize real production code to prove it.
    let bound_variable_fixture = r#"
        async fn totally_fake_bug(&self, node_type: &str) -> Result<(), Error> {
            let Some(schema) = self.get_schema_node(node_type).await? else {
                return Ok(());
            };
            for rel in &schema.relationships {
                println!("{}", rel.name);
            }
            Ok(())
        }
    "#;
    let functions = split_functions(bound_variable_fixture);
    assert_eq!(functions.len(), 1);
    let (name, body) = &functions[0];
    assert_eq!(name, "totally_fake_bug");
    let hits = find_hits_in_function(body, "fixture.rs", name);
    assert_eq!(
        hits,
        vec![Hit {
            file: "fixture.rs".to_string(),
            function: "totally_fake_bug".to_string(),
            field_kind: "relationships".to_string(),
        }],
        "bound-variable form (let Some(schema) = ...get_schema_node(...); schema.relationships) \
         should be detected"
    );

    let direct_chain_fixture = r#"
        async fn another_fake_bug(&self, node_type: &str) -> usize {
            self.get_schema_node(node_type).await.unwrap().unwrap().fields.len()
        }
    "#;
    let functions = split_functions(direct_chain_fixture);
    let (name, body) = &functions[0];
    let hits = find_hits_in_function(body, "fixture.rs", name);
    assert!(
        hits.iter().any(|h| h.field_kind == "fields"),
        "direct-chain form (get_schema_node(...).await.unwrap().unwrap().fields) should be \
         detected"
    );

    let with_relationships_alias_fixture = r#"
        async fn yet_another_fake_bug(&self, node_type: &str) -> Vec<String> {
            let schema = match self.get_schema_with_relationships(node_type).await {
                Ok(s) => s,
                Err(_) => None,
            };
            schema.map(|s| s.fields.iter().map(|f| f.name.clone()).collect()).unwrap_or_default()
        }
    "#;
    let functions = split_functions(with_relationships_alias_fixture);
    let (name, body) = &functions[0];
    let hits = find_hits_in_function(body, "fixture.rs", name);
    assert!(
        hits.iter().any(|h| h.field_kind == "fields"),
        "get_schema_with_relationships (a documented passthrough alias of get_schema_node) \
         combined with the .map(|s| s.fields) closure-indirection idiom should be detected"
    );

    // A call site that only checks existence, or reads some other field,
    // must NOT be flagged — that's the common, correct case and flagging
    // it would make the allowlist unusably noisy.
    let benign_fixture = r#"
        async fn totally_fine(&self, node_type: &str) -> bool {
            let Some(schema) = self.get_schema_node(node_type).await? else {
                return false;
            };
            schema.is_core
        }
    "#;
    let functions = split_functions(benign_fixture);
    let (name, body) = &functions[0];
    let hits = find_hits_in_function(body, "fixture.rs", name);
    assert!(
        hits.is_empty(),
        "a get_schema_node result that never reads .relationships/.fields must not be flagged"
    );

    // A #[cfg(test)]-gated function must be stripped before scanning, so it
    // never contributes a hit regardless of what it does.
    let cfg_test_fixture = r#"
        #[cfg(test)]
        mod tests {
            #[tokio::test]
            async fn some_test() {
                let schema = svc.get_schema_node("task").await.unwrap().unwrap();
                assert_eq!(schema.fields.len(), 3);
            }
        }

        async fn real_production_fn(&self) -> bool {
            true
        }
    "#;
    let stripped = strip_cfg_test_blocks(cfg_test_fixture);
    assert!(
        !stripped.contains("get_schema_node"),
        "content inside a #[cfg(test)] mod block must be stripped before function-splitting"
    );
    let functions = split_functions(&stripped);
    assert!(
        functions
            .iter()
            .any(|(name, _)| name == "real_production_fn"),
        "stripping #[cfg(test)] must not eat the following real function"
    );

    // The semicolon-terminated external-file module form
    // (`#[cfg(test)] #[path = "..."] mod foo_test;`) must be blanked only
    // through its own `;` — NOT through the next `{` found anywhere later
    // in the file, which would wrongly blank real production code between
    // the two. This is the exact shape `schema/mod.rs` uses for
    // `schema_test.rs`.
    let cfg_test_mod_decl_fixture = r#"
        #[cfg(test)]
        #[path = "schema_test.rs"]
        mod schema_test;

        async fn real_production_fn_after_mod_decl(&self) -> bool {
            let Some(schema) = self.get_schema_node("task").await? else {
                return false;
            };
            schema.is_core
        }
    "#;
    let stripped = strip_cfg_test_blocks(cfg_test_mod_decl_fixture);
    assert!(
        stripped.contains("real_production_fn_after_mod_decl"),
        "a semicolon-terminated #[cfg(test)] mod declaration must not blank out unrelated \
         production code that follows it before the next brace"
    );
    let functions = split_functions(&stripped);
    assert!(
        functions
            .iter()
            .any(|(name, _)| name == "real_production_fn_after_mod_decl"),
        "the function after a semicolon-terminated #[cfg(test)] mod declaration must survive \
         stripping intact"
    );
}
