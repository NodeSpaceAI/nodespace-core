//! The skill confusion matrix: for every seeded skill, the requests it should
//! win, ranked against the real registry with the locked embedding model.
//!
//! A skill's `use_for` and `not_for` are retrieval text, and wording that
//! reads as equivalent ranks very differently. This is the record those texts
//! are written against: each request names the skill that owns it, and a run
//! shows where that skill ranks, by how much, and which neighbour is closest.
//! `REQUESTS_ANOTHER_SKILL_MUST_NOT_LEAD` holds the confusions: requests that
//! share a skill's vocabulary and belong to a different one.
//!
//! Two files are kept beside the prompt goldens, in
//! `tests/golden/skill_retrieval/`:
//!
//!   - `baseline.tsv`: the scores before the texts were audited. Frozen; it
//!     is the "before" column.
//!   - `current.tsv`: the scores of the seeded texts as they are. Rewrite it
//!     with `UPDATE_GOLDEN=1` after changing any `use_for` or `not_for`, and
//!     read the diff. `current.sha256` is a digest of the text it was recorded
//!     from, which a test in the gate holds the seed table to.
//!
//! Ranks are asserted; scores are recorded only, since they are compared
//! against each other and never against a fixed number.
//!
//! Ignored by default, like the rest of the live suite: it loads the real
//! embedding model. Run explicitly:
//!
//! ```text
//! cargo nextest run -p nodespace-agent --test it skill_confusion_matrix:: --run-ignored only --no-capture -j 1
//! ```

use std::fmt::Write as _;
use std::path::PathBuf;
use std::sync::Arc;

use nodespace_agent::local_agent::routing::{lookup_retrieval_query, RETRIEVAL_TOP_K};
use nodespace_agent::skill_pipeline::seed_skill_nodes;
use nodespace_core::models::{SkillFields, SkillRole};
use nodespace_core::ops::skill_ops::{find_skills, FindSkillsInput};
use nodespace_core::services::{NodeEmbeddingService, NodeService};
use sha2::{Digest, Sha256};

use crate::live_embedding_prefix_measurement::SKILL_CASES;
use crate::live_skill_retrieval_stability::{
    repeated_rankings, seed_and_embed_registry, TEXT_OVERRIDES_VAR,
};

/// Where the request's owner must rank, on every rep. A procedure skill is
/// ranked in its own lane (ADR-038): `find_skills` returns the best one after
/// the tool skills, so its owner holds the request by being that one, and
/// `First` and `Window` mean the same for it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Want {
    /// First: the skill's declared write-tool fields and any destructive
    /// tool are offered only from the leading candidate.
    First,
    /// Within the candidate window. For a request two skills can both serve,
    /// where either leading is correct.
    Window,
}

/// One request and the skill that owns it.
struct Case {
    request: &'static str,
    owner: &'static str,
    want: Want,
    /// The request is the topic of a lookup, which retrieval never sees bare:
    /// the router embeds [`lookup_retrieval_query`] of it.
    lookup: bool,
}

impl Case {
    /// What retrieval embeds for this request.
    fn query(&self) -> String {
        if self.lookup {
            lookup_retrieval_query(self.request)
        } else {
            self.request.to_string()
        }
    }
}

const fn first(request: &'static str, owner: &'static str) -> Case {
    Case {
        request,
        owner,
        want: Want::First,
        lookup: false,
    }
}

const fn window(request: &'static str, owner: &'static str) -> Case {
    Case {
        request,
        owner,
        want: Want::Window,
        lookup: false,
    }
}

/// A lookup of `topic`, which Research & Search must lead.
const fn lookup(topic: &'static str) -> Case {
    Case {
        request: topic,
        owner: "Research & Search",
        want: Want::First,
        lookup: true,
    }
}

