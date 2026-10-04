//! What an agent outside the app gets from a skill fetch, against the real
//! seeded registry and the real embedding model.
//!
//! `nodespace skill guidance "<task>"` used to match nothing for any
//! non-empty task. It rode the generic node search, whose default scope
//! drops every `skill` node, under a 0.7 score floor the skill search never
//! had. These drive the operation the command is now served by,
//! `skill_ops::find_skill_guidance`, over a database seeded the way the
//! daemon seeds one and embedded through the queue the way the processor
//! embeds it.
//!
//! Ignored by default — loads a real embedding model from the standard
//! NodeSpace catalog path. Run explicitly:
//!
//! ```text
//! .tools/bin/cargo-nextest nextest run -p nodespace-agent --test it live_skill_guidance_fetch:: --run-ignored all
//! ```

use std::sync::Arc;

use nodespace_agent::local_agent::tools::Tool;
use nodespace_agent::skill_pipeline::{seed_skill_nodes, seed_tool_nodes, SKILL_SEEDS};
use nodespace_core::db::SqliteStore;
use nodespace_core::markdown::prepare_nodes_from_template;
use nodespace_core::methodology::{install_playbook, playbook_by_id};
use nodespace_core::models::SkillFields;
use nodespace_core::ops::search_ops::{search_semantic, SearchSemanticInput};
use nodespace_core::ops::skill_ops::{find_skill_guidance, find_skills, FindSkillsInput};
use nodespace_core::schema::handle_create_schema;
use nodespace_core::services::{
    CreateNodeParams, InsertPositionOwned, NodeAccessor, NodeEmbeddingService, NodeService,
};
use nodespace_nlp_engine::{EmbeddingConfig, EmbeddingService};
use serde_json::json;
use tempfile::TempDir;

/// A database seeded with the built-in skills and tools and every queued root
/// embedded. With `with_workspace_types` it is also a workspace with types of
/// its own: the Linear-style setup (its skills linked to `issue` and
/// `cycle`), and two user-defined types no skill is linked to. Returns `None`
/// when the embedding model is not on disk, so the test skips.
async fn seeded_and_embedded(
    with_workspace_types: bool,
) -> Option<(Arc<NodeEmbeddingService>, Arc<NodeService>, TempDir)> {
    let temp_dir = TempDir::new().expect("tempdir");
    let db_path = temp_dir.path().join("test.db");
    let mut store = Arc::new(SqliteStore::new(db_path).await.expect("store must open"));
    let node_service = Arc::new(
        NodeService::new(&mut store)
            .await
            .expect("node service must init"),
    );

    let mut nlp = EmbeddingService::new(EmbeddingConfig::default()).expect("config must validate");
    if nlp.initialize().is_err() || !nlp.is_initialized() {
        eprintln!("SKIP live_skill_guidance_fetch: nomic-embed-text-v1.5 model not found on disk");
        return None;
    }
    let node_accessor: Arc<dyn NodeAccessor> = node_service.clone();
    let embedding_service = Arc::new(NodeEmbeddingService::new(
        Arc::new(nlp),
        store.clone(),
        node_accessor,
        node_service.behaviors().clone(),
    ));

    let groups: Vec<_> = seed_skill_nodes()
        .iter()
        .chain(seed_tool_nodes().iter())
        .map(|t| prepare_nodes_from_template(t).expect("template must parse"))
        .collect();
    node_service
        .seed_nodes_from_templates(groups)
        .await
        .expect("seeding the built-in skills and tools must succeed");

    if with_workspace_types {
        let playbook = playbook_by_id("linear").expect("the linear playbook ships");
        let report = install_playbook(&node_service, &playbook).await;
        assert!(report.success, "the linear playbook must install");

        // Two types of the user's own, with no skill linked to either.
        for params in [
            json!({
                "name": "Invoice",
                "description": "A bill sent to a client for work done",
                "fields": [
                    { "name": "amount", "type": "number", "description": "What the client owes" },
                    { "name": "paid_on", "type": "date" }
                ]
            }),
            json!({
                "name": "Venue",
                "description": "A place an event can be held",
                "fields": [{ "name": "capacity", "type": "number" }]
            }),
        ] {
            handle_create_schema(&node_service, params)
                .await
                .expect("schema must create");
        }
    }

    // Embed what the write paths queued, and nothing else: a root no path
    // queued stays unembedded here, as it would under the processor.
    let queued = store
        .get_stale_embedding_root_ids(None, 0, 3)
        .await
        .expect("the queue must read");
    for id in queued {
        embedding_service
            .embed_root_node(&id)
            .await
            .unwrap_or_else(|e| panic!("queued root {id} must embed: {e}"));
    }

    Some((embedding_service, node_service, temp_dir))
}

