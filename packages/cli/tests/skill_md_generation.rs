//! Drift and completeness guards for `packages/skill/SKILL.md`.
//!
//! The skill is what an external agent (Claude Code, Codex, Antigravity CLI,
//! OpenCode) reads to learn what NodeSpace is and how to drive it. Anything it
//! restates by hand can fall out of step with the code, and nothing catches it
//! — the agent simply acts on stale instructions. These tests make each source
//! of truth structurally responsible for its own coverage.
//!
//! ## The contract runs one way
//!
//! Every *source* must have a rendered section. It is **never** asserted that
//! every section has a source. That asymmetry is deliberate and load-bearing:
//! judgment prose — what NodeSpace is, when to reach for it, how to install it
//! — has no upstream to render from and must survive regeneration untouched. A
//! test demanding the reverse would delete exactly the content a generator
//! cannot produce.
//!
//! ## Why enumeration is dynamic
//!
//! Coverage is computed by walking `Cli::command()` and `seed_skill_nodes()`
//! at test time, not by comparing against a hand-maintained list of names. A
//! checked-in list is itself a second representation that drifts, and it fails
//! in the worst direction — silently, by omission. This mirrors the existing
//! precedent in `agent_guidance.rs`'s `guidance_corpus()`.

use clap::{Command as ClapCommand, CommandFactory};
use nodespace_cli::Cli;
use std::path::PathBuf;

/// Every markdown file the skill ships, concatenated.
///
/// Coverage is asserted against the whole skill folder, not `SKILL.md` alone.
/// The body is kept within the standard's size recommendation by moving the CLI
/// reference into `references/`, which the spec defines as the on-demand tier
/// and which is portable across every target. Content that moves between the
/// two tiers is still shipped and still reachable, so a check that only read
/// `SKILL.md` would report false drift the moment anything was moved.
fn skill_md() -> String {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../skill");
    let mut combined = String::new();

    let read = |p: &PathBuf| -> String {
        std::fs::read_to_string(p).unwrap_or_else(|e| panic!("failed to read {}: {e}", p.display()))
    };

    combined.push_str(&read(&dir.join("SKILL.md")));

    let refs = dir.join("references");
    let mut entries: Vec<PathBuf> = std::fs::read_dir(&refs)
        .unwrap_or_else(|e| panic!("failed to read {}: {e}", refs.display()))
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "md"))
        .collect();
    // Sorted so the concatenation is deterministic regardless of directory order.
    entries.sort();
    assert!(
        !entries.is_empty(),
        "no reference files found in {} — the CLI reference is expected to live there",
        refs.display()
    );
    for path in entries {
        combined.push('\n');
        combined.push_str(&read(&path));
    }

    combined
}

/// The shipped skill with every generated region stripped out — i.e. only the
/// prose a human wrote.
///
/// Some assertions are about whether a *human* explained something, and those
/// must not be satisfiable by generated content. The generated CLI surface
/// contains every command string by construction, so any needle shaped like a
/// command is trivially present in it: an assertion checking the full corpus
/// for `nodespace node delete` passes even if every word of hand-written
/// guidance were deleted. That is the same "cannot fail" defect as searching
/// the whole corpus for a colliding flag name — and it lands on the one test
/// whose subject is precisely the judgment prose this file's one-way contract
/// exists to protect.
fn skill_prose_only() -> String {
    let full = skill_md();
    let mut out = String::new();
    let mut rest = full.as_str();

    // Regions are delimited by `<!-- BEGIN GENERATED: ... -->` / `<!-- END
    // GENERATED: ... -->`. Drop everything between each pair, keeping the prose
    // around them.
    while let Some(begin) = rest.find("<!-- BEGIN GENERATED:") {
        out.push_str(&rest[..begin]);
        let after = &rest[begin..];
        match after.find("<!-- END GENERATED:") {
            Some(end_start) => {
                let tail = &after[end_start..];
                let end_len = tail
                    .find("-->")
                    .map(|i| i + "-->".len())
                    .unwrap_or(tail.len());
                rest = &tail[end_len..];
            }
            // An unterminated begin marker means the file is malformed; the
            // generator's own splice would have rejected it. Stop here rather
            // than silently keeping generated content.
            None => {
                rest = "";
                break;
            }
        }
    }
    out.push_str(rest);
    out
}

