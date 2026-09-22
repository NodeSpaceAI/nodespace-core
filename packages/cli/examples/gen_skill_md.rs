//! Regenerates the generated regions of `packages/skill/SKILL.md` from the
//! repository's own sources of truth, so those regions cannot silently drift
//! from the code they describe.
//!
//! Two source families are covered:
//!
//! 1. **Schema rules** — [`nodespace_agent::skill_rules`], the shared
//!    constants that also render into `seed_skill_nodes()`.
//! 2. **The CLI surface** — the clap derive definitions in
//!    [`nodespace_cli::Cli`]. Every command, subcommand, argument and flag is
//!    walked from `Cli::command()`, so a command or flag added to the CLI
//!    cannot go undocumented.
//!
//! Judgment prose — what NodeSpace *is*, when to reach for it, how to install
//! it — has no derivable source and lives outside the markers, untouched by
//! regeneration. The completeness contract runs one way only: every *source*
//! must have a rendered section. It never requires that every section have a
//! source, which is what keeps hand-written prose safe.
//!
//! ## Why this lives in `nodespace-cli`, as an example
//!
//! It must read both `nodespace_agent::skill_rules` and the clap definitions.
//! The dependency graph is `nodespace-cli -> nodespace-daemon ->
//! nodespace-agent`, so an equivalent binary in `nodespace-agent` (where this
//! logic previously lived) cannot reach clap without a dependency cycle.
//! `nodespace-cli` is downstream of both, so it can see everything needed.
//!
//! It is an *example* rather than a `[[bin]]` so it is not built or shipped as
//! part of a release: `cargo build --release` builds bins, not examples. (This
//! is not about keeping `nodespace-agent` out of the binary — that already
//! reaches it transitively through `nodespace-daemon`. It is about not adding
//! a second executable to the release artifact for a dev-time tool.)
//!
//! Usage (prefer the bun wrappers, which is how the gate calls it):
//!   bun run skill:gen     # regenerate and overwrite SKILL.md
//!   bun run skill:check   # exit 1 if stale

use clap::{ArgAction, Command as ClapCommand, CommandFactory};
use nodespace_agent::skill_rules::{
    ADD_ENUM_VALUES, CREATING_TWO_LINKED_TYPES, DELETE_A_SCHEMA, EDIT_DONT_RECREATE,
    ENUM_EDGE_FIELDS, ENUM_FORMAT, FIELDS_FROM_REQUEST_ONLY, GROUPING_IS_COLLECTIONS,
    NAME_PLACEHOLDER_EXCEPTION, NO_NAME_TITLE_FIELD, ONE_SCHEMA_PER_REQUEST, RELATIONSHIP_VS_FIELD,
    RENAME_VS_RELABEL, SCHEMA_ALREADY_EXISTS, SCHEMA_VALIDATION_ERROR_RETRY,
    TARGET_TYPE_MUST_EXIST, TITLE_TEMPLATE_PLACEHOLDERS, UNIQUE_FIELD_FLAGS,
};
use nodespace_cli::Cli;
use std::env;
use std::fmt::Write as _;
use std::fs;
use std::path::PathBuf;
use std::process::ExitCode;

/// One generated region of SKILL.md: a marker id plus the function that
/// renders its body.
///
/// Adding a region here is the whole extension point — `--write` and
/// `--check` both iterate this registry, so a new region is spliced and
/// drift-checked with no further wiring.
struct GeneratedRegion {
    /// Appears in the markers as `<!-- BEGIN GENERATED: {id} ... -->`.
    id: &'static str,
    /// Path of the file holding this region, relative to `packages/skill/`.
    ///
    /// Regions are not all in `SKILL.md`: the body is kept within the spec's
    /// size recommendation by moving the CLI reference into `references/`,
    /// which the standard defines as the on-demand tier. Naming the file per
    /// region means content can move between the body and a reference file
    /// without the generator caring where it ended up.
    file: &'static str,
    /// Human-readable note spliced into the begin marker, telling a reader
    /// which source to edit instead of the file.
    source_note: &'static str,
    render: fn() -> String,
}