fn input(query: &str, limit: usize) -> FindSkillsInput {
    FindSkillsInput {
        query: query.to_string(),
        limit: Some(limit),
    }
}

/// A query taken from a skill's own description returns that skill first,
/// for every built-in skill.
#[tokio::test]
#[ignore = "requires the locked nomic-embed-text-v1.5 GGUF on disk"]
async fn a_query_from_a_skills_own_description_returns_that_skill_first() {
    let Some((embedding_service, node_service, _temp_dir)) = seeded_and_embedded(false).await
    else {
        return;
    };

    for seed in SKILL_SEEDS {
        let guidance = find_skill_guidance(
            &embedding_service,
            &node_service,
            input(seed.description, 3),
        )
        .await
        .expect("the fetch must succeed");

        let names: Vec<&str> = guidance.skills.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(
            names.first().copied(),
            Some(seed.title),
            "{:?}'s own description did not return it first: {names:?}",
            seed.title
        );
        let first = &guidance.skills[0];
        assert!(
            !first.instructions.trim().is_empty(),
            "{:?} came back with no procedure",
            seed.title
        );
        assert_eq!(first.description, seed.description);
    }
}

/// Short, plainly worded tasks match. Under the generic search's 0.7 floor
/// and default scope, each of these returned nothing.
#[tokio::test]
#[ignore = "requires the locked nomic-embed-text-v1.5 GGUF on disk"]
async fn plainly_worded_tasks_return_the_skill_for_them() {
    let Some((embedding_service, node_service, _temp_dir)) = seeded_and_embedded(false).await
    else {
        return;
    };

    for (task, expected) in [
        ("delete a node", "Node Deletion"),
        ("define a new type with an enum field", "Schema Creation"),
        ("import this markdown document", "Bulk Import"),
        ("merge two duplicate records into one", "Node Merge"),
    ] {
        let guidance = find_skill_guidance(&embedding_service, &node_service, input(task, 3))
            .await
            .expect("the fetch must succeed");
        let names: Vec<&str> = guidance.skills.iter().map(|s| s.name.as_str()).collect();
        assert!(
            names.contains(&expected),
            "{task:?} did not return {expected:?}: {names:?}"
        );
    }
}

/// The fetch ranks skills exactly as the in-app skill search does for the
/// same query: same skills, same order, same scores.
#[tokio::test]
#[ignore = "requires the locked nomic-embed-text-v1.5 GGUF on disk"]
async fn a_fetch_ranks_skills_as_the_in_app_skill_search_does() {
    let Some((embedding_service, node_service, _temp_dir)) = seeded_and_embedded(true).await else {
        return;
    };

    for task in [
        "remove the resolved tickets",
        "mark the outage report done",
        "add an issue to the current cycle",
        "point the rebuild task at the decision it has to respect",
    ] {
        let in_app = find_skills(&embedding_service, &node_service, input(task, 5))
            .await
            .expect("find_skills must succeed");
        let expected: Vec<(String, f64)> = in_app
            .skills
            .iter()
            .filter(|s| s["kind"] == "skill")
            .map(|s| {
                (
                    s["name"].as_str().unwrap_or_default().to_string(),
                    s["confidence"].as_f64().unwrap_or_default(),
                )
            })
            .collect();

        let fetched = find_skill_guidance(&embedding_service, &node_service, input(task, 5))
            .await
            .expect("the fetch must succeed");
        let actual: Vec<(String, f64)> = fetched
            .skills
            .iter()
            .map(|s| (s.name.clone(), s.confidence.unwrap_or_default()))
            .collect();

        assert!(!actual.is_empty(), "{task:?} returned no skill");
        assert_eq!(actual, expected, "{task:?} ranked differently");
    }
}