/// Requests for the built-in skills, beyond the labelled set the embedding
/// prefix study already holds ([`SKILL_CASES`]). Weighted toward the skills
/// that set leaves thin, and toward wording a neighbour shares.
const CASES: &[Case] = &[
    // Research & Search, including lookups that name a state another skill
    // changes, or a subject another skill is about.
    lookup("the notes from last week's planning call"),
    lookup("unpaid invoices"),
    lookup("tasks that are done"),
    lookup("resolved incidents"),
    lookup("how offline sync works"),
    lookup("why we chose Postgres"),
    lookup("what is due this week"),
    lookup("duplicate customer records"),
    lookup("tasks that depend on the API migration"),
    lookup("notes in the Cooking collection"),
    // Node Creation: one record of a type that already exists.
    first("create a ticket for the login bug", "Node Creation"),
    first("add a new customer called Harbor Freight", "Node Creation"),
    first("new note: call the plumber", "Node Creation"),
    first("make a task to renew the domain", "Node Creation"),
    first(
        "log today's gig at the Blue Room, paid 300",
        "Node Creation",
    ),
    first("add another album: Blue Train by Coltrane", "Node Creation"),
    // Schema Creation: a kind of thing, or a change to one.
    first("add a severity field to tickets", "Schema Creation"),
    first(
        "rename the owner field on projects to lead",
        "Schema Creation",
    ),
    first(
        "I want to keep a record of the books I lend out",
        "Schema Creation",
    ),
    first(
        "define a new type for vendors with a name and a contact",
        "Schema Creation",
    ),
    first(
        "tickets should also have an environment, staging or production",
        "Schema Creation",
    ),
    // Graph Editing: an existing record changes and stays.
    first("rename the Q3 plan to Q3 roadmap", "Graph Editing"),
    first(
        "set the due date on the roofing task to Friday",
        "Graph Editing",
    ),
    first(
        "bump the priority on the login bug to high",
        "Graph Editing",
    ),
    first("the Camden invoice came in, mark it paid", "Graph Editing"),
    first(
        "change the amount on the Acme invoice to 2600",
        "Graph Editing",
    ),
    // Relationship Management: an edge between two records.
    // An edge to a decision is Recording a Decision's link step as much as
    // it is an edge: either may lead.
    window(
        "record an edge between the rebuild task and the storage decision",
        "Relationship Management",
    ),
    first(
        "connect the retro notes to the sprint they belong to",
        "Relationship Management",
    ),
    first(
        "this task depends on the API migration",
        "Relationship Management",
    ),
    first(
        "the launch task is blocked by the security review",
        "Relationship Management",
    ),
    first(
        "attach the contract to the Harbor Freight account",
        "Relationship Management",
    ),
    window(
        "which tasks depend on the API migration?",
        "Relationship Management",
    ),
    // Organization: membership of a collection.
    first("group these notes under Travel", "Organization"),
    first(
        "move the recipe notes into the Cooking collection",
        "Organization",
    ),
    first(
        "categorize these receipts as business expenses",
        "Organization",
    ),
    first(
        "add the roofing quote to the House collection",
        "Organization",
    ),
    // Node Deletion.
    first("delete the note about the plumber", "Node Deletion"),
    first("trash the old meeting notes", "Node Deletion"),
    first("erase that draft", "Node Deletion"),
    first("drop the cancelled gigs", "Node Deletion"),
    // Conflict Journal: inspect or dismiss a recorded collision.
    first("list the sync collisions", "Conflict Journal"),
    first("which records are colliding?", "Conflict Journal"),
    // Node Merge: two records become one.
    first("merge the two Sarah Chen records", "Node Merge"),
    first(
        "combine these duplicate customer records into one",
        "Node Merge",
    ),
    first(
        "these two contacts are the same person, merge them",
        "Node Merge",
    ),
    first(
        "fold the duplicate Acme account into the original",
        "Node Merge",
    ),
    first(
        "dedupe the two vendor entries for Harbor Freight",
        "Node Merge",
    ),
    // Play Workflow State: why an automation did nothing.
    first(
        "why didn't the play trigger for this task?",
        "Play Workflow State",
    ),
    first(
        "why hasn't the automation fired on the invoice?",
        "Play Workflow State",
    ),
    first(
        "what's still missing before the rule runs for this ticket?",
        "Play Workflow State",
    ),
    // Play Authoring: change an automation.
    first("turn off the rollover play", "Play Authoring"),
    first(
        "change the play so it runs when a task is marked done",
        "Play Authoring",
    ),
    first(
        "add a rule that archives invoices once they're paid",
        "Play Authoring",
    ),
    first("make the automation skip cancelled tasks", "Play Authoring"),
    // Bulk Import: a document becomes many nodes.
    first("import my notes folder", "Bulk Import"),
    first("bring in this markdown file as a document", "Bulk Import"),
    first("create nodes from this markdown outline", "Bulk Import"),
    first("import the README into the knowledge base", "Bulk Import"),
    // Writing a Spec: what is being built and how done is judged.
    first("write a spec for the CSV export feature", "Writing a Spec"),
    first(
        "spec out offline sync with its acceptance criteria",
        "Writing a Spec",
    ),
    first(
        "draft the requirements for the billing rework",
        "Writing a Spec",
    ),
    window("approve the offline sync spec", "Writing a Spec"),
    // Writing a Plan: how one spec will be met.
    first(
        "write an implementation plan for the export spec",
        "Writing a Plan",
    ),
    first(
        "plan out how we are going to build the offline sync spec",
        "Writing a Plan",
    ),
    first(
        "draft a plan for the billing spec with its risks",
        "Writing a Plan",
    ),
    // Breaking a Plan into Tasks.
    first(
        "break the export plan down into tasks",
        "Breaking a Plan into Tasks",
    ),
    first(
        "split this plan into work items with checklists",
        "Breaking a Plan into Tasks",
    ),
    first(
        "turn the approved plan into tasks",
        "Breaking a Plan into Tasks",
    ),
    // Implementing a Task: the work itself, from the queue to review.
    first("pick up the next ready task", "Implementing a Task"),
    first(
        "start working on the next task in the queue",
        "Implementing a Task",
    ),
    first("implement the export task", "Implementing a Task"),
    first("continue the task I was working on", "Implementing a Task"),
    // Reviewing a Task: the review queue.
    first(
        "review the task that is awaiting review",
        "Reviewing a Task",
    ),
    first("work the review queue", "Reviewing a Task"),
    first(
        "check the finished work on the export task",
        "Reviewing a Task",
    ),
    // Completing a Task: closing one, and a close that was refused.
    first("close out the export task", "Completing a Task"),
    first("it won't let me mark this task done", "Completing a Task"),
    window("mark the export task done", "Completing a Task"),
    // Recording a Decision.
    first(
        "record the decision to use Postgres for sessions",
        "Recording a Decision",
    ),
    first(
        "log an architecture decision: we are going with gRPC",
        "Recording a Decision",
    ),
    first(
        "supersede the old caching decision with this one",
        "Recording a Decision",
    ),
    // Authoring a Skill: an instruction written down for later.
    first("write a skill for how we cut releases", "Authoring a Skill"),
    first(
        "save this procedure so the agent follows it next time",
        "Authoring a Skill",
    ),
    first(
        "teach the agent our naming conventions",
        "Authoring a Skill",
    ),
    first(
        "update the release skill to mention the changelog",
        "Authoring a Skill",
    ),
];