fn regions() -> Vec<GeneratedRegion> {
    vec![
        GeneratedRegion {
            id: "schema-rules",
            file: "references/cli.md",
            source_note: "packages/agent/src/skill_rules.rs, packages/cli/examples/gen_skill_md.rs",
            render: render_schema_rules_block,
        },
        GeneratedRegion {
            id: "cli-surface",
            file: "references/cli.md",
            source_note:
                "packages/cli/src/lib.rs (clap derive), packages/cli/examples/gen_skill_md.rs",
            render: render_cli_surface_block,
        },
        GeneratedRegion {
            id: "builtin-relationships",
            file: "references/cli.md",
            source_note: "packages/core/src/models/schema.rs (BUILTIN_RELATIONSHIP_NAMES), \
                          packages/cli/examples/gen_skill_md.rs",
            render: render_builtin_relationships_block,
        },
    ]
}

fn begin_marker(r: &GeneratedRegion) -> String {
    format!("<!-- BEGIN GENERATED: {} (see {}) -->", r.id, r.source_note)
}

fn end_marker(r: &GeneratedRegion) -> String {
    format!("<!-- END GENERATED: {} -->", r.id)
}

// ---------------------------------------------------------------------------
// Region: schema-rules
// ---------------------------------------------------------------------------

/// Renders the schema-rules block content (the text between the markers,
/// exclusive), joining rules that share a single SKILL.md paragraph.
fn render_schema_rules_block() -> String {
    // ONE_SCHEMA_PER_REQUEST, CREATING_TWO_LINKED_TYPES, SCHEMA_ALREADY_EXISTS,
    // and SCHEMA_VALIDATION_ERROR_RETRY render as four paragraphs (SKILL.md
    // separates them with a blank line), everything else is one rule per
    // paragraph. NO_NAME_TITLE_FIELD and NAME_PLACEHOLDER_EXCEPTION share the
    // "**Schema fields:**" paragraph.
    format!(
        "{one_schema_per_request}\n\n{creating_two_linked_types}\n\n\
         {schema_already_exists}\n\n{schema_validation_error_retry}\n\n\
         {edit_dont_recreate}\n\n\
         {add_enum_values}\n\n\
         {rename_vs_relabel}\n\n\
         {delete_a_schema}\n\n\
         **Schema fields:** {no_name_title_field} {name_placeholder_exception}\n\n\
         {fields_from_request_only}\n\n\
         {enum_format}\n\n\
         {relationship_vs_field} {target_type_must_exist}\n\n\
         {grouping_is_collections}\n\n\
         {enum_edge_fields}\n\n\
         {title_template_placeholders}\n\n\
         {unique_field_flags}",
        one_schema_per_request = ONE_SCHEMA_PER_REQUEST.prose,
        creating_two_linked_types = CREATING_TWO_LINKED_TYPES.prose,
        schema_already_exists = SCHEMA_ALREADY_EXISTS.prose,
        schema_validation_error_retry = SCHEMA_VALIDATION_ERROR_RETRY.prose,
        edit_dont_recreate = EDIT_DONT_RECREATE.prose,
        add_enum_values = ADD_ENUM_VALUES.prose,
        rename_vs_relabel = RENAME_VS_RELABEL.prose,
        delete_a_schema = DELETE_A_SCHEMA.prose,
        no_name_title_field = NO_NAME_TITLE_FIELD.prose,
        name_placeholder_exception = NAME_PLACEHOLDER_EXCEPTION.prose,
        fields_from_request_only = FIELDS_FROM_REQUEST_ONLY.prose,
        enum_format = ENUM_FORMAT.prose,
        relationship_vs_field = RELATIONSHIP_VS_FIELD.prose,
        target_type_must_exist = TARGET_TYPE_MUST_EXIST.prose,
        grouping_is_collections = GROUPING_IS_COLLECTIONS.prose,
        enum_edge_fields = ENUM_EDGE_FIELDS.prose,
        title_template_placeholders = TITLE_TEMPLATE_PLACEHOLDERS.prose,
        unique_field_flags = UNIQUE_FIELD_FLAGS.prose,
    )
}