/// On a workspace with the Linear-style setup, a task in that domain returns
/// the domain's own skills and the schemas of the types it touches.
#[tokio::test]
#[ignore = "requires the locked nomic-embed-text-v1.5 GGUF on disk"]
async fn a_task_in_an_installed_domain_returns_its_skills_and_its_schemas() {
    let Some((embedding_service, node_service, _temp_dir)) = seeded_and_embedded(true).await else {
        return;
    };

    let guidance = find_skill_guidance(
        &embedding_service,
        &node_service,
        input("add an issue to the current cycle", 3),
    )
    .await
    .expect("the fetch must succeed");

    let skills: Vec<&str> = guidance.skills.iter().map(|s| s.name.as_str()).collect();
    for expected in ["Creating an Issue", "Sprints and Cycles"] {
        assert!(
            skills.contains(&expected),
            "the domain skill {expected:?} was not returned: {skills:?}"
        );
    }

    let schemas: Vec<&str> = guidance.schemas.iter().map(|s| s.id.as_str()).collect();
    for expected in ["issue", "cycle"] {
        assert!(
            schemas.contains(&expected),
            "the `{expected}` schema was not returned: {schemas:?}"
        );
    }

    let issue = guidance
        .schemas
        .iter()
        .find(|s| s.id == "issue")
        .expect("issue schema");
    let fields = issue.definition["fields"]
        .as_array()
        .expect("a schema carries its fields");
    assert!(!fields.is_empty(), "{}", issue.definition);
    let status = fields
        .iter()
        .find(|f| f["name"] == "status")
        .expect("issue carries the status field it inherits from task");
    assert!(
        status["enum_values"]
            .as_array()
            .is_some_and(|values| values.iter().any(|v| v == "in_review")),
        "the status field must list its allowed values: {status}"
    );
    assert!(
        issue.definition["relationships"]
            .as_array()
            .is_some_and(|r| !r.is_empty()),
        "a schema carries its relationships: {}",
        issue.definition
    );
}

/// A fetch returns the schema of a type the task is about and no skill is
/// linked to, and does not hand back a type for a task that is about none.
///
/// The schema search has no score floor, so it matches some type to every
/// task. A fetch keeps a match only when it clears the measured bar, when a
/// returned skill is linked to the type, or when the task names the type.
/// `invoice` and `venue` have no skill linked, so the first and last of
/// those are the only ways they can come back.
#[tokio::test]
#[ignore = "requires the locked nomic-embed-text-v1.5 GGUF on disk"]
async fn a_fetch_returns_the_unlinked_types_a_task_is_about_and_no_others() {
    let Some((embedding_service, node_service, _temp_dir)) = seeded_and_embedded(true).await else {
        return;
    };

    // The unlinked types among a fetch's schemas.
    let unlinked_types_for = |task: &'static str| {
        let embedding_service = embedding_service.clone();
        let node_service = node_service.clone();
        async move {
            let guidance = find_skill_guidance(&embedding_service, &node_service, input(task, 3))
                .await
                .expect("the fetch must succeed");
            let mut ids: Vec<String> = guidance
                .schemas
                .into_iter()
                .map(|s| s.id)
                .filter(|id| id == "invoice" || id == "venue")
                .collect();
            ids.sort();
            ids
        }
    };

    // About a type, in words that never name it.
    for (task, expected) in [
        ("bill the client for the March work", "invoice"),
        ("book a place for the offsite", "venue"),
        ("how many people does the hall hold", "venue"),
    ] {
        assert_eq!(
            unlinked_types_for(task).await,
            vec![expected.to_string()],
            "{task:?} is about `{expected}` and not the other"
        );
    }

    // Named outright, on a task the schema search matches only weakly.
    assert_eq!(
        unlinked_types_for("delete the invoice we sent by mistake").await,
        vec!["invoice".to_string()],
        "a type the task names is returned"
    );

    // About an operation, or about another domain: neither type comes back.
    for task in [
        "delete a node",
        "import this markdown document",
        "merge two duplicate records into one",
        "define a new type with an enum field",
        "add an issue to the current cycle",
        "what did we decide about the retry budget",
    ] {
        assert_eq!(
            unlinked_types_for(task).await,
            Vec::<String>::new(),
            "{task:?} is about neither type"
        );
    }
}