/// Subcommands a user can actually invoke — clap's generated `help` and
/// anything hidden are not part of the documented surface. Mirrors the
/// generator's filter.
fn visible(cmd: &ClapCommand) -> Vec<&ClapCommand> {
    cmd.get_subcommands()
        .filter(|c| !c.is_hide_set() && c.get_name() != "help")
        .collect()
}

/// Every command and subcommand reachable from the clap root appears in
/// SKILL.md.
///
/// This is the test that closes the drift where `session`, `uninstall`, and
/// `model` had zero occurrences in a hand-written CLI reference. Because the
/// expected set is derived from the parser itself,
/// a command added later is covered with no change here.
#[test]
fn every_cli_command_is_documented() {
    let skill = skill_md();
    let cli = Cli::command();
    let mut missing = Vec::new();

    for sub in visible(&cli) {
        let leaves = visible(sub);
        if leaves.is_empty() {
            let needle = format!("nodespace {}", sub.get_name());
            if !skill.contains(&needle) {
                missing.push(needle);
            }
            continue;
        }
        for leaf in leaves {
            let needle = format!("nodespace {} {}", sub.get_name(), leaf.get_name());
            if !skill.contains(&needle) {
                missing.push(needle);
            }
        }
    }

    assert!(
        missing.is_empty(),
        "SKILL.md does not document these CLI commands: {missing:#?}\n\
         Run `bun run skill:gen` and commit the result."
    );
}

/// Every non-global, non-hidden flag on every leaf command appears in
/// SKILL.md.
///
/// Commands were the coarse drift; flags were the fine drift — the whole
/// `import dir` flag set (`--exclude`,
/// `--include-agent-files`, `--include-hidden`, `--no-recursive`, `--replace`,
/// `--collection`, `--use-filename-as-title`) and `node query --id` as
/// undocumented while their parent commands were present. A flag an agent
/// cannot see is a capability it will not use.
#[test]
fn every_cli_flag_is_documented() {
    let skill = skill_md();
    let cli = Cli::command();
    let mut missing = Vec::new();

    fn check(cmd: &ClapCommand, path: &str, skill: &str, missing: &mut Vec<String>) {
        // Scope the search to this command's own section rather than the whole
        // corpus. A bare `skill.contains("--limit")` passes as soon as ANY
        // command documents `--limit`, so a genuinely undocumented flag whose
        // name collides with an existing one slips through — and collisions are
        // the common case here (`--limit`, `--json`, `--collection` each appear
        // under many commands). Slicing to the section makes the assertion
        // answer the question it claims to.
        // A command with no section of its own is reported here rather than
        // silently yielding an empty slice. `every_cli_command_is_documented`
        // does NOT cover this case: it searches for the bare string
        // "nodespace <path>" anywhere in the corpus, which a worked example or
        // a prose mention satisfies without any generated section existing.
        let Some(section) = command_section(skill, path) else {
            missing.push(format!("{path} (no generated section at all)"));
            return;
        };

        for arg in cmd.get_arguments() {
            if arg.is_hide_set() || arg.is_global_set() {
                continue;
            }
            if matches!(arg.get_id().as_str(), "help" | "version") {
                continue;
            }
            let Some(long) = arg.get_long() else {
                continue; // positionals are covered by the command test
            };
            let needle = format!("--{long}");
            if !section.contains(&needle) {
                missing.push(format!("{path} {needle}"));
            }
        }
    }

    for sub in visible(&cli) {
        let leaves = visible(sub);
        if leaves.is_empty() {
            check(sub, sub.get_name(), &skill, &mut missing);
            continue;
        }
        for leaf in leaves {
            let path = format!("{} {}", sub.get_name(), leaf.get_name());
            check(leaf, &path, &skill, &mut missing);
        }
    }

    assert!(
        missing.is_empty(),
        "these CLI flags are not documented under their own command's section: \
         {missing:#?}\nRun `bun run skill:gen` and commit the result."
    );
}

