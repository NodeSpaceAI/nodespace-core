//! End-to-end install of the Linear-style recipe against a real service.
//!
//! The unit tests in `methodology::linear` assert the *shape* of the recipe's
//! JSON. That is not enough on its own: a CEL condition that will not compile,
//! a cron the parser rejects, a `mapsTo` naming a value that does not exist, or
//! a relationship missing its reverse half all produce perfectly well-shaped
//! JSON and fail only when something tries to install it.
//!
//! So this installs the whole recipe the way the GUI will — `handle_create_schema`,
//! `handle_update_schema`, then play nodes through `create_node` — and asserts
//! the result. Every write here runs the same validation a hand-authored call
//! would, `validate_play_rules` included, so a broken Play is rejected here
//! rather than discovered at execution time.

use anyhow::Result;
use nodespace_core::db::SqliteStore;
use nodespace_core::methodology::{install_recipe, recipe_by_id, MethodologyRecipe, StepOutcome};
use nodespace_core::models::Node;
use nodespace_core::schema::{handle_create_schema, handle_update_schema};
use nodespace_core::services::NodeService;
use std::sync::Arc;
use tempfile::TempDir;

async fn test_service() -> Result<(Arc<NodeService>, TempDir)> {
    let temp_dir = TempDir::new()?;
    let db_path = temp_dir.path().join("test.db");
    let mut store = Arc::new(SqliteStore::new(db_path).await?);
    let service = Arc::new(NodeService::new(&mut store).await?);
    Ok((service, temp_dir))
}

fn linear() -> MethodologyRecipe {
    recipe_by_id("linear").expect("the linear recipe ships")
}

/// Install every step, in recipe order, failing loudly on the first rejection.
async fn install(service: &Arc<NodeService>, recipe: &MethodologyRecipe) -> Result<()> {
    for step in &recipe.schemas {
        handle_create_schema(service, step.params.clone())
            .await
            .map_err(|e| anyhow::anyhow!("create_schema({}) rejected: {e}", step.schema_id))?;
    }
    for ext in &recipe.field_value_extensions {
        handle_update_schema(service, ext.params.clone())
            .await
            .map_err(|e| {
                anyhow::anyhow!(
                    "add_field_values({}.{}) rejected: {e}",
                    ext.schema_id,
                    ext.field
                )
            })?;
    }
    for play in &recipe.plays {
        let node = Node::new_with_id(
            play.play_id.to_string(),
            "play".to_string(),
            play.name.to_string(),
            play.properties(),
        );
        service
            .create_node(node)
            .await
            .map_err(|e| anyhow::anyhow!("installing play {} rejected: {e}", play.play_id))?;
    }
    Ok(())
}

/// The whole recipe installs. This is the test that would catch a
/// non-compiling CEL condition, an unparseable cron, or a `mapsTo` naming a
/// value `task.status` does not have.
#[tokio::test]
async fn the_linear_recipe_installs_cleanly() -> Result<()> {
    let (service, _tmp) = test_service().await?;
    install(&service, &linear()).await?;
    Ok(())
}

/// ADR-078's headline property: an instance of `issue` is genuinely an
/// `issue`, not a `task` wearing a label.
#[tokio::test]
async fn an_issue_instance_carries_issue_as_its_real_node_type() -> Result<()> {
    let (service, _tmp) = test_service().await?;
    install(&service, &linear()).await?;

    let id = service
        .create_node(Node::new(
            "issue".to_string(),
            "Fix the thing".to_string(),
            serde_json::json!({ "status": "backlog", "estimate": "3" }),
        ))
        .await?;

    let node = service.get_node(&id).await?.expect("issue exists");
    assert_eq!(
        node.node_type, "issue",
        "an issue instance must report its own type, not the parent's"
    );
    Ok(())
}

