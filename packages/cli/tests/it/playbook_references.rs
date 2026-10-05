//! Runs each methodology playbook reference end to end, as written.
//!
//! `packages/skill/references/*-playbook.md` is what an agent follows to
//! install a methodology over the CLI. Its commands are rendered from the
//! methodology definitions, so they can name a flag or a shape the CLI does
//! not take and nothing else would notice: the agent would simply fail on the
//! first command. Each test here reads a playbook's `bash` blocks in order,
//! parses every command with the CLI's own argument parser and runs it against
//! a fresh daemon.

use std::path::PathBuf;

use clap::Parser;
use nodespace_cli::{commands, connect, Cli, Command, DatabaseIdInterceptor, NodeClient};
use nodespace_daemon::nodespace::{NodeSortOrder, QueryNodesSimpleRequest};

use crate::cli_integration::spawn_test_daemon;

/// Stands in the playbooks for the id of the skill a block is linking.
const SKILL_ID_PLACEHOLDER: &str = "<skill-id>";

/// The commands of one fenced `bash` block, each split into its arguments the
/// way a POSIX shell would split it.
type Block = Vec<Vec<String>>;

/// Every fenced `bash` block of a playbook reference, in document order.
fn playbook_blocks(file: &str) -> Vec<Block> {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../skill/references")
        .join(file);
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("failed to read {}: {e}", path.display()));

    let mut blocks = Vec::new();
    let mut current: Option<String> = None;
    for line in text.lines() {
        match (&mut current, line) {
            (None, "```bash") => current = Some(String::new()),
            (Some(script), "```") => {
                blocks.push(split_commands(script));
                current = None;
            }
            (Some(script), line) => {
                script.push_str(line);
                script.push('\n');
            }
            (None, _) => {}
        }
    }
    assert!(current.is_none(), "{file} ends inside a bash block");
    blocks
}

/// One block's script as argument lists: a trailing `\` continues a command
/// onto the next line.
fn split_commands(script: &str) -> Block {
    script
        .replace("\\\n", " ")
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| {
            shell_words::split(line).unwrap_or_else(|e| panic!("unparseable command {line:?}: {e}"))
        })
        .collect()
}

/// The ids of every node of `node_type`.
async fn ids_of_type(raw: &mut NodeClient, node_type: &str) -> Vec<String> {
    raw.query_nodes_simple(QueryNodesSimpleRequest {
        include_archived: false,
        id: None,
        mentioned_by: None,
        content_contains: None,
        title_contains: None,
        node_type: Some(node_type.into()),
        limit: 0,
        offset: 0,
        order_by: NodeSortOrder::Unspecified as i32,
    })
    .await
    .unwrap_or_else(|e| panic!("query {node_type} nodes: {e}"))
    .into_inner()
    .nodes
    .into_iter()
    .map(|node| node.id)
    .collect()
}

/// Parses one command as the `nodespace` binary would and runs it.
///
/// Returns the type of the node it created, when it is a `node create`.
async fn run_command(client: &mut NodeClient, argv: &[String]) -> Option<String> {
    assert_eq!(argv[0], "nodespace", "not a nodespace command: {argv:?}");
    let cli = Cli::try_parse_from(argv)
        .unwrap_or_else(|e| panic!("the CLI refuses a documented command {argv:?}:\n{e}"));

    let created_type = match &cli.command {
        Command::Node {
            action: commands::node::NodeAction::Create(args),
        } => Some(args.node_type.clone()),
        _ => None,
    };
    let result = match cli.command {
        Command::Node { action } => commands::node::run(client, action, cli.json).await,
        Command::Schema { action } => commands::schema::run(client, action, cli.json).await,
        Command::Relationship { action } => {
            commands::relationship::run(client, action, cli.json).await
        }
        other => panic!("a playbook runs a command this test does not drive: {other:?}"),
    };
    result.unwrap_or_else(|e| panic!("a documented command failed {argv:?}:\n{e:#}"));
    created_type
}