/// The slice of the generated surface belonging to `nodespace <path>`, from its
/// heading to the next heading.
///
/// The generator emits a leaf as `**`nodespace node query`**` and a
/// subcommand-less command as `### `nodespace search``, so both forms are
/// tried. Both markers are backtick-delimited, so a command whose name is a
/// prefix of another (`node get` vs `node get-related`) does not match the
/// wrong section.
///
/// Returns `None` when the command has no generated section at all; the caller
/// reports that as its own failure.
fn command_section<'a>(skill: &'a str, path: &str) -> Option<&'a str> {
    let heading = format!("**`nodespace {path}`**");
    let alt = format!("### `nodespace {path}`\n");
    let (start, marker_len) = match skill.find(&heading) {
        Some(i) => (i, heading.len()),
        None => (skill.find(&alt)?, alt.len()),
    };

    let rest = &skill[start + marker_len..];
    // Either form of heading ends the section: the next leaf, or the next
    // command group.
    let end = [rest.find("\n**`nodespace "), rest.find("\n### ")]
        .into_iter()
        .flatten()
        .min()
        .unwrap_or(rest.len());
    Some(&rest[..end])
}

/// Every global flag is documented once.
#[test]
fn every_global_cli_flag_is_documented() {
    let skill = skill_md();
    let cli = Cli::command();
    let mut missing = Vec::new();

    for arg in cli.get_arguments() {
        if arg.is_hide_set() || !arg.is_global_set() {
            continue;
        }
        if let Some(long) = arg.get_long() {
            let needle = format!("--{long}");
            if !skill.contains(&needle) {
                missing.push(needle);
            }
        }
    }

    assert!(
        missing.is_empty(),
        "SKILL.md does not document these global flags: {missing:#?}"
    );
}

/// Every seeded skill is represented in SKILL.md.
///
/// `seed_skill_nodes()` defines **eight** skills, not
/// the six built from named `*_guidance()` functions: **Research & Search** and
/// **Node Creation** are inline raw strings with no builder function. Any check
/// keyed on function names would silently skip both — and Research & Search is
/// the largest body of the set, and the un-generated source of SKILL.md's own
/// Tool Decision Guide.
///
/// Enumerating `seed_skill_nodes()` directly is what makes that impossible: a
/// skill defined any way at all is a `NodeTemplate` in the returned vector.
#[test]
fn every_seeded_skill_is_represented() {
    // Prose only. The generated CLI surface contains every command string by
    // construction, so checking the full corpus would pass even if all the
    // hand-written guidance were deleted — this test would be measuring the
    // generator instead of the thing it is named for.
    let skill = skill_prose_only().to_lowercase();
    let mut missing = Vec::new();

    for template in nodespace_agent::skill_pipeline::seed_skill_nodes() {
        // The mapping below is hand-written, and unavoidably so: the two sides
        // share no identifier. A seeded skill's `tool_whitelist` names internal
        // tools (`create_node`, `search_semantic`) that appear nowhere in the
        // skill, because the CLI is the external surface — so there is nothing
        // to derive a needle from. What is NOT hand-maintained is the *list of
        // skills*, which comes from `seed_skill_nodes()`; a skill added later
        // hits the fallback arm and fails, rather than being silently omitted.
        //
        // Needles are full CLI invocations rather than bare nouns. A needle
        // like "search" matches dozens of unrelated places and would pass for a
        // skill whose commands were entirely absent, which is the failure mode
        // this test exists to prevent.
        let covered = match template.title.as_str() {
            "Research & Search" => {
                skill.contains("nodespace search") && skill.contains("nodespace node query")
            }
            "Node Creation" => skill.contains("nodespace node create"),
            "Schema Creation" => {
                skill.contains("nodespace schema create") && skill.contains("nodespace schema list")
            }
            "Graph Editing" => {
                skill.contains("nodespace node update")
                    && skill.contains("nodespace node set-status")
            }
            "Relationship Management" => {
                skill.contains("nodespace relationship create")
                    && skill.contains("nodespace relationship get")
            }
            "Node Deletion" => skill.contains("nodespace node delete"),
            "Conflict Resolution" => {
                skill.contains("nodespace conflicts list")
                    && skill.contains("nodespace conflicts dismiss")
            }
            "Node Merge" => skill.contains("nodespace conflicts merge"),
            "Bulk Import" => skill.contains("nodespace import"),
            "Organization" => skill.contains("member_of"),
            // A skill added later has no arm here and fails loudly, which is
            // the intent: someone must decide how it surfaces to an external
            // agent rather than have it silently omitted.
            other => {
                missing.push(format!("{other} (no coverage rule defined)"));
                true
            }
        };
        if !covered {
            missing.push(template.title.clone());
        }
    }

    assert!(
        missing.is_empty(),
        "these seeded skills have no counterpart in SKILL.md: {missing:#?}\n\
         Every skill the local agent is taught should be reachable by an \
         external agent too, or explicitly justified as internal-only."
    );
}