/// The extended vocabulary is real and enforced: an appended value is
/// accepted, and a value nobody declared is not.
#[tokio::test]
async fn extended_status_values_are_accepted_and_unknown_ones_are_not() -> Result<()> {
    let (service, _tmp) = test_service().await?;
    install(&service, &linear()).await?;

    for status in ["triage", "backlog", "in_review"] {
        service
            .create_node(Node::new(
                "issue".to_string(),
                format!("issue in {status}"),
                serde_json::json!({ "status": status }),
            ))
            .await
            .unwrap_or_else(|e| panic!("'{status}' should be a valid issue status: {e}"));
    }

    let rejected = service
        .create_node(Node::new(
            "issue".to_string(),
            "bogus".to_string(),
            serde_json::json!({ "status": "not_a_real_status" }),
        ))
        .await;
    assert!(
        rejected.is_err(),
        "an undeclared status must be rejected, or the vocabulary means nothing"
    );
    Ok(())
}

/// Installing twice must not silently adopt or overwrite the first install.
/// The second attempt is expected to fail on the already-taken schema id —
/// which is what the GUI's collision handling detects before retrying under a
/// suffixed id.
#[tokio::test]
async fn reinstalling_over_an_existing_schema_id_is_refused_not_silent() -> Result<()> {
    let (service, _tmp) = test_service().await?;
    let recipe = linear();
    install(&service, &recipe).await?;

    let again = handle_create_schema(&service, recipe.schemas[0].params.clone()).await;
    assert!(
        again.is_err(),
        "a colliding schema id must be refused so the caller can disclose and re-key it, \
         never silently adopted or overwritten"
    );
    Ok(())
}

/// Seeded plays are resettable: each carries its shipped rules under
/// `_seed.default_rules`, which is what `reset_seeded_play_to_default` reads
/// and what marks the play seeded for the edit/disable warning (ADR-060 §8).
#[tokio::test]
async fn installed_plays_are_marked_seeded_with_their_shipped_defaults() -> Result<()> {
    let (service, _tmp) = test_service().await?;
    let recipe = linear();
    install(&service, &recipe).await?;

    for play in &recipe.plays {
        let node = service
            .get_node(play.play_id)
            .await?
            .unwrap_or_else(|| panic!("{} should exist after install", play.play_id));

        assert_eq!(node.node_type, "play");

        // `_seed` is internal bookkeeping and stays at the property root; the
        // schema-declared fields are bucketed under the node's type by
        // `normalize_flat_properties_to_namespace`, so the two are read from
        // different places.
        let seed = node
            .properties
            .get("_seed")
            .unwrap_or_else(|| panic!("{} must be marked seeded", play.play_id));

        let rules = node
            .properties
            .get("play")
            .and_then(|b| b.get("rules"))
            .or_else(|| node.properties.get("rules"))
            .unwrap_or_else(|| panic!("{} must carry rules", play.play_id));

        assert_eq!(
            seed.get("default_rules"),
            Some(rules),
            "{}'s shipped default must match its installed rules",
            play.play_id
        );
    }
    Ok(())
}

/// The recipe must be installable in the order it declares. A Play whose
/// trigger names a type with no schema is rejected by `validate_play_rules`,
/// so installing the plays before the schemas would fail — this pins that the
/// declared order is the working one.
#[tokio::test]
async fn plays_reference_types_the_recipe_creates_first() -> Result<()> {
    let (service, _tmp) = test_service().await?;
    let recipe = linear();

    // Plays first, with no schemas: every play referencing issue/cycle must
    // be refused, proving the ordering requirement is real and enforced.
    let mut refused = 0;
    for play in &recipe.plays {
        let node = Node::new_with_id(
            format!("premature-{}", play.play_id),
            "play".to_string(),
            play.name.to_string(),
            play.properties(),
        );
        if service.create_node(node).await.is_err() {
            refused += 1;
        }
    }
    assert_eq!(
        refused,
        recipe.plays.len(),
        "every Play references a recipe-created type, so all must be refused before \
         the schemas exist"
    );

    // And the declared order works.
    install(&service, &recipe).await?;
    Ok(())
}

// ---------------------------------------------------------------------------
// install_recipe: the path the GUI actually calls
// ---------------------------------------------------------------------------