/// The confusions: a request that shares `skill`'s vocabulary and belongs to
/// another, so `skill` must not lead it. Where the owner is one skill, the
/// request is also a [`Case`] above; these add the requests any of several
/// skills may serve, where only the wrong one is pinned.
const REQUESTS_ANOTHER_SKILL_MUST_NOT_LEAD: &[(&str, &str)] = &[
    // A record of a type that already exists is not a new kind of thing.
    ("create a ticket for the login bug", "Schema Creation"),
    (
        "add a new customer called Harbor Freight",
        "Schema Creation",
    ),
    (
        "add another album: Blue Train by Coltrane",
        "Schema Creation",
    ),
    (
        "log today's gig at the Blue Room, paid 300",
        "Schema Creation",
    ),
    // A saved query or view reads records; it defines no type.
    ("save a view of my overdue invoices", "Schema Creation"),
    ("create a saved query for open tickets", "Schema Creation"),
    ("make a filter for tasks due this week", "Schema Creation"),
    ("set up a board of tickets by status", "Schema Creation"),
    // A new kind of thing is not one record.
    ("I need a tracker for the venues I book", "Node Creation"),
    ("start keeping tabs on who owes me money", "Node Creation"),
    // Pointing one record at another is an edge, not a field.
    ("link this invoice to the Acme account", "Graph Editing"),
    (
        "attach the contract to the Harbor Freight account",
        "Graph Editing",
    ),
    // Membership of a collection is not an edge between two records.
    (
        "Add this note to my reading list collection",
        "Relationship Management",
    ),
    (
        "move the recipe notes into the Cooking collection",
        "Relationship Management",
    ),
    // Combining duplicates is a merge; the journal only lists and dismisses.
    ("merge the two Sarah Chen records", "Conflict Journal"),
    (
        "combine these duplicate customer records into one",
        "Conflict Journal",
    ),
    ("show me the open conflicts", "Node Merge"),
    // One record is not a document to import.
    ("add a task to call the roofer on Friday", "Bulk Import"),
    ("new note: call the plumber", "Bulk Import"),
    // One new task is a record, not a plan's breakdown and not work to do.
    (
        "make a task to renew the domain",
        "Breaking a Plan into Tasks",
    ),
    ("make a task to renew the domain", "Implementing a Task"),
    (
        "add a task to call the roofer on Friday",
        "Implementing a Task",
    ),
    // A field or a state of some other record is not a task being closed.
    (
        "the Camden invoice came in, mark it paid",
        "Completing a Task",
    ),
    (
        "set the due date on the roofing task to Friday",
        "Completing a Task",
    ),
    // An automation is a play, not a written instruction.
    (
        "add a rule that archives invoices once they're paid",
        "Authoring a Skill",
    ),
    ("turn off the rollover play", "Authoring a Skill"),
    // A new kind of record is a type, not one decision or one spec.
    (
        "define a new type for vendors with a name and a contact",
        "Writing a Spec",
    ),
    (
        "I want to keep a record of the books I lend out",
        "Recording a Decision",
    ),
];