/// Every built-in structural relationship name is documented.
///
/// The skill used to say a relationship name must be defined on the source
/// node's schema, full stop — stricter than the system is. Four names are legal
/// between any two nodes without a declaration, and the local agent's own
/// guidance says so, so the two surfaces disagreed. Enumerating the constant
/// the validator checks against means a name added to it cannot be omitted.
#[test]
fn every_builtin_relationship_name_is_documented() {
    let skill = skill_md();
    let missing: Vec<&str> = nodespace_core::models::schema::BUILTIN_RELATIONSHIP_NAMES
        .iter()
        .copied()
        .filter(|name| !skill.contains(name))
        .collect();

    assert!(
        missing.is_empty(),
        "the shipped skill does not mention these built-in relationship names: \
         {missing:#?}\nRun `bun run skill:gen` and commit the result."
    );
}

/// The SKILL.md body stays within the Agent Skills size recommendations.
///
/// The body is loaded in full the moment the skill activates, so its size is a
/// per-activation cost paid by every agent on every matching task — which is
/// what the standard's guidance (≤500 lines, <5000 tokens) is about. Detail
/// belongs in `references/`, loaded only when actually needed.
///
/// This guard exists because the file has already crossed the line once: it
/// was 534 lines before the CLI surface was generated into it, and generating
/// the previously-missing commands pushed it to 786. Without a test, the next
/// addition repeats that quietly.
#[test]
fn skill_md_body_is_within_spec_size_recommendations() {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../skill/SKILL.md");
    let body = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("failed to read {}: {e}", path.display()));

    const MAX_LINES: usize = 500;
    let lines = body.lines().count();
    assert!(
        lines <= MAX_LINES,
        "SKILL.md body is {lines} lines, over the {MAX_LINES}-line recommendation. \
         Move detail into packages/skill/references/ rather than growing the body."
    );

    // ~4 chars/token is the usual English approximation; the spec's limit is
    // advisory, so an approximation is the right instrument. Being wrong by a
    // few percent does not change whether a 500-line file is acceptable.
    const MAX_TOKENS: usize = 5000;
    let approx_tokens = body.chars().count() / 4;
    assert!(
        approx_tokens < MAX_TOKENS,
        "SKILL.md body is ~{approx_tokens} tokens, over the {MAX_TOKENS}-token recommendation. \
         Move detail into packages/skill/references/ rather than growing the body."
    );
}

/// Reference files are reachable from the body.
///
/// A reference nothing points at is a file an agent never opens. The standard's
/// progressive disclosure only works if the body names what to load.
#[test]
fn every_reference_file_is_linked_from_the_body() {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../skill");
    let body = std::fs::read_to_string(dir.join("SKILL.md")).expect("failed to read SKILL.md");

    let mut unlinked = Vec::new();
    for entry in std::fs::read_dir(dir.join("references")).expect("failed to read references/") {
        let path = entry.expect("bad dir entry").path();
        if path.extension().is_some_and(|x| x == "md") {
            let name = path.file_name().unwrap().to_string_lossy().to_string();
            if !body.contains(&format!("references/{name}")) {
                unlinked.push(name);
            }
        }
    }

    assert!(
        unlinked.is_empty(),
        "these reference files are never mentioned in SKILL.md, so an agent will \
         not know to read them: {unlinked:#?}"
    );
}

/// The Preflight Check's MCP passthrough branch (Branch 2) names the actual
/// tool the `nodespace mcp` server exposes.
///
/// Unlike the CLI surface and schema rules, Branch 2's prose has no
/// generator behind it — it is hand-written, same as the rest of the
/// Preflight Check, because it is judgment prose rather than a derivable
/// listing (see this file's module doc comment). That means nothing
/// automatically catches the tool name drifting away from
/// `packages/cli/src/commands/mcp.rs`'s own `TOOL_NAME` constant if it is
/// ever renamed. This test is that catch: it reads the constant directly
/// rather than hand-typing a second copy of it.
#[test]
fn skill_md_mcp_branch_names_the_real_passthrough_tool() {
    let skill = skill_md();
    let tool_name = nodespace_cli::commands::mcp::TOOL_NAME;
    let needle = format!("`{tool_name}`");
    assert!(
        skill.contains(&needle),
        "SKILL.md's Preflight Check (Branch 2) does not reference the MCP \
         passthrough's actual tool name ({needle}, from mcp.rs's TOOL_NAME \
         constant) — update the Preflight Check section."
    );
}