/// The report's wire shape is a contract with the desktop app, which decodes
/// it into its own mirrored types. Pinned here because a drift in either
/// side's serde attributes surfaces as a decode error at install time — with
/// the install already done and the report, the only record of what landed,
/// lost.
#[tokio::test]
async fn the_install_report_serializes_to_the_shape_clients_decode() -> Result<()> {
    let (service, _tmp) = test_service().await?;

    // A pre-existing `cycle` guarantees a suffixed outcome, so every variant
    // that carries data is exercised rather than just the happy one.
    handle_create_schema(
        &service,
        serde_json::json!({
            "name": "Cycle",
            "description": "A bicycle in the shed",
            "fields": [{ "name": "colour", "type": "string", "protection": "user" }],
        }),
    )
    .await
    .expect("pre-existing schema");

    let report = install_recipe(&service, &linear()).await;
    let json: serde_json::Value = serde_json::to_value(&report)?;

    assert_eq!(json["recipeId"], "linear", "camelCase, not recipe_id");
    assert_eq!(json["success"], true);

    let steps = json["steps"].as_array().expect("steps is an array");
    for step in steps {
        assert!(step["label"].is_string());
        assert!(
            step["outcome"]["kind"].is_string(),
            "outcome is internally tagged on `kind`"
        );
    }

    let suffixed = steps
        .iter()
        .find(|s| s["outcome"]["kind"] == "suffixed")
        .expect("the colliding cycle must report a suffixed outcome");
    assert_eq!(suffixed["outcome"]["requested"], "cycle");
    assert!(suffixed["outcome"]["created"].is_string());

    let created = steps
        .iter()
        .find(|s| s["outcome"]["kind"] == "created")
        .expect("a non-colliding step must report created");
    assert!(created["outcome"]["id"].is_string());

    Ok(())
}

/// The installer reports every step as done, with nothing re-keyed, into an
/// empty workspace.
#[tokio::test]
async fn install_recipe_reports_every_step_created_in_a_clean_workspace() -> Result<()> {
    let (service, _tmp) = test_service().await?;
    let recipe = linear();

    let report = install_recipe(&service, &recipe).await;

    assert!(
        report.success,
        "install should succeed in a clean workspace; first failure: {:?}",
        report.failure()
    );
    assert!(
        report.suffixed().is_empty(),
        "nothing should be re-keyed when no id is taken"
    );

    let expected = recipe.schemas.len()
        + recipe.field_value_extensions.len()
        + recipe.plays.len()
        + recipe.skills.len();
    assert_eq!(report.steps.len(), expected, "one report row per step");

    for step in &report.steps {
        assert!(
            matches!(step.outcome, StepOutcome::Created { .. }),
            "{} should be Created, got {:?}",
            step.label,
            step.outcome
        );
    }
    Ok(())
}

/// A workspace that already has a `cycle` keeps it. The recipe's own cycle
/// lands under a suffixed id, and the report says so.
///
/// This is the acceptance criterion's core case: never silent adoption (the
/// stranger's `cycle` has none of the fields the Plays read), never silent
/// overwrite, and never a blocking dialog.
#[tokio::test]
async fn an_existing_schema_id_is_re_keyed_and_disclosed_not_adopted() -> Result<()> {
    let (service, _tmp) = test_service().await?;

    // Someone's unrelated "Cycle" — same name, entirely different shape.
    handle_create_schema(
        &service,
        serde_json::json!({
            "name": "Cycle",
            "description": "A bicycle in the shed",
            "fields": [{ "name": "colour", "type": "string", "protection": "user" }],
        }),
    )
    .await
    .expect("the pre-existing schema should be created");

    let report = install_recipe(&service, &linear()).await;
    assert!(
        report.success,
        "a collision must be resolved, not fatal; first failure: {:?}",
        report.failure()
    );

    let suffixed = report.suffixed();
    assert_eq!(suffixed.len(), 1, "exactly the cycle schema should re-key");
    assert_eq!(suffixed[0].0, "cycle");
    assert_ne!(suffixed[0].1, "cycle", "must land under a different id");

    // The pre-existing schema is untouched.
    let existing = service
        .get_schema_node("cycle")
        .await?
        .expect("the original cycle schema must survive");
    assert!(
        existing.get_field("colour").is_some(),
        "the user's own schema must be left exactly as it was"
    );
    assert!(
        existing.get_field("start_date").is_none(),
        "the recipe must not have written its fields into someone else's schema"
    );
    Ok(())
}

