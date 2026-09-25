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
//! A third known false negative, by deliberate design rather than
//! oversight: a bound `NAME` that is later rebound by ANY `let NAME = ...`/
//! `let Some(NAME) = ...` (bare `let`, let-else, `if let`, `while let` all
//! count) ends the search window right there, unconditionally — even when
//! the new value is clearly derived from the old one (`let x = match x {
//! ... }`, the common idiom for narrowing a `Result<Option<T>>` down to
//! `T` in two steps). An earlier version of this scanner tried to except
//! that self-referential case from truncation so its own later field
//! access would still be found, and went through three rounds of
//! increasingly complex, still-incomplete fixes chasing edge cases in that
//! classification (a missed binding shape, a heuristic that scanned into a
//! following block's own body, a depth-aware rewrite that still
//! misclassified a self-referential struct-literal/match/if-expression
//! RHS) before concluding that "does this reshadow derive from the old
//! value" is fundamentally a real-parser question this deliberately
//! simple text scanner has no business trying to answer. Unconditional
//! truncation on any reshadow eliminates that entire recurring bug
//! surface at the cost of this one false-negative class. The concrete,
//! checked cost — two real call sites, found by re-running this scanner
//! against the real codebase after the change and checking every entry
//! `ALLOWLIST` no longer needed: `schema/mod.rs::handle_create_schema`
//! reads `persisted.fields`/`.relationships` (a write-confirmation echo,
//! own-declarations-only and correct — not a bug) via `let persisted =
//! ...; let persisted = match persisted { ... };`; and
//! `schema/mod.rs::resolve_effective_fields` — itself one of the chain-
//! walkers this scanner is supposed to recognize as legitimate — reads
//! `schema.fields` via `if let Some(schema) = schema { chain_fields.push
//! (schema.fields); }`, an if-let reshadowing a variable with itself.
//! Neither call site has an [`ALLOWLIST`] entry any more, the same as the
//! two false-negative classes above.
//!
//! Granularity is per (file, function, field kind) — not per line or per
//! exact call site. A function already on the allowlist for `.fields` is
//! not re-flagged if it grows a *second* `.fields` read reachable from a
//! *different* `get_schema_node` call; keeping the granularity coarser
//! keeps the allowlist small and the scanner simple, at that one known
//! cost. In practice a new bug-class instance shows up in a new function or
//! a new file, which this still catches. The same coarseness also means two
//! *different* functions that happen to share a name in one file (e.g. the
//! same method name in two separate `impl` blocks — legal Rust, rare in
//! this codebase's style) collapse into a single [`Hit`]: an allowlist
//! entry justified for one would silently also cover a genuinely-buggy
//! second function of the same name. Disambiguating would need the key to
//! carry enough of the function's signature/impl target to tell them apart,
//! which is a larger change than this guard's "coarse but simple" design
//! trades for; accepted as a known gap rather than built out, consistent
//! with the false-negative-biased philosophy above.
//!
//! Detection tolerates a bounded set of chain-adapter calls (`.clone()`,
//! `.unwrap()`, `.expect(...)`, `.as_ref()`, `.as_mut()` — see
//! [`CHAIN_ADAPTERS`]) and one level of `.map(|x| ...)` indirection between
//! a tracked call and the `.fields`/`.relationships` access, in both the
//! bound-variable and no-binding forms, and depth-counts the tracked call's
//! own argument parens ([`matching_paren_end`]) rather than assuming no
//! nested call appears in them. Any OTHER method call spliced in between
//! (`.to_owned()`, a custom accessor, etc.) still defeats detection — the
//! adapter list is a finite, named set, not "any method call," by the same
//! simplicity-over-completeness trade-off as everything else here.

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
/// form) would desync this from the real lexical boundary. The blast radius
/// of that desync is the *whole rest of the file* this function is scanning
/// — `strip_comments` runs in a single pass over the entire source before
/// [`strip_cfg_test_blocks`]/[`split_functions`] ever run, so an
/// odd-unescaped-quote-count raw string anywhere in a file, including
/// inside a `#[cfg(test)]` block that gets stripped later, could still
/// corrupt comment-stripping for real production code elsewhere in that
/// same file — not just "near a `get_schema_node`-family call" as a purely
/// local read of this gap might suggest. No such raw string (odd internal
/// `"` count) exists anywhere in `packages/core/src` production code today
/// — checked directly, not just inferred from this guard's own test suite
/// passing against the real codebase — so this remains a documented latent
/// gap rather than an observed failure.
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

        // Find the first `{` or `;` after the signature that occurs at
        // paren/bracket depth zero — i.e. outside the parameter list and
        // outside any `[T; N]` fixed-size array type in a parameter or
        // return type. A plain `.find(['{', ';'])` from right after the
        // function name would instead stop at the FIRST such character
        // anywhere, including one inside `[u8; 32]` or a function-pointer
        // parameter's own parens, and wrongly treat the whole function as a
        // body-less trait declaration — silently dropping it, and every
        // `.fields`/`.relationships` read in it, from the scan entirely.
        let mut depth = 0i32;
        let mut marker = None;
        let mut k = sig_end;
        while k < bytes.len() {
            match bytes[k] {
                b'(' | b'[' => depth += 1,
                b')' | b']' => depth -= 1,
                b'{' | b';' if depth <= 0 => {
                    marker = Some(k);
                    break;
                }
                _ => {}
            }
            k += 1;
        }
        let Some(marker) = marker else {
            continue;
        };
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