/// Branch 2 documents the MCP passthrough's real dispatch timeout and the
/// streaming commands it cannot support, not an invented number.
///
/// `nodespace mcp` kills and reports as a timeout any dispatched command
/// that outlives `DISPATCH_TIMEOUT` — in practice, `session launch`/`session
/// attach`, which stream a PTY and would otherwise block the server's
/// single-threaded request loop forever (see `mcp.rs`'s own doc comment).
/// If that constant is ever tuned, this test forces the doc's numeral to be
/// re-checked rather than silently going stale.
#[test]
fn skill_md_mcp_branch_documents_the_real_dispatch_timeout() {
    let skill = skill_md();
    let timeout_secs = nodespace_cli::commands::mcp::DISPATCH_TIMEOUT.as_secs();
    let needle = format!("{timeout_secs}s");
    assert!(
        skill.contains(&needle),
        "SKILL.md's Preflight Check (Branch 2) does not mention the MCP \
         passthrough's actual dispatch timeout ({needle}, from mcp.rs's \
         DISPATCH_TIMEOUT constant) — update the Preflight Check section."
    );
    assert!(
        skill.contains("session launch") && skill.contains("session attach"),
        "SKILL.md's Preflight Check (Branch 2) should name the streaming \
         commands (session launch / session attach) that the MCP passthrough \
         cannot support, matching mcp.rs's own timeout behavior."
    );
}

/// The checked-in SKILL.md matches what the generator produces.
///
/// This is the same guarantee `bun run skill:check` gives in the pre-push
/// gate, asserted here too so `cargo test` alone catches drift — a contributor
/// editing `skill_rules.rs` or a clap doc comment sees the failure without
/// needing to remember a separate command.
#[test]
fn checked_in_skill_md_is_up_to_date() {
    let status = std::process::Command::new(env!("CARGO"))
        .args([
            "run",
            "-q",
            "-p",
            "nodespace-cli",
            "--example",
            "gen_skill_md",
            "--",
            "--check",
        ])
        .current_dir(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../.."))
        .status()
        .expect("failed to run the gen_skill_md example");

    assert!(
        status.success(),
        "packages/skill/SKILL.md is stale — run `bun run skill:gen` and commit the result"
    );
}

/// Every rule registered in `SCHEMA_RULES` must actually reach the shipped
/// skill content.
///
/// `render_schema_rules_block` interpolates each rule by name into a hand-written
/// format string, so registering a rule in the `SCHEMA_RULES` array is NOT
/// enough to publish it — a rule the template forgets to name is silently
/// dropped, and `checked_in_skill_md_is_up_to_date` still passes because the
/// generator and the checked-in copy agree on the same incomplete output. This
/// test is the guard for that gap: it compares against the array, which is the
/// list a new rule is naturally added to.
///
/// Matched on a distinctive prefix rather than the whole prose because the
/// template joins some rules into a shared paragraph (e.g.
/// `RELATIONSHIP_VS_FIELD` and `TARGET_TYPE_MUST_EXIST`), so an exact
/// whole-string match against the rendered file is not a property that holds.
#[test]
fn every_schema_rule_reaches_the_skill() {
    let skill = skill_md();
    let mut missing = Vec::new();

    for rule in nodespace_agent::skill_rules::SCHEMA_RULES {
        // The lead sentence is what identifies a rule in the rendered output,
        // and is stable against the paragraph-joining above.
        let needle: String = rule.prose.chars().take(60).collect();
        if !skill.contains(&needle) {
            missing.push(rule.id);
        }
    }

    assert!(
        missing.is_empty(),
        "schema rules registered in SCHEMA_RULES but absent from the shipped skill: {}. \
         Add each to the format string in `render_schema_rules_block` \
         (packages/cli/examples/gen_skill_md.rs), then regenerate.",
        missing.join(", ")
    );
}