/// Requests in the matrix that their owner does not yet win: measured and
/// recorded, not asserted. Most score under 0.75 for every skill, where a
/// proper noun or an amount decides the order more than any wording does.
///
/// The list is a ratchet. A wording that wins one of these fails the matrix
/// until the request is taken off, and a request that newly misses is a
/// failure, not an entry, so the list only shrinks. It excuses an owner's
/// miss and nothing else: a request listed here that is also in
/// [`REQUESTS_ANOTHER_SKILL_MUST_NOT_LEAD`] still fails if that skill leads it.
const NOT_YET_WON: &[&str] = &[
    // One new record, which a skill about existing records leads.
    "make a task to renew the domain",
    "new note: call the plumber",
    "log today's gig at the Blue Room, paid 300",
    "log a new invoice for Acme, $2400, due next month",
    "add another album: Blue Train by Coltrane",
    // A change to a type's fields, a new rule, or a new kind of record,
    // worded like an update or a lookup.
    "add a severity field to tickets",
    "rename the owner field on projects to lead",
    "add a rule that archives invoices once they're paid",
    "I want to keep a record of the books I lend out",
    // An update that names an amount or a payment.
    "change the amount on the Acme invoice to 2600",
    "the Camden invoice came in, mark it paid",
    "the 2400 one came back, set it to returned",
    // A dependency stated as a fact or asked as a question, with no linking
    // verb. The question is a lookup once Stage 1 has routed it, which leads
    // with Research & Search; that skill searches and does not hold the
    // traversal tool, so either way the question is answered without it.
    "this task depends on the API migration",
    "the launch task is blocked by the security review",
    // Filing with no word for a collection, or by a proper noun.
    "put the roofing notes in with the house stuff",
    "add the roofing quote to the House collection",
    // Duplicates: three skills are about them.
    "get rid of the duplicate customer record",
    "dedupe the two vendor entries for Harbor Freight",
];

/// Labelled requests whose owner changed when `decision` became a core type
/// with a skill of its own (ADR-092). Each names a decision to record or to
/// link, which Recording a Decision now does: it creates the decision and
/// makes the link from the spec or task it constrains. Before there was such
/// a type, logging decisions meant defining one, and pointing a task at a
/// decision was a bare edge.
const REOWNED_BY_RECORDING_A_DECISION: &[&str] = &[
    "log the design decisions for the system and who authored each one",
    "create a record of who made which architecture decision",
    "point rebuild task at the decision it has to respect",
];

/// The labelled set of the embedding prefix study, as window cases, under
/// the owner each request has now.
fn labelled_cases() -> Vec<Case> {
    SKILL_CASES
        .iter()
        .map(|(request, owner)| {
            if REOWNED_BY_RECORDING_A_DECISION.contains(request) {
                window(request, "Recording a Decision")
            } else {
                window(request, owner)
            }
        })
        .collect()
}

