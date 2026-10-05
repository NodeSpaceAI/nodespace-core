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
//! Coverage is computed by walking `Cli::command()` and `SKILL_SEEDS` at test
//! time, not by comparing against a hand-maintained list of names. A
//! checked-in list is itself a second representation that drifts, and it fails
//! in the worst direction — silently, by omission. This mirrors the existing
//! precedent in `agent_guidance.rs`'s `guidance_corpus()`.
//!
//! ## What the body carries
//!
//! `SKILL.md` says what NodeSpace is and how to fetch its instructions. The
//! procedures themselves are the skill nodes, which an agent fetches with
//! `nodespace skill guidance` and which come back written in CLI commands. So
//! the guards here are that the body teaches the fetch, and that what a fetch
//! serves names commands the CLI has.

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

/// The `nodespace …` invocations a piece of guidance shows in code spans,
/// each as the words that follow `nodespace` up to its first flag or
/// placeholder: `nodespace node update <id> --content …` is `["node",
/// "update"]`.
fn cited_commands(text: &str) -> Vec<Vec<String>> {
    text.split('`')
        // Every second segment sits between a pair of backticks.
        .skip(1)
        .step_by(2)
        .filter_map(|span| span.strip_prefix("nodespace "))
        .map(|rest| {
            rest.split_whitespace()
                .take_while(|word| word.chars().all(|c| c.is_ascii_lowercase() || c == '-'))
                .map(str::to_string)
                .collect::<Vec<_>>()
        })
        .filter(|words| !words.is_empty())
        .collect()
}

/// Every built-in skill, as an agent outside the app is served it, tells
/// that agent what to run, and every command it names is one the CLI has.
///
/// `SKILL.md` no longer restates the built-in skills' procedures: an outside
/// agent fetches each skill, rendered in CLI commands. So what has to hold is
/// that the rendering is real. A command the CLI does not have is the one
/// mistake the fetched text cannot recover from, and nothing else ties a rule
/// fragment's wording to the parser.
///
/// The list of skills comes from the seed table, so a skill added later is
/// checked with no change here.
#[test]
fn every_seeded_skill_is_served_in_real_cli_commands() {
    let cli = Cli::command();
    let mut problems = Vec::new();

    for seed in nodespace_agent::skill_pipeline::SKILL_SEEDS {
        let commands = cited_commands(&seed.external_body());
        if commands.is_empty() {
            problems.push(format!("{}: names no `nodespace` command", seed.title));
        }
        for words in commands {
            let shown = format!("nodespace {}", words.join(" "));
            let Some(group) = visible(&cli).into_iter().find(|c| c.get_name() == words[0]) else {
                problems.push(format!("{}: `{shown}` is not a command", seed.title));
                continue;
            };
            let leaves = visible(group);
            if leaves.is_empty() {
                continue;
            }
            // A group is cited either by a leaf (`node update`) or, rarely,
            // bare. A second word that is not one of its leaves is a command
            // that does not exist. A flag there is the group run with its own
            // arguments (`query --type`), which `query` takes beside `run`.
            if let Some(leaf) = words.get(1).filter(|word| !word.starts_with('-')) {
                if !leaves.iter().any(|c| c.get_name() == leaf) {
                    problems.push(format!("{}: `{shown}` is not a command", seed.title));
                }
            }
        }
    }

    assert!(
        problems.is_empty(),
        "built-in skills served to an external agent must name real CLI commands: \
         {problems:#?}\nFix the `skill-md` form of the rule fragment that names it \
         (packages/agent/src/seeds/rules/skill-md/)."
    );
}