// ---------------------------------------------------------------------------
// Region: cli-surface
// ---------------------------------------------------------------------------

/// Renders the complete CLI surface by walking the clap command tree.
///
/// This is the machine-checkable half of the CLI reference: every command,
/// every subcommand, every flag, with the doc comments the clap derive already
/// carries. The hand-written CLI Reference prose above it in SKILL.md keeps the
/// worked examples and the judgment calls ("read the schema first", "don't
/// invent fields") that no derive can produce.
fn render_cli_surface_block() -> String {
    let cli = Cli::command();
    let mut out = String::new();

    out.push_str(
        "Every command, subcommand, and flag below is generated from the CLI's own \
         definitions, so this list is exhaustive and cannot fall behind the binary.\n",
    );

    let globals = render_global_args(&cli);
    if !globals.is_empty() {
        out.push_str("\n**Global flags** (accepted on every command):\n\n");
        out.push_str(&globals);
    }

    for sub in visible_subcommands(&cli) {
        let _ = write!(out, "\n### `nodespace {}`\n", sub.get_name());
        if let Some(about) = about_of(sub) {
            let _ = write!(out, "\n{about}\n");
        }

        let leaves = visible_subcommands(sub);
        if leaves.is_empty() {
            let args = render_args(sub);
            if !args.is_empty() {
                out.push('\n');
                out.push_str(&args);
            }
            continue;
        }

        for leaf in leaves {
            let _ = write!(
                out,
                "\n**`nodespace {} {}`**",
                sub.get_name(),
                leaf.get_name()
            );
            match about_of(leaf) {
                Some(about) => {
                    let _ = writeln!(out, " — {about}");
                }
                None => out.push('\n'),
            }
            let args = render_args(leaf);
            if !args.is_empty() {
                out.push('\n');
                out.push_str(&args);
            }
        }
    }

    out
}

/// Subcommands a user can actually invoke — clap's auto-generated `help`
/// command and anything explicitly hidden are not part of the documented
/// surface.
fn visible_subcommands(cmd: &ClapCommand) -> Vec<&ClapCommand> {
    cmd.get_subcommands()
        .filter(|c| !c.is_hide_set() && c.get_name() != "help")
        .collect()
}

/// The command's own description, preferring the short `about`. `long_about`
/// is deliberately not used: it carries multi-paragraph setup text aimed at a
/// human reading `--help`, which would bloat a body already over its token
/// budget.
fn about_of(cmd: &ClapCommand) -> Option<String> {
    cmd.get_about().map(|s| collapse_whitespace(&s.to_string()))
}

fn render_global_args(cmd: &ClapCommand) -> String {
    render_arg_list(cmd, true)
}

fn render_args(cmd: &ClapCommand) -> String {
    render_arg_list(cmd, false)
}