/// Every skill's score for `query`, best first.
async fn scores(
    embedding_service: &Arc<NodeEmbeddingService>,
    node_service: &Arc<NodeService>,
    query: &str,
) -> Vec<(String, f64)> {
    find_skills(
        embedding_service,
        node_service,
        FindSkillsInput {
            query: query.to_string(),
            limit: Some(10),
        },
    )
    .await
    .expect("find_skills must succeed")
    .skills
    .iter()
    .filter(|s| s.get("kind").and_then(|v| v.as_str()) == Some("skill"))
    .map(|s| {
        (
            s.get("name")
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_string(),
            s.get("confidence")
                .and_then(|v| v.as_f64())
                .unwrap_or_default(),
        )
    })
    .collect()
}

fn golden_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/golden/skill_retrieval/current.tsv")
}

fn digest_path() -> PathBuf {
    golden_path().with_extension("sha256")
}

/// A digest of what `current.tsv` was recorded from: every seeded skill's
/// name, `use_for` and `not_for`, and every request the matrix ranks.
fn seed_text_digest() -> String {
    let mut hasher = Sha256::new();
    let mut part = |text: &str| {
        hasher.update(text.as_bytes());
        hasher.update([0]);
    };
    for template in seed_skill_nodes() {
        let fields = SkillFields::from_properties(&template.root_properties)
            .expect("seed decodes as a skill");
        part(&template.title);
        part(&fields.use_for);
        part(fields.not_for.as_deref().unwrap_or(""));
    }
    for case in labelled_cases() {
        part(case.request);
        part(case.owner);
    }
    for case in CASES {
        part(&case.query());
        part(case.owner);
        part(&format!("{:?}", case.want));
    }
    for (request, skill) in REQUESTS_ANOTHER_SKILL_MUST_NOT_LEAD {
        part(request);
        part(skill);
    }
    format!("{:x}\n", hasher.finalize())
}

/// Runs in the gate, with no model. The matrix itself is ignored by default,
/// so this is what stops a `use_for` or `not_for` being reworded, or a
/// request added, without the matrix being recorded again: the recording
/// carries a digest of what it was made from. It shows the recording is of
/// this text, not that the live guards were run; the message says to run
/// them.
#[test]
fn the_recorded_matrix_is_of_the_seeded_text() {
    let recorded = std::fs::read_to_string(digest_path()).expect("read the recorded digest");
    assert_eq!(
        recorded,
        seed_text_digest(),
        "a skill's use_for or not_for, or the matrix's requests, changed since current.tsv was \
         recorded: run the confusion matrix and the live guards on the locked model, then record \
         with UPDATE_GOLDEN=1"
    );
}