/// `SKILL.md` teaches the fetch and carries no procedure a skill owns.
///
/// The body is loaded whole on activation, so everything in it is paid for on
/// every task. Procedure lives in the skill nodes, fetched when a task needs
/// it. This pins the three things the body has to say for that to work, and
/// the sections that used to restate procedure and must not come back.
#[test]
fn skill_md_teaches_the_fetch_and_restates_no_procedure() {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../skill");
    let body = std::fs::read_to_string(dir.join("SKILL.md")).expect("failed to read SKILL.md");

    // How to fetch: by task, and listing everything.
    assert!(
        body.contains("nodespace skill guidance \"<the task, in your own words>\""),
        "SKILL.md must show how to fetch the skills for a task"
    );
    assert!(
        body.contains("every skill, by name and what it is for"),
        "SKILL.md must show how to list every skill"
    );
    // What comes back, and how to treat it.
    assert!(
        body.contains("What comes back is your instructions for the operation."),
        "SKILL.md must tell the agent to treat fetched skills as its instructions"
    );
    // That more instructions live in NodeSpace than this file carries, of
    // both kinds.
    for needle in [
        "More instructions live there than this file carries",
        "How to operate NodeSpace itself",
        "How this workspace works",
        "a team's own record types",
        "fetch them before you work in one of its domains",
    ] {
        assert!(body.contains(needle), "SKILL.md must say: {needle:?}");
    }
    // The trust boundary stays in front of the first fetch.
    assert!(body.contains("references/graph-authored-guidance.md"));

    // An installed setup is described as types and workflows. The one place
    // the word survives is a reference file's name.
    let prose: String = body
        .lines()
        .map(|line| line.replace("-playbook.md", ""))
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        !prose.to_lowercase().contains("playbook"),
        "SKILL.md describes an installed setup as its types and workflows, not as a Playbook"
    );

    for heading in ["## Tool Decision Guide", "## Common Agent Tasks"] {
        assert!(
            !body.contains(heading),
            "SKILL.md restates procedure a skill owns: {heading:?} is back"
        );
    }
    // The relationship claim that contradicted the command reference: four
    // names need no declaration.
    assert!(
        !body.contains("Relationships must be defined on a schema"),
        "SKILL.md must not say every relationship needs a schema declaration"
    );
    for name in nodespace_core::models::schema::BUILTIN_RELATIONSHIP_NAMES {
        assert!(
            body.contains(&format!("`{name}`")),
            "SKILL.md's mental model must name the built-in relationship `{name}`"
        );
    }
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

/// The "Reaching NodeSpace" section's MCP passthrough entry names the actual
/// tool the `nodespace mcp` server exposes.
///
/// Unlike the CLI surface and schema rules, that prose has no
/// generator behind it — it is hand-written, same as the rest of the
/// section, because it is judgment prose rather than a derivable
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
        "SKILL.md's Reaching NodeSpace section does not reference the MCP \
         passthrough's actual tool name ({needle}, from mcp.rs's TOOL_NAME \
         constant) — update that section."
    );
}

/// The section documents the MCP passthrough's real dispatch timeout and the
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
        "SKILL.md's Reaching NodeSpace section does not mention the MCP \
         passthrough's actual dispatch timeout ({needle}, from mcp.rs's \
         DISPATCH_TIMEOUT constant) — update that section."
    );
    assert!(
        skill.contains("session launch") && skill.contains("session attach"),
        "SKILL.md's Reaching NodeSpace section should name the streaming \
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
    // `cargo` from PATH, not `env!("CARGO")`: that names the toolchain's own
    // binary, which skips rustup. rustup then picks a toolchain for each rustc
    // call by its directory, so dependencies outside the repository compile
    // with the machine's default instead of the pinned one, and the build
    // fails on the mix.
    let status = std::process::Command::new("cargo")
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
/// The region is laid out by a hand-written template that includes each rule
/// by id (`packages/agent/src/seeds/skill-md/schema-rules.md`), so registering
/// a rule in the `SCHEMA_RULES` array is NOT
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
         Include each in packages/agent/src/seeds/skill-md/schema-rules.md, \
         then regenerate.",
        missing.join(", ")
    );
}