/// Renders one bullet per argument. `globals_only` selects between the
/// root-level global flags and a leaf command's own arguments, so a global
/// flag is documented once at the top rather than repeated under all ~40
/// leaves.
fn render_arg_list(cmd: &ClapCommand, globals_only: bool) -> String {
    let mut out = String::new();
    for arg in cmd.get_arguments() {
        if arg.is_hide_set() {
            continue;
        }
        // `--help`/`--version` are clap's own, not part of the NodeSpace surface.
        if matches!(arg.get_id().as_str(), "help" | "version") {
            continue;
        }
        if arg.is_global_set() != globals_only {
            continue;
        }

        let is_positional = arg.get_long().is_none() && arg.get_short().is_none();
        let name = match arg.get_long() {
            Some(long) => format!("--{long}"),
            None => format!("<{}>", arg.get_id().as_str().to_uppercase()),
        };

        // A flag that takes no value renders as a bare switch; one that does
        // shows its value placeholder, so the reader can tell `--json` from
        // `--type <TYPE>` without consulting the binary. A positional already
        // renders as its own placeholder above, so it takes none — otherwise
        // it doubles up as `<ID> <ID>`.
        let takes_value =
            !is_positional && !matches!(arg.get_action(), ArgAction::SetTrue | ArgAction::SetFalse);
        let placeholder = if takes_value {
            arg.get_value_names()
                .and_then(|n| n.first().map(|v| format!(" <{v}>")))
                .unwrap_or_else(|| format!(" <{}>", arg.get_id().as_str().to_uppercase()))
        } else {
            String::new()
        };

        let help = arg
            .get_help()
            .map(|h| collapse_whitespace(&h.to_string()))
            .unwrap_or_default();

        let mut annotations = Vec::new();
        if arg.is_required_set() {
            annotations.push("required".to_string());
        }
        if let Some(env) = arg.get_env() {
            annotations.push(format!("env: `{}`", env.to_string_lossy()));
        }
        let suffix = if annotations.is_empty() {
            String::new()
        } else {
            format!(" ({})", annotations.join(", "))
        };

        let _ = if help.is_empty() {
            writeln!(out, "- `{name}{placeholder}`{suffix}")
        } else {
            writeln!(out, "- `{name}{placeholder}` — {help}{suffix}")
        };
    }
    out
}