/// A skill for the blocks that link one the playbook describes in prose
/// instead of creating with a command (its closing workspace skill).
async fn create_prose_skill(client: &mut NodeClient, raw: &mut NodeClient) -> String {
    let before = ids_of_type(raw, "skill").await;
    let argv = [
        "nodespace",
        "node",
        "create",
        "--type",
        "skill",
        "--content",
        "Workspace",
        "--properties",
        r#"{"description":"What workflow this workspace uses."}"#,
    ]
    .map(String::from);
    run_command(client, &argv).await;
    new_id(raw, "skill", &before).await
}

/// The one node of `node_type` that is not in `before`.
async fn new_id(raw: &mut NodeClient, node_type: &str, before: &[String]) -> String {
    let mut added: Vec<String> = ids_of_type(raw, node_type)
        .await
        .into_iter()
        .filter(|id| !before.contains(id))
        .collect();
    assert_eq!(added.len(), 1, "expected one new {node_type} node");
    added.remove(0)
}

/// Runs every command of `file` in order against a fresh daemon and checks
/// that each `node create` it documents left a node behind.
async fn run_playbook(file: &str) {
    let blocks = playbook_blocks(file);
    let documented = |node_type: &str| {
        blocks
            .iter()
            .flatten()
            .filter(|argv| {
                argv.windows(2)
                    .any(|w| w[0] == "--type" && w[1] == node_type)
            })
            .filter(|argv| argv[1..3] == ["node", "create"])
            .count()
    };
    let (plays, skills, views) = (documented("play"), documented("skill"), documented("query"));
    assert!(
        plays > 0 && skills > 0 && views > 0,
        "{file} documents {plays} plays, {skills} skills and {views} views; \
         the block reader no longer finds its commands"
    );

    let (sock, shutdown, _tempdir) = spawn_test_daemon().await;
    let mut client = connect(&sock, DatabaseIdInterceptor::none())
        .await
        .expect("connect");
    let mut raw = connect(&sock, DatabaseIdInterceptor::none())
        .await
        .expect("raw connect");

    let counts_before = [
        ids_of_type(&mut raw, "play").await.len(),
        ids_of_type(&mut raw, "skill").await.len(),
        ids_of_type(&mut raw, "query").await.len(),
    ];

    let mut prose_skills = 0;
    for block in &blocks {
        // The skill this block links: the one it creates, or one the
        // surrounding prose asks for.
        let mut skill_id: Option<String> = None;
        for argv in block {
            let links_a_skill = argv.iter().any(|arg| arg == SKILL_ID_PLACEHOLDER);
            if links_a_skill && skill_id.is_none() {
                skill_id = Some(create_prose_skill(&mut client, &mut raw).await);
                prose_skills += 1;
            }
            let argv: Vec<String> = argv
                .iter()
                .map(|arg| match (arg.as_str(), &skill_id) {
                    (SKILL_ID_PLACEHOLDER, Some(id)) => id.clone(),
                    _ => arg.clone(),
                })
                .collect();

            let skills_before = ids_of_type(&mut raw, "skill").await;
            if run_command(&mut client, &argv).await.as_deref() == Some("skill") {
                skill_id = Some(new_id(&mut raw, "skill", &skills_before).await);
            }
        }
    }

    let counts_after = [
        ids_of_type(&mut raw, "play").await.len(),
        ids_of_type(&mut raw, "skill").await.len(),
        ids_of_type(&mut raw, "query").await.len(),
    ];
    assert_eq!(
        [
            counts_after[0] - counts_before[0],
            counts_after[1] - counts_before[1],
            counts_after[2] - counts_before[2],
        ],
        [plays, skills + prose_skills, views],
        "{file}: plays, skills and views created"
    );

    let _ = shutdown.send(());
}

#[tokio::test]
async fn the_linear_playbook_runs_as_written() {
    run_playbook("linear-playbook.md").await;
}

#[tokio::test]
async fn the_jira_playbook_runs_as_written() {
    run_playbook("jira-playbook.md").await;
}

#[tokio::test]
async fn the_spec_driven_playbook_runs_as_written() {
    run_playbook("spec-driven-playbook.md").await;
}

/// The `Authoring a skill` example in the CLI reference is the fourth place
/// the JSON-object form is documented; it must parse too.
#[test]
fn the_cli_reference_examples_that_set_properties_parse() {
    let with_properties: Vec<Vec<String>> = playbook_blocks("cli.md")
        .into_iter()
        .flatten()
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