/// A skill a user wrote names built-in tools, which an agent outside the app
/// cannot call. A fetch for its task returns it as written, with the
/// `nodespace` command of each tool it names: what that agent runs in the
/// tool's place. A tool named only inside a longer identifier is not one of
/// them.
///
/// The command is then run over the real transport by
/// `cli_integration::skill_guidance_fetches_skills_and_schemas_end_to_end`.
#[tokio::test]
#[ignore = "requires the locked nomic-embed-text-v1.5 GGUF on disk"]
async fn a_user_written_skill_naming_a_tool_is_fetched_with_the_command_for_it() {
    let Some((embedding_service, node_service, _temp_dir)) = seeded_and_embedded(false).await
    else {
        return;
    };

    let description = "Record a decision the team made and link it to the task it settles.";
    let skill = SkillFields::new(description, &["get_node"], 3).into_node("Recording a Decision");
    let skill_id = skill.id.clone();
    node_service
        .create_node(skill)
        .await
        .expect("the user's skill must create");
    node_service
        .create_node_with_parent(CreateNodeParams {
            id: None,
            node_type: "text".to_string(),
            content: "Find the task with `search_nodes`, then link the decision to it with \
                      create_relationship. Never call update_nodes_from_markdown."
                .to_string(),
            parent_id: Some(skill_id.clone()),
            position: InsertPositionOwned::End,
            properties: json!({}),
            lifecycle_status: None,
        })
        .await
        .expect("the user's procedure must create");
    embedding_service
        .embed_root_node(&skill_id)
        .await
        .expect("the user's skill must embed");

    let guidance = find_skill_guidance(&embedding_service, &node_service, input(description, 3))
        .await
        .expect("the fetch must succeed");
    let fetched = guidance
        .skills
        .iter()
        .find(|s| s.id == skill_id)
        .unwrap_or_else(|| {
            panic!(
                "the user's skill did not match its own description: {:?}",
                guidance.skills.iter().map(|s| &s.name).collect::<Vec<_>>()
            )
        });

    assert!(
        fetched.instructions.contains("create_relationship"),
        "a user's skill is served as written: {}",
        fetched.instructions
    );
    let commands: Vec<(&str, &str)> = fetched
        .tool_commands
        .iter()
        .map(|c| (c.tool.as_str(), c.command.as_str()))
        .collect();
    assert_eq!(
        commands,
        [
            ("create_relationship", "nodespace relationship create"),
            ("get_node", "nodespace node get"),
            ("search_nodes", "nodespace query"),
        ],
        "the tools it lists and names, each with its command"
    );
    for command in &fetched.tool_commands {
        assert!(
            Tool::is_seeded_as(&command.tool, &command.node_id),
            "{} was read from a node that is not the tool's seed",
            command.tool
        );
    }

    // Every built-in skill the same fetch returns carries its tools' commands
    // too.
    for skill in guidance.skills.iter().filter(|s| s.id != skill_id) {
        assert!(
            !skill.tool_commands.is_empty(),
            "{:?} came back with no tool commands",
            skill.name
        );
    }
}

/// `nodespace search "<query>" --type skill` returns matching skills, and a
/// search that names no type still leaves system types out.
#[tokio::test]
#[ignore = "requires the locked nomic-embed-text-v1.5 GGUF on disk"]
async fn a_search_naming_the_skill_type_returns_skills_and_an_unfiltered_one_does_not() {
    let Some((embedding_service, node_service, _temp_dir)) = seeded_and_embedded(false).await
    else {
        return;
    };

    let search = |node_types: Option<Vec<String>>| SearchSemanticInput {
        query: "delete a node".to_string(),
        // The CLI's own request leaves the threshold to the server default.
        threshold: None,
        limit: Some(20),
        collection_id: None,
        collection: None,
        exclude_collections: None,
        include_markdown: Some(0),
        include_archived: None,
        scope: None,
        node_types,
        property_filters: None,
        include_edges: None,
        graph_boost: None,
        include_title_matches: None,
    };

    let typed = search_semantic(
        &node_service,
        &embedding_service,
        search(Some(vec!["skill".to_string()])),
    )
    .await
    .expect("the typed search must succeed");
    let titles: Vec<&str> = typed
        .matched_nodes
        .iter()
        .map(|n| n.content.as_str())
        .collect();
    assert!(
        titles.contains(&"Node Deletion"),
        "`--type skill` must return matching skills: {titles:?}"
    );
    assert!(
        typed.matched_nodes.iter().all(|n| n.node_type == "skill"),
        "a typed search returns only the named type"
    );

    let unfiltered = search_semantic(&node_service, &embedding_service, search(None))
        .await
        .expect("the unfiltered search must succeed");
    assert!(
        unfiltered
            .matched_nodes
            .iter()
            .all(|n| n.node_type != "skill"),
        "an unfiltered search keeps system types out: {:?}",
        unfiltered
            .matched_nodes
            .iter()
            .map(|n| (&n.node_type, &n.content))
            .collect::<Vec<_>>()
    );
}