/// Re-keying rewrites later references, so the installed set stays internally
/// consistent rather than half-pointing at the stranger's schema.
#[tokio::test]
async fn re_keying_follows_through_to_the_plays_that_reference_it() -> Result<()> {
    let (service, _tmp) = test_service().await?;

    handle_create_schema(
        &service,
        serde_json::json!({
            "name": "Cycle",
            "description": "A bicycle in the shed",
            "fields": [{ "name": "colour", "type": "string", "protection": "user" }],
        }),
    )
    .await
    .expect("pre-existing schema");

    let report = install_recipe(&service, &linear()).await;
    assert!(report.success, "first failure: {:?}", report.failure());

    let new_id = report.suffixed()[0].1.to_string();

    let play = service
        .get_node("linear-cycle-rollover")
        .await?
        .expect("the rollover play should exist");
    let rules = play
        .properties
        .get("play")
        .and_then(|b| b.get("rules"))
        .or_else(|| play.properties.get("rules"))
        .expect("rules")
        .to_string();

    assert!(
        rules.contains(&new_id),
        "the rollover Play must target the re-keyed cycle ({new_id}), not the original"
    );
    Ok(())
}

/// Installing twice leaves the first install intact and re-keys the second,
/// rather than erroring out or overwriting.
#[tokio::test]
async fn installing_twice_re_keys_rather_than_failing_or_overwriting() -> Result<()> {
    let (service, _tmp) = test_service().await?;
    let recipe = linear();

    let first = install_recipe(&service, &recipe).await;
    assert!(first.success, "first failure: {:?}", first.failure());
    assert!(first.suffixed().is_empty());

    let second = install_recipe(&service, &recipe).await;
    assert!(
        second.success,
        "a second install should resolve collisions, not fail: {:?}",
        second.failure()
    );
    // Every id-bearing step collides the second time: both schemas and all
    // three play nodes. Vocabulary extensions do not — they target the
    // re-keyed schema, which has no values yet — and skills reconcile by
    // seed key rather than colliding.
    assert_eq!(
        second.suffixed().len(),
        recipe.schemas.len() + recipe.plays.len(),
        "each schema and play collides on a second install and must be re-keyed; got {:?}",
        second.suffixed()
    );

    // The originals still exist, unchanged.
    for step in &recipe.schemas {
        assert!(
            service.get_schema_node(step.schema_id).await?.is_some(),
            "{} from the first install must survive the second",
            step.schema_id
        );
    }
    Ok(())
}

/// A re-key must reach the seeded guidance, not just the schemas and Plays.
///
/// The skills name schema ids in prose. Left untouched after a re-key they
/// point at the stranger's schema the re-key existed to avoid — which has none
/// of the fields the guidance describes, so an agent following it writes
/// nonsense. Worse than a dangling reference, because it resolves.
#[tokio::test]
async fn a_re_key_is_disclosed_in_the_seeded_guidance() -> Result<()> {
    let (service, _tmp) = test_service().await?;

    handle_create_schema(
        &service,
        serde_json::json!({
            "name": "Cycle",
            "description": "A bicycle in the shed",
            "fields": [{ "name": "colour", "type": "string", "protection": "user" }],
        }),
    )
    .await
    .expect("pre-existing schema");

    let report = install_recipe(&service, &linear()).await;
    assert!(report.success, "first failure: {:?}", report.failure());
    let new_id = report.suffixed()[0].1.to_string();

    let guidance = seeded_guidance(&service, "Cycles").await?;
    assert!(
        guidance.contains(&new_id),
        "guidance must name the id the cycle schema actually landed under ({new_id}), \
         or it sends agents at someone else's schema"
    );
    Ok(())
}