/// Rank every request and confusion against the seeded skills, record the
/// scores, and fail on a request whose owner misses its place, or that the
/// wrong skill leads, unless it is listed as not yet won.
#[tokio::test]
#[ignore = "requires the locked nomic-embed-text-v1.5 GGUF on disk"]
async fn skills_win_their_requests() {
    let registry = seed_skill_nodes();
    let installed: Vec<String> = registry.iter().map(|t| t.title.clone()).collect();
    let procedures: Vec<String> = registry
        .iter()
        .filter(|t| {
            SkillFields::from_properties(&t.root_properties)
                .expect("seed decodes as a skill")
                .role
                == SkillRole::Procedure
        })
        .map(|t| t.title.clone())
        .collect();
    let Some((es, ns, _tmp)) = seed_and_embed_registry(registry).await else {
        return;
    };

    let mut table = String::from(
        "request\towner\twant\trank\tscore\tleader\tleader_score\tclosest_other\tmargin\n",
    );
    // (request, the line to print, whether `NOT_YET_WON` may excuse it)
    let mut failures: Vec<(&str, String, bool)> = Vec::new();

    let labelled = labelled_cases();
    for case in labelled.iter().chain(CASES) {
        let Case {
            request,
            owner,
            want,
            ..
        } = *case;
        let query = case.query();
        assert!(
            installed.iter().any(|title| title == owner),
            "{owner:?} owns {request:?} but is not a seeded skill"
        );
        let ranked = scores(&es, &ns, &query).await;
        let rank = ranked.iter().position(|(skill, _)| skill == owner);
        let own_score = rank.map(|i| ranked[i].1);
        let (leader, leader_score) = ranked.first().cloned().unwrap_or_default();
        let closest_other = ranked.iter().find(|(skill, _)| skill != owner);
        let margin = match (own_score, closest_other) {
            (Some(own), Some((_, other))) => format!("{:+.3}", own - other),
            _ => "-".to_string(),
        };
        let _ = writeln!(
            table,
            "{query}\t{owner}\t{want:?}\t{}\t{}\t{leader}\t{leader_score:.3}\t{}\t{margin}",
            rank.map_or("-".to_string(), |i| (i + 1).to_string()),
            own_score.map_or("-".to_string(), |s| format!("{s:.3}")),
            closest_other.map_or("-", |(skill, _)| skill.as_str()),
        );

        let held = repeated_rankings(&es, &ns, &query)
            .await
            .iter()
            .all(|top| match want {
                _ if procedures.iter().any(|p| p == owner) => {
                    top.iter().any(|skill| skill == owner)
                }
                Want::First => top.first().is_some_and(|skill| skill == owner),
                Want::Window => top.iter().any(|skill| skill == owner),
            });
        if !held {
            let top: Vec<String> = ranked
                .iter()
                .take(RETRIEVAL_TOP_K + 1)
                .map(|(skill, score)| format!("{skill}={score:.3}"))
                .collect();
            failures.push((
                request,
                format!("{query:?} wants {owner} {want:?}: {top:?}"),
                true,
            ));
        }
    }

    for (request, skill) in REQUESTS_ANOTHER_SKILL_MUST_NOT_LEAD {
        // A procedure never leads a request, so its pairs hold by construction.
        if procedures.iter().any(|p| p == skill) {
            continue;
        }
        let ranked = scores(&es, &ns, request).await;
        let (leader, leader_score) = ranked.first().cloned().unwrap_or_default();
        let wrong_score = ranked
            .iter()
            .find(|(candidate, _)| candidate == skill)
            .map(|(_, score)| *score);
        let _ = writeln!(
            table,
            "{request}\tnot {skill}\tNotFirst\t-\t{}\t{leader}\t{leader_score:.3}\t-\t-",
            wrong_score.map_or("-".to_string(), |s| format!("{s:.3}")),
        );
        let led = repeated_rankings(&es, &ns, request)
            .await
            .iter()
            .any(|top| top.first().is_some_and(|first| first == skill));
        if led {
            failures.push((
                request,
                format!("{request:?} must not lead with {skill}"),
                false,
            ));
        }
    }

    let trial = std::env::var(TEXT_OVERRIDES_VAR).is_ok();
    // A trial wording is never recorded as the seeded texts' scores.
    if std::env::var("UPDATE_GOLDEN").is_ok() && !trial {
        let path = golden_path();
        std::fs::create_dir_all(path.parent().expect("golden dir")).expect("create golden dir");
        std::fs::write(&path, &table).expect("write matrix");
        std::fs::write(digest_path(), seed_text_digest()).expect("write digest");
        eprintln!("wrote {}", path.display());
    }
    eprintln!("{table}");

    let (open, unexpected): (Vec<_>, Vec<_>) = failures
        .iter()
        .partition(|(request, _, excusable)| *excusable && NOT_YET_WON.contains(request));
    let lines = |misses: &[&(&str, String, bool)]| {
        misses
            .iter()
            .map(|(_, line, _)| line.as_str())
            .collect::<Vec<_>>()
            .join("\n")
    };
    eprintln!(
        "{} of {} request(s) miss their skill, {} of them recorded as not yet won:\n{}",
        failures.len(),
        table.lines().count() - 1,
        open.len(),
        lines(&failures.iter().collect::<Vec<_>>())
    );

    // A trial wording is judged by what it wins and loses above, not held to
    // the seeds' list, and is never a pass: a variable left set would
    // otherwise turn every run green.
    assert!(
        !trial,
        "trial wording from {TEXT_OVERRIDES_VAR}: the results above are a measurement, not a pass"
    );
    assert!(
        unexpected.is_empty(),
        "{} request(s) miss their skill:\n{}",
        unexpected.len(),
        lines(&unexpected)
    );
    // The list is a ratchet: a request that is won comes off it.
    let stale: Vec<&str> = NOT_YET_WON
        .iter()
        .copied()
        .filter(|request| {
            !failures
                .iter()
                .any(|(missed, _, excusable)| *excusable && missed == request)
        })
        .collect();
    assert!(
        stale.is_empty(),
        "now won, so remove from NOT_YET_WON: {stale:?}"
    );
}
