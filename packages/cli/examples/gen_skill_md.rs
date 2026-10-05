//! Regenerates the generated regions of `packages/skill/SKILL.md` from the
//! repository's own sources of truth, so those regions cannot silently drift
//! from the code they describe.
//!
//! Two source families are covered:
//!
//! 1. **Schema rules** — [`nodespace_agent::skill_rules`], the shared rule
//!    fragments the seeded skills also include.
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
            source_note: "packages/agent/src/seeds/rules/skill-md/, \
                          packages/agent/src/seeds/skill-md/schema-rules.md",
            render: nodespace_agent::skill_rules::skill_md_schema_rules,
        },
        GeneratedRegion {
            id: "play-rules",
            file: "references/cli.md",
            source_note: "packages/agent/src/seeds/rules/skill-md/, \
                          packages/agent/src/seeds/skill-md/play-rules.md",
            render: nodespace_agent::skill_rules::skill_md_play_rules,
        },
        GeneratedRegion {
            id: "relationship-rules",
            file: "references/cli.md",
            source_note:
                "packages/agent/src/seeds/rules/skill-md/relationship-direction.md, \
                          packages/agent/src/seeds/rules/skill-md/relationship-reverse-traversal.md",
            render: nodespace_agent::skill_rules::skill_md_relationship_rules,
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
        GeneratedRegion {
            id: "linear-playbook",
            file: "references/linear-playbook.md",
            source_note: "packages/core/src/methodology/linear.rs, \
                          packages/core/src/methodology/skills/linear/, \
                          packages/cli/examples/gen_skill_md.rs",
            render: || render_playbook_block("linear", "linear"),
        },
        GeneratedRegion {
            id: "spec-driven-playbook",
            file: "references/spec-driven-playbook.md",
            source_note: "packages/core/src/methodology/spec_driven.rs, \
                          packages/core/src/methodology/skills/spec_driven/, \
                          packages/cli/examples/gen_skill_md.rs",
            render: || render_playbook_block("spec-driven", "spec_driven"),
        },
        GeneratedRegion {
            id: "jira-playbook",
            file: "references/jira-playbook.md",
            source_note: "packages/core/src/methodology/jira.rs, \
                          packages/core/src/methodology/skills/jira/, \
                          packages/cli/examples/gen_skill_md.rs",
            render: || render_playbook_block("jira", "jira"),
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
//
// Rendered by `nodespace_agent::skill_rules::skill_md_schema_rules`: the
// schema rules in their prose form, laid out by
// `packages/agent/src/seeds/skill-md/schema-rules.md`. The seeded skills
// include the same rules in their local-agent form, so neither surface
// restates a rule.
//
// The `play-rules` region is rendered the same way, by
// `skill_md_play_rules` from `seeds/skill-md/play-rules.md`, and the
// `relationship-rules` region by `skill_md_relationship_rules` from the two
// relationship rules the Relationship Management skill includes.

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

        // A command's own arguments come first: a command that takes them
        // and has subcommands too (`query`, `query run`) is run either way.
        let args = render_args(sub);
        if !args.is_empty() {
            out.push('\n');
            out.push_str(&args);
        }

        for leaf in visible_subcommands(sub) {
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
        // `--help`/`--version` are clap's own, not part of the NodeSpace
        // surface. Matched by action, not id: a command's own `--version`
        // flag (`node delete`) is part of it.
        if matches!(
            arg.get_action(),
            clap::ArgAction::Help
                | clap::ArgAction::HelpShort
                | clap::ArgAction::HelpLong
                | clap::ArgAction::Version
        ) {
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
// Regions: one per methodology playbook
// ---------------------------------------------------------------------------

/// Renders the playbook `id` as the CLI calls that reproduce it. `skills_dir`
/// names its skill sources under `packages/core/src/methodology/skills/`.
///
/// The playbook itself is typed Rust in `nodespace_core::methodology`, executed
/// in-process by the desktop app. An external agent has no access to that, so
/// this restates the same content as literal `nodespace` verbs — the transport
/// every external agent already has, whether over a shell or the MCP
/// passthrough.
///
/// Generated rather than hand-written because the two would drift, and the
/// failure would be silent in the worst way: an agent following a stale doc
/// installs a schema subtly different from the one the GUI installs, and the
/// two workspaces diverge with nothing to flag it.
///
/// Plays are rendered as `node create --type play`. There is no
/// `playbook create` verb — a Play is an ordinary node whose `rules` property
/// the engine reads — and inventing one in the docs would send an agent at a
/// command that does not exist.
fn render_playbook_block(id: &str, skills_dir: &str) -> String {
    use nodespace_core::methodology::playbook_by_id;

    let playbook =
        playbook_by_id(id).unwrap_or_else(|| panic!("the {id} playbook ships with this build"));
    let mut out = String::new();

    let _ = writeln!(out, "## {}\n", playbook.name);
    let _ = writeln!(out, "{}\n", playbook.description);
    let _ = writeln!(
        out,
        "Run these in order. Each step depends on the ones before it: a Play whose trigger \
         names a type is rejected until that type's schema exists, so a re-ordered sequence \
         fails rather than half-installing.\n"
    );

    let _ = writeln!(out, "### 1. Schemas\n");
    for step in &playbook.schemas {
        let _ = writeln!(out, "Create `{}`:\n", step.schema_id);
        let _ = writeln!(
            out,
            "```bash\nnodespace schema create --params '{}'\n```\n",
            compact_json(&step.params)
        );
    }

    let _ = writeln!(out, "### 2. Schema extensions\n");
    if playbook.field_value_extensions.is_empty() {
        let _ = writeln!(
            out,
            "None — this playbook adds no fields or values to an existing schema.\n"
        );
    }
    if playbook
        .field_value_extensions
        .iter()
        .any(|e| !e.adds_field())
    {
        let _ = writeln!(
            out,
            "A vocabulary extension appends values to a field. When the field is **inherited** \
             from a base type, every new value carries `mapsTo` naming the base value it \
             collapses to when something reading at the base scope looks at it. Without that a \
             base-scoped Play or query would meet a value it has never heard of.\n"
        );
    }
    for ext in &playbook.field_value_extensions {
        if ext.adds_field() {
            let _ = writeln!(out, "Add `{}` to `{}`:\n", ext.field, ext.schema_id);
        } else {
            let _ = writeln!(out, "Extend `{}.{}`:\n", ext.schema_id, ext.field);
        }
        let _ = writeln!(
            out,
            "```bash\nnodespace schema update --params '{}'\n```\n",
            compact_json(&ext.params)
        );
    }

    let _ = writeln!(out, "### 3. Plays\n");
    let _ = writeln!(
        out,
        "A Play is a node of type `play` carrying a `rules` property. Its rules are \
         validated on write — conditions are CEL-compiled and every referenced type and \
         path is checked — so a malformed Play is refused here, not at execution time.\n"
    );
    for play in &playbook.plays {
        let _ = writeln!(out, "**{}** — {}\n", play.name, play.description);
        let _ = writeln!(
            out,
            "```bash\nnodespace node create --type play --content '{}' \\\n  --properties '{}'\n```\n",
            play.name,
            compact_json(&play.properties())
        );
    }

    let _ = writeln!(out, "### 4. Guidance skills\n");
    let _ = writeln!(
        out,
        "Skill nodes carrying usage guidance, discovered through the ordinary skill-search \
         mechanism. Each is a `skill` root node whose markdown body becomes ordinary child \
         nodes. Deliberately several narrow skills rather than one broad one: retrieval \
         scores a precise match far better than a skill diluted across every intent.\n"
    );
    let _ = writeln!(
        out,
        "Create each as below, then add its guidance as markdown children. The properties \
         carry the whole retrieval surface — `description`, and an `exclusion` where the \
         skill has one, which keeps general requests from ranking it above a built-in — so \
         set them verbatim. The bodies are long-form prose; read them from \
         `packages/core/src/methodology/skills/{skills_dir}/` rather than reproducing them \
         here.\n"
    );
    let _ = writeln!(
        out,
        "After creating a skill, link it to the types it is about with an `applies_to` edge \
         to each type's schema, as shown under it. Skill search then hands an agent the skill \
         together with exactly those types' definitions. If a type landed under another id \
         (step 1 reported a re-key), link to that id.\n"
    );
    for skill in playbook.skills {
        let _ = writeln!(out, "**{}** — {}\n", skill.title, skill.description);
        let _ = writeln!(
            out,
            "```bash\nnodespace node create --type skill --content '{}' \\\n  --properties '{}'\n{}```\n",
            shell_single_quote_body(skill.title),
            compact_json(&skill.template().root_properties),
            applies_to_commands(skill)
        );
    }

    let _ = writeln!(out, "### 5. Saved views\n");
    let _ = writeln!(
        out,
        "A saved view is a node of type `query`. Its properties carry both the query \
         (`target_type`, `filters`, `sorting`) and how it renders (`view_config`), so the \
         board opens as authored with no per-user setup.\n"
    );
    for view in &playbook.views {
        let _ = writeln!(out, "**{}**\n", view.name);
        let _ = writeln!(
            out,
            "```bash\nnodespace node create --type query --content '{}' \\\n  --properties '{}'\n```\n",
            view.name,
            compact_json(&view.properties())
        );
    }

    let _ = writeln!(out, "### 6. Workspace skill\n");
    let overview = &playbook.overview;
    let _ = writeln!(
        out,
        "One more skill, created after everything else because it names what you created. \
         Title it exactly as shown: it is how an agent in this workspace later recognizes \
         the Playbook is installed.\n\n**{}** — {}\n\nCreate it the same way as the \
         guidance skills, and end its body with an \"Installed in this workspace\" section \
         listing the types, Plays, guidance skills and views above, by id. Link it to the \
         Playbook's types:\n\n```bash\n{}```",
        overview.title,
        overview.description,
        applies_to_commands(overview)
    );
    out.trim_end().to_string()
}

/// One `relationship create` line per schema `skill` is about, each ending in
/// a newline: the `applies_to` edges an install creates for it.
fn applies_to_commands(skill: &nodespace_core::methodology::skills::PlaybookSkill) -> String {
    skill
        .applies_to
        .iter()
        .map(|schema_id| {
            format!(
                "nodespace relationship create --from <skill-id> --type {} --to {schema_id}\n",
                nodespace_core::models::SKILL_APPLIES_TO
            )
        })
        .collect()
}

/// Serializes `value` for embedding in a single-quoted shell argument.
///
/// Compact rather than pretty-printed: a `--params` payload is one shell word,
/// and embedded newlines would break the surrounding quoting.
///
/// Single quotes inside the payload are escaped as `'\''` — close, escaped
/// literal, reopen — which is the only way to get one into a single-quoted
/// POSIX word. This is load-bearing rather than defensive: a Play's CEL
/// conditions contain string literals (`node.status == 'done'`), and an
/// unescaped quote there terminates the surrounding argument, leaving an
/// agent copy-pasting the command with a mangled write rather than a parse
/// error it could notice.
fn compact_json(value: &serde_json::Value) -> String {
    let json = serde_json::to_string(value).expect("playbook JSON is serializable");
    shell_single_quote_body(&json)
}

/// Escape `s` for use inside a single-quoted shell word.
fn shell_single_quote_body(s: &str) -> String {
    s.replace('\'', r"'\''")
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
                // Name the sources of the regions that actually went stale,
                // read from their own `source_note`, rather than a fixed list
                // — a hardcoded one sends whoever hits this at the wrong file
                // as soon as a region is added.
                let sources: Vec<&str> = regions()
                    .iter()
                    .filter(|r| stale.contains(&r.file))
                    .map(|r| r.source_note)
                    .collect();
                eprintln!(
                    "stale generated content in: {}\n\
                     A generated region no longer matches its source ({}). \
                     Run `bun run skill:gen` and commit the result.",
                    stale.join(", "),
                    sources.join("; ")
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