/// The common case: nothing collided, so the guidance ships exactly as
/// authored with no note bolted on.
#[tokio::test]
async fn a_clean_install_leaves_the_guidance_unannotated() -> Result<()> {
    let (service, _tmp) = test_service().await?;
    let report = install_recipe(&service, &linear()).await;
    assert!(report.success);
    assert!(report.suffixed().is_empty());

    let guidance = seeded_guidance(&service, "Cycles").await?;
    assert!(
        !guidance.contains("Type names in this workspace"),
        "no rename note should appear when nothing was re-keyed"
    );
    Ok(())
}

/// The flattened markdown body of the seeded skill whose title contains
/// `title_fragment`.
async fn seeded_guidance(service: &Arc<NodeService>, title_fragment: &str) -> Result<String> {
    let skills = service.query_nodes_by_type("skill", Some("active")).await?;
    let skill = skills
        .iter()
        .find(|n| n.content.contains(title_fragment))
        .unwrap_or_else(|| panic!("no seeded skill titled like '{title_fragment}'"));

    // (root, nodes-by-id, children-by-parent) — the flattened body is every
    // descendant's content; order does not matter for a contains check.
    let (_root, nodes, _children) = service.get_subtree_data(&skill.id).await?;
    Ok(nodes
        .values()
        .map(|n| n.content.as_str())
        .collect::<Vec<_>>()
        .join("\n"))
}

/// A schema step that references an EARLIER step's schema must follow that
/// step's re-key.
///
/// Exercises `create_schema_resolving_collisions`' own rewrite, which the
/// shipped Linear recipe cannot reach: `issue` extends core `task` and
/// `cycle.tasks` targets core `task`, and a core type is never suffixed. A
/// unit test over `rewrite_schema_ids` does not cover this either — it proves
/// the helper works, not that this call site invokes it. Only a recipe whose
/// second schema names its first can tell the difference.
///
/// Without the rewrite the derived schema extends whatever stranger's schema
/// already held the id: wrong inherited fields, no error anywhere.
#[tokio::test]
async fn a_later_schema_step_follows_an_earlier_step_s_re_key() -> Result<()> {
    use nodespace_core::methodology::{MethodologyRecipe, SchemaStep};

    let (service, _tmp) = test_service().await?;

    // A squatter on the id the recipe's first schema wants, forcing a re-key.
    handle_create_schema(
        &service,
        serde_json::json!({
            "name": "Widget",
            "description": "Someone else's widget, unrelated to this recipe",
            "fields": [{ "name": "colour", "type": "string", "protection": "user" }],
        }),
    )
    .await
    .expect("squatter schema");

    let recipe = MethodologyRecipe {
        id: "two-step",
        name: "Two-step",
        description: "A recipe whose second schema extends its first.",
        schemas: vec![
            SchemaStep {
                schema_id: "widget",
                params: serde_json::json!({
                    "name": "Widget",
                    "description": "The recipe's own widget",
                    "fields": [{ "name": "size", "type": "string", "protection": "user" }],
                }),
            },
            SchemaStep {
                schema_id: "gadget",
                params: serde_json::json!({
                    "name": "Gadget",
                    "extends": "widget",
                    "description": "Extends the recipe's widget, not the stranger's",
                    "fields": [],
                }),
            },
        ],
        field_value_extensions: vec![],
        plays: vec![],
        skills: vec![],
    };

    let report = install_recipe(&service, &recipe).await;
    assert!(report.success, "install failed: {:?}", report.failure());

    let widget_id = report
        .suffixed()
        .first()
        .map(|(_, created)| created.to_string())
        .expect("the squatted widget must have been re-keyed");
    assert_ne!(widget_id, "widget");

    // The derived schema must extend the RE-KEYED widget. Reading the parent
    // back through the relationship table is what distinguishes "followed the
    // rename" from "silently extended the stranger".
    let gadget = service
        .get_schema_node("gadget")
        .await?
        .expect("gadget schema should exist");
    let parent = nodespace_core::schema::extends_chain::declared_parent(&gadget)
        .expect("gadget should declare an extends target");

    assert_eq!(
        parent, widget_id,
        "gadget must extend the recipe's own re-keyed widget ({widget_id}), \
         not the stranger's schema that squatted the original id"
    );

    Ok(())
}