/// Doc comments arrive from clap with hard-wrapped newlines and runs of
/// indentation. Markdown would render those as accidental line breaks, so
/// they collapse to single spaces.
fn collapse_whitespace(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

// ---------------------------------------------------------------------------
// Region: builtin-relationships
// ---------------------------------------------------------------------------

/// Renders the built-in structural relationship names from their canonical
/// definition in `nodespace-core`.
///
/// The seeded guidance the local agent reads states that these four names are
/// legal between any two records, while the skill said a relationship name
/// "must be defined on the source node's schema" — full stop. That is stricter
/// than the system actually is, so an external agent following the skill would
/// refuse a `mentions` or `member_of` edge that would have been accepted, or
/// try to declare one on a schema, where creation rejects it.
///
/// Rendering from `BUILTIN_RELATIONSHIP_NAMES` rather than restating the list
/// means the two surfaces cannot disagree again: the constant is what the
/// validator itself checks against.
fn render_builtin_relationships_block() -> String {
    let names = nodespace_core::models::schema::BUILTIN_RELATIONSHIP_NAMES
        .iter()
        .map(|n| format!("`{n}`"))
        .collect::<Vec<_>>()
        .join(", ");

    format!(
        "**Built-in relationship names.** Four names are structural and legal between any \
         two nodes without being declared on a schema: {names}. They have hardcoded \
         semantics — hierarchy, mentions, collection membership, and roles — and their own \
         UI affordances.\n\n\
         Because they share the one `relationship_type` column with schema-declared \
         relationships, a schema may **not** declare a relationship under one of these \
         names; `schema create`/`schema update` rejects it. Conversely, any *other* name \
         must be declared on the source node's schema before `relationship create` will \
         accept it. When no declared relationship fits, use `mentions`."
    )
}

// ---------------------------------------------------------------------------
// Splicing
// ---------------------------------------------------------------------------

/// Splices `block` between the markers for `region` in `source`, replacing
/// whatever was there before. Returns an error if the markers aren't found,
/// are duplicated, or are malformed — that indicates SKILL.md itself was
/// edited in a way that broke the generation contract and needs a human to
/// look at it, not a silent no-op or a splice into the wrong occurrence.
fn splice_generated_block(
    source: &str,
    region: &GeneratedRegion,
    block: &str,
) -> Result<String, String> {
    let begin = begin_marker(region);
    let end = end_marker(region);
    let file = region.file;

    let begin_idx = source
        .find(&begin)
        .ok_or_else(|| format!("{file} is missing the marker: {begin}"))?;
    if source[begin_idx + begin.len()..].contains(&begin) {
        return Err(format!(
            "{file} has more than one occurrence of the begin marker: {begin}"
        ));
    }
    let after_begin = begin_idx + begin.len();
    let end_idx = source[after_begin..]
        .find(&end)
        .ok_or_else(|| format!("{file} is missing the marker: {end}"))?
        + after_begin;
    if source[end_idx + end.len()..].contains(&end) {
        return Err(format!(
            "{file} has more than one occurrence of the end marker: {end}"
        ));
    }

    Ok(format!(
        "{prefix}{begin}\n{block}\n{end}{suffix}",
        prefix = &source[..begin_idx],
        suffix = &source[end_idx + end.len()..],
    ))
}

/// `packages/skill/`, which every region's `file` is relative to.
fn skill_pkg_dir() -> PathBuf {
    // CARGO_MANIFEST_DIR is packages/cli — the skill package is its sibling.
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../skill")
}

/// One file's worth of work: what is on disk now, and what it should be once
/// every region targeting it has been spliced in.
struct FileOutcome {
    path: PathBuf,
    relative: &'static str,
    current: String,
    regenerated: String,
}

/// Reads each file a region targets and applies every region belonging to it.
///
/// Regions are grouped by file so a file is read and written once no matter
/// how many regions it carries.
fn compute() -> Result<Vec<FileOutcome>, String> {
    let dir = skill_pkg_dir();
    let mut outcomes: Vec<FileOutcome> = Vec::new();

    for region in regions() {
        let idx = match outcomes.iter().position(|o| o.relative == region.file) {
            Some(i) => i,
            None => {
                let path = dir.join(region.file);
                let current = fs::read_to_string(&path)
                    .map_err(|e| format!("failed to read {}: {e}", path.display()))?;
                outcomes.push(FileOutcome {
                    path,
                    relative: region.file,
                    regenerated: current.clone(),
                    current,
                });
                outcomes.len() - 1
            }
        };
        let block = (region.render)();
        outcomes[idx].regenerated =
            splice_generated_block(&outcomes[idx].regenerated, &region, &block)?;
    }

    Ok(outcomes)
}

fn main() -> ExitCode {
    let mode = env::args().nth(1);

    let outcomes = match compute() {
        Ok(o) => o,
        Err(e) => {
            eprintln!("{e}");
            return ExitCode::FAILURE;
        }
    };

    match mode.as_deref() {
        Some("--write") => {
            for o in &outcomes {
                if o.current == o.regenerated {
                    continue;
                }
                if let Err(e) = fs::write(&o.path, &o.regenerated) {
                    eprintln!("failed to write {}: {e}", o.path.display());
                    return ExitCode::FAILURE;
                }
                println!("Regenerated {}", o.relative);
            }
            println!("Skill content is up to date.");
            ExitCode::SUCCESS
        }
        Some("--check") => {
            let stale: Vec<&str> = outcomes
                .iter()
                .filter(|o| o.current != o.regenerated)
                .map(|o| o.relative)
                .collect();
            if stale.is_empty() {
                println!("Skill content is up to date.");
                ExitCode::SUCCESS
            } else {
                eprintln!(
                    "stale generated content in: {}\n\
                     A generated region no longer matches its source \
                     (packages/agent/src/skill_rules.rs, or the clap definitions in \
                     packages/cli/src/lib.rs). Run `bun run skill:gen` and commit the result.",
                    stale.join(", ")
                );
                ExitCode::FAILURE
            }
        }
        _ => {
            eprintln!("Usage: gen_skill_md --check | --write");
            ExitCode::FAILURE
        }
    }
}
