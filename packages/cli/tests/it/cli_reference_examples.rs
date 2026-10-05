//! The hand-written examples in the CLI reference parse.
//!
//! `packages/skill/references/cli.md` is what an agent reads to learn the
//! CLI. Its command surface is generated from the clap definitions, but its
//! worked examples are written by hand, so one can name a flag the CLI does
//! not take and nothing else would notice: the agent would fail on the
//! command it copied.

use std::path::PathBuf;

use clap::Parser;
use nodespace_cli::Cli;

/// Every `nodespace` command in the reference's fenced `bash` blocks, split
/// into its arguments the way a POSIX shell would split it. A trailing `\`
/// continues a command onto the next line.
fn documented_commands() -> Vec<Vec<String>> {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../skill/references/cli.md");
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("failed to read {}: {e}", path.display()));

    let mut scripts = Vec::new();
    let mut current: Option<String> = None;
    for line in text.lines() {
        match (&mut current, line) {
            (None, "```bash") => current = Some(String::new()),
            (Some(script), "```") => {
                scripts.push(std::mem::take(script));
                current = None;
            }
            (Some(script), line) => {
                script.push_str(line);
                script.push('\n');
            }
            (None, _) => {}
        }
    }

    scripts
        .iter()
        .flat_map(|script| {
            script
                .replace("\\\n", " ")
                .lines()
                .filter(|line| line.starts_with("nodespace "))
                .filter_map(|line| shell_words::split(line).ok())
                .collect::<Vec<_>>()
        })
        .collect()
}

/// The JSON-object form of setting properties is documented by example only,
/// so the example is what pins the flag.
#[test]
fn the_examples_that_set_properties_from_a_json_object_parse() {
    let with_properties: Vec<Vec<String>> = documented_commands()
        .into_iter()
        .filter(|argv| argv.iter().any(|arg| arg == "--properties"))
        .collect();
    assert!(
        !with_properties.is_empty(),
        "cli.md no longer shows a --properties example"
    );
    for argv in with_properties {
        Cli::try_parse_from(&argv)
            .unwrap_or_else(|e| panic!("the CLI refuses a documented command {argv:?}:\n{e}"));
    }
}