/// Find the end index (exclusive) of the balanced-parens call whose opening
/// `(` is at `open_paren`, by depth-counting rather than a `[^)]*`-style
/// regex — so an argument that itself contains a call (a nested `(...)`) is
/// still matched correctly instead of terminating the match at the
/// argument's own inner closing paren. Returns `None` if `open_paren` isn't
/// actually a `(` or the parens never balance before the text ends.
fn matching_paren_end(text: &str, open_paren: usize) -> Option<usize> {
    let bytes = text.as_bytes();
    if bytes.get(open_paren) != Some(&b'(') {
        return None;
    }
    let mut depth = 0i32;
    let mut i = open_paren;
    while i < bytes.len() {
        match bytes[i] {
            b'(' => depth += 1,
            b')' => {
                depth -= 1;
                if depth == 0 {
                    return Some(i + 1);
                }
            }
            _ => {}
        }
        i += 1;
    }
    None
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

/// Zero or more single-hop adapter calls that don't change *which* schema's
/// fields/relationships end up being read (`.clone()`, `.unwrap()`,
/// `.expect(...)`, `.as_ref()`, `.as_mut()`) — allowed to appear between a
/// `get_schema_node`-family result (bound or not) and the `.fields`/
/// `.relationships` access this guard watches for, so `schema.clone().fields`
/// or `...get_schema_node(id).await?.as_ref().unwrap().fields` are still
/// detected, not just the bare `schema.fields` shape. Does not include
/// `.map(...)` — that indirection needs its own closure-variable tracking
/// and is handled separately wherever this constant is used.
const CHAIN_ADAPTERS: &str =
    r"(?:\s*\.\s*(?:clone\(\)|as_ref\(\)|as_mut\(\)|unwrap\(\)|expect\([^()]*\)))*";

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
        r"\blet\s+(?:mut\s+)?(?:Some\(\s*([A-Za-z_][A-Za-z0-9_]*)\s*\)|([A-Za-z_][A-Za-z0-9_]*))\s*=\s*[^;{{}}]{{0,200}}?{TRACKED_CALLS}\s*\("
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

        // Scope the search window to end before NAME is rebound (shadowed)
        // by a LATER `let NAME = ...`/`let Some(NAME) = ...` of any kind
        // (bare let, let-else, if-let, while-let all match — see
        // `reshadow_re`, which mirrors `binding_re`'s own alternation
        // above) — unconditionally, regardless of whether the new value
        // looks derived from NAME's old one. A field access on a
        // genuinely different value bound under the same name must not be
        // attributed to this binding's `get_schema_node` result — a false
        // positive the module doc's design goal explicitly wants to avoid.
        //
        // An earlier version of this check tried to except a
        // *self-referential* re-`let` (`let x = match x { ... }`, the
        // common idiom for narrowing a `Result<Option<T>>` down to `T` in
        // two steps — exactly what `handle_create_schema` used to do)
        // from truncation, so that shape's own later field access would
        // still be found. That classification went through three rounds
        // of increasingly complex, still-incomplete fixes: missing the
        // `Some(NAME)` form entirely; a "next `;`" heuristic that scanned
        // into a following block's own body (`if let`/`while let` have no
        // `;` to find at all; a let-else's else-block commonly mentions
        // the name legitimately); then a depth-aware rewrite that still
        // misclassified a self-referential struct-literal/match/
        // if-expression RHS whose own `{` triggered early. Given this
        // guard's stated design goal — false negatives over false
        // positives, a deliberately non-exhaustive text scan, not a real
        // parser — the right trade is to stop trying to classify
        // self-reference at all: truncate on ANY reshadow, full stop. This
        // makes `handle_create_schema`'s own `persisted.fields`/
        // `.relationships` reads invisible to this scanner — see the
        // module doc's third known false-negative class, and its two
        // ALLOWLIST entries were removed as a direct, documented
        // consequence rather than left as a silent gap.
        let reshadow_re = regex::Regex::new(&format!(
            r"\blet\s+(?:mut\s+)?(?:Some\(\s*{0}\s*\)|{0})\s*=",
            regex::escape(name)
        ))
        .unwrap();
        let rest = match reshadow_re.find(rest) {
            Some(m) => &rest[..m.start()],
            None => rest,
        };

        for field_kind in ["relationships", "fields"] {
            // Direct, optionally through one or more adapter calls:
            // `NAME.field_kind` or `NAME.clone().field_kind` etc.
            let access_re = regex::Regex::new(&format!(
                r"\b{}\b{CHAIN_ADAPTERS}\s*\.\s*{}\b",
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
    // not one, since `get_schema_node` returns `Result<Option<SchemaNode>>`),
    // or with one level of `.map(|x| ...)` indirection instead of `.unwrap()`
    // chains (e.g. `...get_schema_with_relationships(id).await?.map(|s| s.fields...)`).
    //
    // The call's own argument list is matched by depth-aware paren counting
    // ([`matching_paren_end`]), not a `[^)]*`-style regex — an argument
    // that itself contains a call (`get_schema_node(entry.node_type())`)
    // has its own `(`/`)`, which a flat "no `)` at all" character class
    // cannot span.
    let call_start_re = regex::Regex::new(&format!(r"{TRACKED_CALLS}\s*\(")).unwrap();
    for m in call_start_re.find_iter(body) {
        let open_paren = m.end() - 1;
        let Some(close_paren) = matching_paren_end(body, open_paren) else {
            continue;
        };
        let after = &body[close_paren..];

        let suffix_re = regex::Regex::new(&format!(
            r"^(?:\s*\.\s*await\s*\??)?{CHAIN_ADAPTERS}\s*\.\s*(relationships|fields)\b"
        ))
        .unwrap();
        if let Some(cap) = suffix_re.captures(after) {
            kinds.insert(cap.get(1).unwrap().as_str().to_string());
            continue;
        }

        let map_suffix_re = regex::Regex::new(&format!(
            r"^(?:\s*\.\s*await\s*\??)?{CHAIN_ADAPTERS}\s*\.\s*map\(\s*\|\s*([A-Za-z_][A-Za-z0-9_]*)\s*\|"
        ))
        .unwrap();
        if let Some(cap) = map_suffix_re.captures(after) {
            let closure_var = cap.get(1).unwrap().as_str();
            let window_start = cap.get(0).unwrap().end();
            let window = &after[window_start..(window_start + 200).min(after.len())];
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

    // Bound-variable form through a `.clone()`/`.as_ref().unwrap()` chain
    // adapter — not just the bare `NAME.field` shape.
    let clone_chain_fixture = r#"
        async fn clone_chain_bug(&self, node_type: &str) -> Vec<String> {
            let schema = self.get_schema_node(node_type).await.unwrap().unwrap();
            schema.clone().fields.iter().map(|f| f.name.clone()).collect()
        }
    "#;
    let functions = split_functions(clone_chain_fixture);
    let (name, body) = &functions[0];
    let hits = find_hits_in_function(body, "fixture.rs", name);
    assert!(
        hits.iter().any(|h| h.field_kind == "fields"),
        "a chain-adapter call (schema.clone().fields) between the bound NAME and the field \
         access should be detected, not just the bare NAME.fields shape"
    );

    // No-binding form with `.map(|x| ...)` indirection instead of an
    // `.unwrap()`/`.expect(...)` chain — no intermediate `let` at all.
    let unbound_map_fixture = r#"
        async fn unbound_map_bug(&self, node_type: &str) -> Vec<String> {
            self.get_schema_node(node_type).await.unwrap()
                .map(|s| s.fields.iter().map(|f| f.name.clone()).collect())
                .unwrap_or_default()
        }
    "#;
    let functions = split_functions(unbound_map_fixture);
    let (name, body) = &functions[0];
    let hits = find_hits_in_function(body, "fixture.rs", name);
    assert!(
        hits.iter().any(|h| h.field_kind == "fields"),
        "the no-binding chained form should also catch `.map(|x| ...)` indirection, not only \
         `.unwrap()`/`.expect(...)` chains"
    );

    // No-binding form whose call argument itself contains a nested call —
    // a `[^)]*`-style regex cannot span the inner call's own parens.
    let nested_paren_fixture = r#"
        async fn nested_paren_bug(&self, entry: &Entry) -> usize {
            self.get_schema_node(entry.node_type()).await.unwrap().unwrap().fields.len()
        }
    "#;
    let functions = split_functions(nested_paren_fixture);
    let (name, body) = &functions[0];
    let hits = find_hits_in_function(body, "fixture.rs", name);
    assert!(
        hits.iter().any(|h| h.field_kind == "fields"),
        "a get_schema_node call whose own argument contains a nested call \
         (entry.node_type()) should still be detected"
    );

    // A later `let` that rebinds NAME — to ANY value, related or not — must
    // stop the search window there; a field access after it must not be
    // attributed to the original get_schema_node result.
    let unrelated_reshadow_fixture = r#"
        async fn shadow_false_positive(&self, node_type: &str) -> usize {
            let schema = self.get_schema_node(node_type).await.unwrap().unwrap();
            let ok = schema.is_core;
            let schema = unrelated_lookup();
            schema.fields.len()
        }
    "#;
    let functions = split_functions(unrelated_reshadow_fixture);
    let (name, body) = &functions[0];
    let hits = find_hits_in_function(body, "fixture.rs", name);
    assert!(
        hits.is_empty(),
        "a field access on a variable reshadowed to an unrelated value must NOT be attributed \
         to the earlier get_schema_node binding of the same name: {hits:?}"
    );

    // Even a SELF-referential re-`let` (`let x = match x { ... }`, the
    // idiom for narrowing a `Result<Option<T>>` down to `T` in two steps —
    // `schema/mod.rs::handle_create_schema` uses exactly this shape in real
    // code) is now truncated too, unconditionally. This is a deliberate,
    // documented trade-off (see the module doc's third known false-negative
    // class), not an oversight: distinguishing "the new value is derived
    // from the old one" from "the new value is unrelated" turned out to be
    // a real-parser question this text scanner kept getting wrong in new
    // ways, so it no longer tries. `handle_create_schema` itself has no
    // ALLOWLIST entry as a direct, accepted consequence.
    let self_referential_reshadow_is_now_excluded_fixture = r#"
        async fn self_ref_reshadow(&self, node_type: &str) -> usize {
            let persisted = self.get_schema_node(node_type).await.unwrap();
            let persisted = match persisted {
                Some(s) => s,
                None => return 0,
            };
            persisted.fields.len()
        }
    "#;
    let functions = split_functions(self_referential_reshadow_is_now_excluded_fixture);
    let (name, body) = &functions[0];
    let hits = find_hits_in_function(body, "fixture.rs", name);
    assert!(
        hits.is_empty(),
        "a self-referential re-let (let x = match x {{ ... }}) is now expected to be excluded \
         by the unconditional reshadow truncation, not specially preserved: {hits:?}"
    );

    // A function signature containing a fixed-size array type (`[u8; 32]`)
    // has a `;` before the body's own `{` — `split_functions` must not
    // mistake that for a body-less trait declaration and drop the whole
    // function (and everything in it) from the scan.
    let array_sig_fixture = r#"
        async fn hashes_a_thing(&self, node_type: &str) -> [u8; 32] {
            let Some(schema) = self.get_schema_node(node_type).await.unwrap() else {
                return [0u8; 32];
            };
            for f in &schema.fields {
                println!("{}", f.name);
            }
            [0u8; 32]
        }
    "#;
    let functions = split_functions(array_sig_fixture);
    assert_eq!(
        functions.len(),
        1,
        "a function whose signature contains a `[T; N]` array type must still be found by \
         split_functions, not silently dropped"
    );
    let (name, body) = &functions[0];
    let hits = find_hits_in_function(body, "fixture.rs", name);
    assert!(
        hits.iter().any(|h| h.field_kind == "fields"),
        "a field access inside a function with an array-typed signature must still be detected"
    );

    // The reshadow-window-truncation logic must recognize a reshadow via
    // `let Some(NAME) = ... else { ... }`, not only plain `let NAME = ...`
    // — `binding_re` itself supports both forms, so the reshadow check that
    // scopes the search window must mirror that same alternation or an
    // unrelated `Some(NAME)`-shaped reshadow silently falls through it.
    let unrelated_some_reshadow_fixture = r#"
        async fn shadow_false_positive_some(&self, node_type: &str) -> usize {
            let schema = self.get_schema_node(node_type).await.unwrap().unwrap();
            let ok = schema.is_core;
            let Some(schema) = unrelated_lookup() else { return 0; };
            schema.fields.len()
        }
    "#;
    let functions = split_functions(unrelated_some_reshadow_fixture);
    let (name, body) = &functions[0];
    let hits = find_hits_in_function(body, "fixture.rs", name);
    assert!(
        hits.is_empty(),
        "a field access on a variable reshadowed via `let Some(NAME) = <unrelated>` must NOT \
         be attributed to the earlier get_schema_node binding of the same name: {hits:?}"
    );

    // A SELF-referential `let Some(NAME) = NAME else { ... }` reshadow is
    // now also excluded, symmetric with the plain-`let` case above — the
    // unconditional truncation doesn't distinguish this form either.
    let self_ref_some_reshadow_is_now_excluded_fixture = r#"
        async fn self_ref_some_reshadow(&self, node_type: &str) -> usize {
            let schema = self.get_schema_node(node_type).await.unwrap();
            let Some(schema) = schema else { return 0; };
            schema.fields.len()
        }
    "#;
    let functions = split_functions(self_ref_some_reshadow_is_now_excluded_fixture);
    let (name, body) = &functions[0];
    let hits = find_hits_in_function(body, "fixture.rs", name);
    assert!(
        hits.is_empty(),
        "a self-referential `let Some(NAME) = NAME else {{ ... }}` reshadow is now expected to \
         be excluded by the unconditional truncation: {hits:?}"
    );

    // `reshadow_re`'s `let`-based pattern also matches the `let` embedded
    // in `if let`/`while let` (they're the same regex, unconditionally, not
    // a case this scanner special-cases) — an unrelated reshadow through
    // either form still correctly ends the window, even though its body
    // legitimately mentions the newly (unrelated) bound name.
    let if_let_unrelated_fixture = r#"
        async fn if_let_unrelated(&self, node_type: &str) -> usize {
            let schema = self.get_schema_node(node_type).await.unwrap().unwrap();
            let ok = schema.is_core;
            if let Some(schema) = unrelated_lookup() {
                return schema.fields.len();
            }
            0
        }
    "#;
    let functions = split_functions(if_let_unrelated_fixture);
    let (name, body) = &functions[0];
    let hits = find_hits_in_function(body, "fixture.rs", name);
    assert!(
        hits.is_empty(),
        "an if-let reshadow to an unrelated value must not be flagged: {hits:?}"
    );
}
