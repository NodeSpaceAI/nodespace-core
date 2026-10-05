//! Retrieval quality under nomic-embed-text-v1.5's asymmetric task prefixes.
//!
//! nomic-embed-text-v1.5 expects stored content embedded as
//! `search_document: …` and the text being searched with as
//! `search_query: …`. This measures every query-side retrieval path the agent
//! uses — skill retrieval (`find_skills`), schema retrieval (typed KNN over the
//! history-blended query `build_retrieval_query` builds), node search
//! (`semantic_search_nodes`) and context-assembly neighbours (a seed node's
//! content used as the query) — and prints the score distribution each
//! threshold and routing bar is set against.
//!
//! Every path goes through production code with a real model, so the report
//! reflects whatever prefix the query side actually uses. The first thing it
//! prints is which prefix that is, checked by comparing the production query
//! vector against the engine's `embed_query` and `embed_document` outputs.
//!
//! Production embeds queries with `search_document` (see
//! `NodeEmbeddingService::embed_query_text` for the measured reason). To
//! measure the `search_query` arm, switch that function's `embed_document`
//! call to `embed_query` locally and run this again; the header line confirms
//! which arm ran.
//!
//! Ignored by default — loads a real embedding model from the standard
//! NodeSpace catalog path. Run explicitly:
//!
//! ```text
//! cargo test -p nodespace-agent --test it live_embedding_prefix_measurement:: -- --ignored --nocapture
//! ```

use std::collections::HashMap;
use std::sync::Arc;

use nodespace_agent::agent_types::SkillCandidate;
use nodespace_agent::local_agent::routing::{score_bar_for, RETRIEVAL_TOP_K};
use nodespace_agent::skill_pipeline::seed_skill_nodes;
use nodespace_core::db::SqliteStore;
use nodespace_core::markdown::prepare_nodes_from_template;
use nodespace_core::ops::context_ops::build_retrieval_query;
use nodespace_core::ops::skill_ops::{find_skills, FindSkillsInput};
use nodespace_core::schema::handle_create_schema;
use nodespace_core::services::node_service::CreateNodeParams;
use nodespace_core::services::{
    InsertPositionOwned, NodeAccessor, NodeEmbeddingService, NodeService,
};
use nodespace_nlp_engine::{EmbeddingConfig, EmbeddingService};
use serde_json::json;
use tempfile::TempDir;

/// Copies of production's private constants — check they still match before
/// trusting a run. `context_ops::SCHEMA_SIMILARITY_THRESHOLD` /
/// `MAX_SEMANTIC_SCHEMAS`.
const SCHEMA_THRESHOLD: f64 = 0.2;
const MAX_SEMANTIC_SCHEMAS: usize = 5;
/// `SEMANTIC_THRESHOLD` in `local_agent/tools.rs` and `context_assembly.rs`.
const SEMANTIC_THRESHOLD: f32 = 0.3;
/// `context_assembly::NEIGHBORS_PER_SEED`.
const NEIGHBORS_PER_SEED: usize = 5;

// ---------------------------------------------------------------------------
// Corpora
// ---------------------------------------------------------------------------

/// (request, skill that should win). The live routing suite's requests plus
/// the indirect intents from the skill-retrieval wording study.
const SKILL_CASES: &[(&str, &str)] = &[
    (
        "Start keeping the calls we make on how the system is built, and who made each one",
        "Schema Creation",
    ),
    (
        "track architecture decisions and who made them",
        "Schema Creation",
    ),
    (
        "log the design decisions for the system and who authored each one",
        "Schema Creation",
    ),
    (
        "create a record of who made which architecture decision",
        "Schema Creation",
    ),
    (
        "keep a history of decisions made about how the system is built",
        "Schema Creation",
    ),
    (
        "track equipment checkout and return status",
        "Schema Creation",
    ),
    ("track albums I mean to listen to", "Schema Creation"),
    ("I need a tracker for the venues I book", "Schema Creation"),
    ("start keeping tabs on who owes me money", "Schema Creation"),
    (
        "set up something to log my freelance gigs",
        "Schema Creation",
    ),
    ("add a priority field to my invoices", "Schema Creation"),
    (
        "point rebuild task at the decision it has to respect",
        "Relationship Management",
    ),
    (
        "link this invoice to the Acme account",
        "Relationship Management",
    ),
    (
        "Add a new task to follow up with the vendor next week",
        "Node Creation",
    ),
    (
        "put down Kind of Blue, it's by Miles Davis",
        "Node Creation",
    ),
    ("add a task to call the roofer on Friday", "Node Creation"),
    (
        "log a new invoice for Acme, $2400, due next month",
        "Node Creation",
    ),
    (
        "Add this note to my reading list collection",
        "Organization",
    ),
    ("file these under the Q3 folder", "Organization"),
    (
        "put the roofing notes in with the house stuff",
        "Organization",
    ),
    (
        "The incident Rowan was on call for — mark it resolved",
        "Graph Editing",
    ),
    ("mark the incident as resolved", "Graph Editing"),
    ("mark incident resolved", "Graph Editing"),
    ("set the incident's resolved field to true", "Graph Editing"),
    ("mark the invoice as paid", "Graph Editing"),
    ("close out the support ticket", "Graph Editing"),
    ("mark the outage report done", "Graph Editing"),
    ("remove the due date from the launch task", "Graph Editing"),
    (
        "clear the assignee on the onboarding ticket",
        "Graph Editing",
    ),
    (
        "the 2400 one came back, set it to returned",
        "Graph Editing",
    ),
    ("mark the Camden invoice as paid", "Graph Editing"),
    ("that rehearsal got cancelled, update it", "Graph Editing"),
    (
        "resolve the conflict between the two Sarah Chen records",
        "Conflict Journal",
    ),
    ("show me the open conflicts", "Conflict Journal"),
    (
        "dismiss that duplicate collision, it's fine",
        "Conflict Journal",
    ),
    ("are there any unresolved conflicts?", "Conflict Journal"),
    (
        "keep the existing node for that conflict",
        "Conflict Journal",
    ),
    ("delete the resolved incidents", "Node Deletion"),
    ("get rid of all the resolved bugs", "Node Deletion"),
    ("purge resolved alerts from last month", "Node Deletion"),
    ("delete the incident, it's resolved", "Node Deletion"),
    ("remove the resolved tickets", "Node Deletion"),
    ("remove the closed tickets", "Node Deletion"),
    ("remove the done tasks", "Node Deletion"),
    ("remove the completed items", "Node Deletion"),
    ("remove the paid invoices", "Node Deletion"),
    ("get rid of the paid invoices", "Node Deletion"),
    ("get rid of the duplicate customer record", "Node Deletion"),
    (
        "what did I write about the Halvorsen meeting?",
        "Research & Search",
    ),
    ("what equipment is on the books?", "Research & Search"),
    ("run through the albums for me", "Research & Search"),
    ("load this markdown doc in as notes", "Bulk Import"),
];

/// (key, name, description) — the workspace from the schema-retrieval study.
const SCHEMAS: &[(&str, &str, &str)] = &[
    (
        "invoice",
        "Invoice",
        "A billing invoice linked to a customer, with amount, due date, and payment status.",
    ),
    (
        "customer",
        "Customer",
        "A person or company we do business with, including contact details.",
    ),
    (
        "equipment_item",
        "Equipment Item",
        "A piece of equipment tracked for checkout and return, with serial number and condition.",
    ),
    (
        "album",
        "Album",
        "A music album to listen to, with artist, release year, and listened status.",
    ),
    (
        "venue",
        "Venue",
        "A performance venue that can be booked, with capacity and location.",
    ),
    (
        "freelance_gig",
        "Freelance Gig",
        "A freelance job taken on, with client, rate, deadline, and booking status.",
    ),
    (
        "job_application",
        "Job Application",
        "A job application submitted, with company, role, date applied, and current stage.",
    ),
    (
        "plant",
        "Plant",
        "A plant in the greenhouse, with species, potting date, and health status.",
    ),
    (
        "service_record",
        "Service Record",
        "A maintenance service performed on a vehicle or machine, with mileage and cost.",
    ),
    (
        "wine_bottle",
        "Wine Bottle",
        "A bottle of wine in the cellar, with producer, vintage, and drink-by window.",
    ),
    (
        "short_story",
        "Short Story",
        "A short story submitted to a magazine, with title, market, and submission status.",
    ),
    (
        "conference_talk",
        "Conference Talk",
        "A talk proposed or delivered at a conference, with event, city, and acceptance status.",
    ),
];

/// (prior turns oldest-first, latest message, schema key that must be
/// retrieved, label). Single-message cases have no prior turns.
const SCHEMA_CASES: &[(&[&str], &str, &str, &str)] = &[
    (
        &[],
        "Add an invoice for $500 due next Friday",
        "invoice",
        "single",
    ),
    (
        &[],
        "Log a new gig for the Peterson wedding at 800 a day",
        "freelance_gig",
        "single",
    ),
    (
        &[],
        "Put down that I applied to Redwood Analytics yesterday",
        "job_application",
        "single",
    ),
    (
        &[],
        "Record that the van hit 60k miles and got an oil change",
        "service_record",
        "single",
    ),
    (
        &[],
        "Note that I sent Winter Light out to Granta last week",
        "short_story",
        "single",
    ),
    (&[], "Who still owes me money?", "invoice", "single"),
    (
        &[],
        "Which of the loaner units hasn't come back?",
        "equipment_item",
        "single",
    ),
    (
        &[],
        "What have I got that's ready to drink now?",
        "wine_bottle",
        "single",
    ),
    (
        &[],
        "Anything I haven't gotten around to listening to?",
        "album",
        "single",
    ),
    (&[], "Where am I playing next month?", "venue", "single"),
    (&[], "Mark the Camden one as paid", "invoice", "single"),
    (
        &[],
        "The fiddle-leaf is looking rough, update it",
        "plant",
        "single",
    ),
    (
        &[],
        "That talk got accepted — change the status",
        "conference_talk",
        "single",
    ),
    (&[], "Show me all customers", "customer", "single"),
    (&[], "List my equipment items", "equipment_item", "single"),
    (
        &[
            "Start tracking the venues I book",
            "Created the Venue type with capacity and location.",
        ],
        "Add the Fillmore, capacity 1200",
        "venue",
        "follow-up",
    ),
    (
        &[
            "Set up something to track the wine in my cellar",
            "Created the Wine Bottle type.",
        ],
        "Put down two of the 2018 Barolo",
        "wine_bottle",
        "follow-up",
    ),
    (
        &[
            "I want to log the gear we lend out",
            "Created the Equipment Item type.",
        ],
        "The U87, serial 4417",
        "equipment_item",
        "follow-up",
    ),
    (
        &[
            "Show me the job applications I've got open",
            "Found 3 open applications.",
        ],
        "Set the Redwood one to rejected",
        "job_application",
        "follow-up",
    ),
    (
        &[
            "Track the talks I pitch to conferences",
            "Created the Conference Talk type.",
        ],
        "Add one for Strange Loop in September",
        "conference_talk",
        "follow-up",
    ),
    (
        &[
            "Start tracking the venues I book",
            "Created the Venue type with capacity and location.",
        ],
        "Which ones hold more than a thousand?",
        "venue",
        "follow-up",
    ),
    (
        &[
            "Log the gear we lend out",
            "Created the Equipment Item type.",
        ],
        "Which ones are still out?",
        "equipment_item",
        "follow-up",
    ),
    (
        &["Put down two of the 2018 Barolo", "Added two bottles."],
        "Actually make that three",
        "wine_bottle",
        "follow-up",
    ),
    (
        &[
            "Start tracking the venues I book",
            "Created the Venue type with capacity and location.",
        ],
        "Actually, mark invoice 44 as paid",
        "invoice",
        "switch",
    ),
    (
        &["Add the Fillmore, capacity 1200", "Added the Fillmore."],
        "What albums haven't I listened to?",
        "album",
        "switch",
    ),
    (
        &[
            "Which of the loaner units are still out?",
            "Two units are still checked out.",
        ],
        "The fiddle-leaf fig is looking rough, update it",
        "plant",
        "switch",
    ),
    (
        &[
            "Log the van's oil change at 60k miles",
            "Recorded the service.",
        ],
        "Who still owes me money?",
        "invoice",
        "switch",
    ),
    (
        &[
            "Track the short stories I send out",
            "Created the Short Story type.",
        ],
        "Where am I playing next month?",
        "venue",
        "switch",
    ),
];

/// (topic, note). Three notes per topic, so a note's same-topic siblings are
/// the neighbours context assembly should surface for it.
const NOTES: &[(&str, &str)] = &[
    ("kitchen", "Kitchen renovation: the contractor quoted $38k for cabinets, quartz counters and moving the sink to the island."),
    ("kitchen", "Picked the backsplash tile for the kitchen remodel — white zellige, installer comes after the counters are templated."),
    ("kitchen", "Electrician says the kitchen needs a new 50A circuit for the induction range before the remodel can pass inspection."),
    ("marathon", "Marathon training week 9: long run of 18 miles at 9:10 pace, left calf tight after mile 14."),
    ("marathon", "Race-day fueling plan for the Chicago marathon: a gel every 40 minutes, electrolytes at each aid station."),
    ("marathon", "Taper schedule for the last three weeks before the marathon — cut mileage 20%, keep one tempo run."),
    ("launch", "Q3 product launch: pricing page and onboarding emails must ship before the September 15 announcement."),
    ("launch", "Launch readiness review — support team still needs the FAQ and the release notes for the new billing tier."),
    ("launch", "Press embargo for the Q3 launch lifts at 9am Pacific; analyst briefings are booked the week before."),
    ("sourdough", "Sourdough starter feeding: 1:5:5 ratio twice a day, it doubles in about six hours at 76°F."),
    ("sourdough", "Last loaf came out dense — probably underproofed; try a longer bulk ferment and a warmer spot."),
    ("sourdough", "Bread recipe: 80% hydration, 20% whole wheat, bake in the Dutch oven at 500°F lid on for 20 minutes."),
    ("vendor", "Acme renewal: they want a 12% price increase on the support contract; counter at 5% with a two-year term."),
    ("vendor", "Legal flagged the Acme contract's auto-renewal clause and the uncapped liability section for redlines."),
    ("vendor", "Call with Acme's account manager — they'll drop the increase to 7% if we commit to the premium SLA."),
    ("school", "Parent-teacher conference for Maya on Thursday; her teacher wants to talk about reading level and homework."),
    ("school", "Field trip permission slip and $15 for the science museum are due to Maya's school by Friday."),
    ("school", "Maya's school science fair project: growing bean plants under different colored lights."),
    ("network", "Home network: replaced the ISP router with a mesh system, three nodes, backhaul over ethernet."),
    ("network", "Set up a separate VLAN for the smart-home devices so the cameras can't reach the laptops."),
    ("network", "Wi-Fi keeps dropping in the office upstairs — moved the mesh node and changed to channel 44."),
    ("reading", "Notes on Thinking, Fast and Slow: System 1 is fast and intuitive, System 2 slow and deliberate."),
    ("reading", "Kahneman's anchoring examples — arbitrary numbers shift people's estimates even when they know they're random."),
    ("reading", "The planning fallacy chapter: we underestimate how long our own projects take, use the outside view instead."),
];

/// (search query, index into `NOTES` that must come back).
const NODE_QUERIES: &[(&str, usize)] = &[
    ("how much will the cabinets cost", 0),
    ("what tile did we choose", 1),
    ("does the stove need new wiring", 2),
    ("how did my longest run go", 3),
    ("what should I eat during the race", 4),
    ("how do I reduce training before race day", 5),
    ("what has to be done before the announcement", 6),
    ("is support ready for the release", 7),
    ("when can journalists publish", 8),
    ("how often do I feed the starter", 9),
    ("why was my bread heavy", 10),
    ("what temperature do I bake at", 11),
    ("what price increase is the vendor asking for", 12),
    ("which contract clauses worry legal", 13),
    ("did the supplier offer a better deal", 14),
    ("when is the meeting with Maya's teacher", 15),
    ("what does the school need by Friday", 16),
    ("what is the science project about", 17),
    ("what wifi hardware do we have at home", 18),
    ("how are the security cameras isolated", 19),
    ("why is the internet flaky upstairs", 20),
    ("what are the two systems of thinking", 21),
    ("how do random numbers bias estimates", 22),
    ("why do projects run late", 23),
];

/// Documents long enough to split into several chunks (a chunk is ~1,536
/// characters), each answering its queries in one paragraph among many. The
/// asymmetric prefixes are designed for exactly this: a short query against a
/// long passage.
const LONG_DOCS: &[&str] = &[
    "Minutes: Platform team weekly sync\n\n\
     Attendees were Priya, Tomas, Wen and Alex. Priya opened with the roadmap review: the \
     data export feature slipped a sprint because the CSV serializer rewrite took longer than \
     planned, and the team agreed it now targets the end of October. Tomas asked whether the \
     export should include archived records; the group decided archived records stay out of \
     the first release and get a follow-up ticket.\n\n\
     Wen walked through the on-call rotation. The pager load dropped from eleven pages a week \
     to four after the retry fix landed, but two of the remaining four were the same flaky \
     disk-space alert on the build runners. Wen will raise the alert threshold from 80% to 90% \
     and add automatic cleanup of old build caches so the alert only fires when something is \
     genuinely wrong.\n\n\
     Alex raised hiring. The backend role has three candidates in the final round, and the \
     panel needs one more interviewer who has worked on the billing service. Tomas volunteered. \
     Offers should go out within two weeks if the panel reaches a decision on Friday.\n\n\
     The last item was the database upgrade. The team will move the primary cluster from \
     version 14 to 16 during the maintenance window on the second Saturday of November, with a \
     full rehearsal on the staging cluster the week before. Rollback is a snapshot restore, \
     which the team measured at about forty minutes on staging data. Priya asked everyone to \
     read the upgrade runbook before the rehearsal and leave comments on anything unclear.",
    "Itinerary: two weeks in Japan\n\n\
     We land at Haneda on the 3rd at 6:40am and take the monorail into the city. The first four \
     nights are at a small hotel in Asakusa, walking distance from Senso-ji. The plan for Tokyo \
     is loose: Tsukiji outer market for breakfast one morning, the Mori art museum, an evening \
     in Shimokitazawa for the record shops and vintage stores, and a day trip to Kamakura to see \
     the big Buddha if the weather holds.\n\n\
     On the 7th we pick up the rail passes at Tokyo station and take the shinkansen to Kyoto, \
     roughly two hours and fifteen minutes. Reserved seats are on the right side of the train \
     so we can see Mount Fuji on the way. In Kyoto we are staying in a ryokan near Gion for three \
     nights; dinner is included on the first night only, and it is a kaiseki meal served in the \
     room, so we need to be back by 6pm that evening.\n\n\
     From Kyoto we do Nara as a half day for the deer park and Todai-ji, then continue to Osaka \
     for two nights. Osaka is mostly about food: okonomiyaki in Dotonbori, a takoyaki crawl, and \
     the Kuromon market. We booked a cooking class on the second Osaka evening.\n\n\
     The last stretch is three nights in Hakone at an onsen inn with a private outdoor bath. \
     Getting there means the shinkansen back to Odawara and then the Hakone Tozan railway. The \
     flight home leaves Narita on the 17th at 5:25pm, so we should leave Hakone by 10am that \
     day and send the big suitcases ahead by luggage forwarding two days earlier.",
    "Design doc: offline sync for the mobile app\n\n\
     Problem. Field technicians lose connectivity in basements and plant rooms, and the app \
     currently discards any edit made while offline. Support logs show about 6% of work orders \
     need to be re-entered by hand because of this. The goal is to let a technician keep working \
     offline for a full shift and have every edit reach the server once the device reconnects.\n\n\
     Approach. Every edit is written to a local operation log in SQLite before it is applied to \
     the local view. When the device is online, a background worker drains the log in order, \
     sending each operation with a client-generated id so the server can deduplicate retries. \
     We considered CRDTs and rejected them for now: work orders are edited by one technician at a \
     time, so conflicts are rare and a simpler model is enough.\n\n\
     Conflicts. When the server has a newer version of a field than the one the operation was \
     based on, the server keeps its value and returns the rejected edit, and the app shows it to \
     the technician in a review list instead of silently dropping it. Attachments such as photos \
     upload separately and are never in conflict because they are append-only.\n\n\
     Rollout and risks. The operation log is capped at 5,000 entries; beyond that the app warns \
     the technician to reconnect. Battery usage of the sync worker must stay under 2% per hour, \
     which we will verify on the three oldest supported devices. The feature ships behind a \
     per-customer toggle and is enabled first for the two pilot customers in the utilities \
     sector.",
    "Onboarding guide for new support engineers\n\n\
     Welcome to the support team. Your first week is mostly shadowing: you will sit in on live \
     chat with a senior engineer, read the top fifty most-viewed help articles, and get access \
     to the ticketing system, the admin console and the internal status page. Ask your buddy for \
     anything you cannot find; that is what they are there for.\n\n\
     Ticket priorities work like this. P1 means the customer cannot use the product at all and \
     must get a human response within fifteen minutes, day or night; P1s page the on-call \
     engineer automatically. P2 is a major feature broken with no workaround and needs a response \
     within two business hours. Everything else is P3 and should be answered within one business \
     day. Never downgrade a P1 without talking to the on-call engineer first.\n\n\
     Refunds and credits. You can issue a credit of up to $200 on your own authority when the \
     customer lost work because of an outage. Anything larger, and any full refund, needs \
     approval from a team lead, which you request with the refund form in the admin console. \
     Always note the ticket number in the credit reason so finance can reconcile it.\n\n\
     Escalating to engineering. If you can reproduce a bug, file it in the engineering tracker \
     with steps, the account id and screenshots, and link it from the ticket. If you cannot \
     reproduce it but several customers report the same thing, post in the incidents channel \
     instead; three similar reports within an hour is the informal threshold for treating it as \
     a possible incident.",
    "Postmortem: checkout outage on March 12\n\n\
     Summary. For 47 minutes, from 14:03 to 14:50 UTC, roughly a third of checkout attempts failed \
     with a timeout. About 2,100 orders were affected. No payments were double charged, and every \
     affected customer who retried after 14:50 completed their order.\n\n\
     Root cause. A configuration change that morning lowered the connection pool size for the \
     payments service from 200 to 20, a typo in the intended value of 120. The change passed \
     review because the diff showed only the one number and the reviewer read it as a small \
     tweak. Under normal morning load 20 connections were enough, so nothing failed until the \
     afternoon traffic peak exhausted the pool and requests started queueing past the gateway \
     timeout.\n\n\
     Detection and response. The error-rate alert fired at 14:09, six minutes in, because it \
     requires five minutes of sustained errors. The on-call engineer first suspected the payment \
     provider and spent twenty minutes on their status page before looking at our own pool \
     metrics. Reverting the config change at 14:48 resolved it within two minutes.\n\n\
     Action items. Add validation that rejects a pool size change of more than 50% without an \
     explicit override. Add pool saturation to the checkout dashboard's first row. Shorten the \
     checkout alert window from five minutes to two. Update the runbook so the first step for \
     checkout timeouts is to check recent config changes, before investigating third parties.",
    "Apartment lease summary\n\n\
     The lease runs twelve months starting September 1, at $2,450 a month, due on the first \
     of each month. Rent paid after the fifth incurs a late fee of $75. Payment is by bank \
     transfer to the management company; they do not accept cash or personal checks.\n\n\
     The security deposit is one month's rent, held in an interest-bearing account. It is \
     returned within 30 days of move-out minus any deductions for damage beyond normal wear and \
     tear, with an itemized list. Photos taken at move-in are attached to the lease as the \
     baseline, so it is worth keeping our own copies.\n\n\
     Pets are allowed with a one-time pet fee of $400 and a limit of two animals under 50 pounds \
     each. Subletting is not allowed without written consent from the landlord, and short-term \
     rentals are prohibited outright. Minor repairs under $100 are our responsibility; anything \
     bigger goes through the maintenance request portal, and emergencies such as leaks or no heat \
     go to the 24-hour maintenance line.\n\n\
     To leave early we have to give 60 days written notice and pay a break fee equal to two \
     months rent, unless the landlord finds a replacement tenant sooner. To renew, the landlord \
     must send the renewal offer at least 90 days before the lease ends, and any rent increase \
     at renewal is capped at 5%. Parking is one assigned space in the garage, included in the \
     rent; a second space costs $150 a month if one is available.",
];

/// (search query, index into `LONG_DOCS` that must come back). Each query
/// targets a fact in a later paragraph, not the document's opening.
const LONG_QUERIES: &[(&str, usize)] = &[
    ("when is the postgres version upgrade happening", 0),
    ("who is joining the interview panel for the backend hire", 0),
    ("what time does our flight home leave", 1),
    ("which side of the train has the mountain view", 1),
    ("what happens when two edits to a work order conflict", 2),
    ("how much battery can background syncing use", 2),
    ("how big a credit can I give a customer without approval", 3),
    ("how fast do we have to respond to an urgent ticket", 3),
    ("why did the connection pool run out", 4),
    ("how long did it take before the alert fired", 4),
    ("what does it cost to end the lease early", 5),
    ("can I have a dog in the apartment", 5),
];

// ---------------------------------------------------------------------------
// Harness
// ---------------------------------------------------------------------------

struct Env {
    embedding: Arc<NodeEmbeddingService>,
    nodes: Arc<NodeService>,
    nlp: Arc<EmbeddingService>,
    _dir: TempDir,
}

async fn env() -> Option<Env> {
    let dir = TempDir::new().expect("tempdir");
    let mut store = Arc::new(
        SqliteStore::new(dir.path().join("test.db"))
            .await
            .expect("store must open"),
    );
    let nodes = Arc::new(NodeService::new(&mut store).await.expect("node service"));
    let mut nlp = EmbeddingService::new(EmbeddingConfig::default()).expect("config");
    if nlp.initialize().is_err() || !nlp.is_initialized() {
        eprintln!("SKIP live_embedding_prefix_measurement: embedding model not on disk");
        return None;
    }
    let nlp = Arc::new(nlp);
    let accessor: Arc<dyn NodeAccessor> = nodes.clone();
    let embedding = Arc::new(NodeEmbeddingService::new(
        nlp.clone(),
        store.clone(),
        accessor,
        nodes.behaviors().clone(),
    ));
    Some(Env {
        embedding,
        nodes,
        nlp,
        _dir: dir,
    })
}

async fn create_root(
    env: &Env,
    node_type: &str,
    content: &str,
    properties: serde_json::Value,
) -> String {
    let id = env
        .nodes
        .create_node_with_parent(CreateNodeParams {
            id: None,
            node_type: node_type.to_string(),
            content: content.to_string(),
            parent_id: None,
            position: InsertPositionOwned::End,
            properties,
            lifecycle_status: None,
        })
        .await
        .expect("node must insert");
    env.embedding
        .embed_root_node(&id)
        .await
        .expect("root must embed");
    id
}

fn pct(n: usize, d: usize) -> String {
    format!("{:.0}% ({n}/{d})", 100.0 * n as f64 / d as f64)
}

fn mean(xs: &[f64]) -> f64 {
    xs.iter().sum::<f64>() / xs.len().max(1) as f64
}

/// Which prefix the production query path embeds with.
fn query_prefix_in_use(env: &Env) -> &'static str {
    let probe = "which invoices are overdue";
    let production = env.embedding.embed_query_text(probe).expect("query embeds");
    let as_query = env.nlp.embed_query(probe).expect("embeds");
    let as_document = env.nlp.embed_document(probe).expect("embeds");
    assert_ne!(
        as_query, as_document,
        "the two prefixes must yield different vectors"
    );
    if production == as_query {
        "search_query"
    } else if production == as_document {
        "search_document"
    } else {
        panic!("production query vector matches neither prefix")
    }
}

// ---------------------------------------------------------------------------
// The measurement
// ---------------------------------------------------------------------------

#[tokio::test]
#[ignore = "requires the locked nomic-embed-text-v1.5 GGUF on disk"]
async fn measure_query_side_retrieval() {
    let Some(env) = env().await else {
        return;
    };
    println!(
        "\n=== query-side prefix in use: {} ===",
        query_prefix_in_use(&env)
    );

    measure_skills(&env).await;
    measure_schemas(&env).await;
    measure_node_search(&env).await;
}

async fn measure_skills(env: &Env) {
    for tmpl in seed_skill_nodes() {
        let prepared = prepare_nodes_from_template(&tmpl).expect("template parses");
        for p in &prepared {
            env.nodes
                .create_node_with_parent(CreateNodeParams {
                    id: Some(p.id.clone()),
                    node_type: p.node_type.clone(),
                    content: p.content.clone(),
                    parent_id: p.parent_id.clone(),
                    position: InsertPositionOwned::End,
                    properties: p.properties.clone(),
                    lifecycle_status: None,
                })
                .await
                .expect("skill node inserts");
        }
        env.embedding
            .embed_root_node(&prepared[0].id)
            .await
            .expect("skill root embeds");
    }

    println!(
        "\n--- skill retrieval (find_skills, {} requests) ---",
        SKILL_CASES.len()
    );
    let (mut rank1, mut top_k, mut clears_bar) = (0, 0, 0);
    let mut wrong_rank1_clearing_bar = 0;
    let (mut expected_scores, mut margins, mut all_scores) = (Vec::new(), Vec::new(), Vec::new());
    for (query, want) in SKILL_CASES {
        let output = find_skills(
            &env.embedding,
            &env.nodes,
            FindSkillsInput {
                query: query.to_string(),
                limit: Some(11),
            },
        )
        .await
        .expect("find_skills succeeds");
        let candidates: Vec<SkillCandidate> = output
            .skills
            .iter()
            .filter(|s| s.get("kind").and_then(|v| v.as_str()) != Some("schema"))
            .map(|s| SkillCandidate {
                id: String::new(),
                name: s["name"].as_str().unwrap_or_default().to_string(),
                description: String::new(),
                score: s["confidence"].as_f64().unwrap_or(0.0) as f32,
                tools: s["tools"]
                    .as_array()
                    .map(|a| {
                        a.iter()
                            .filter_map(|t| t.as_str().map(str::to_string))
                            .collect()
                    })
                    .unwrap_or_default(),
                instructions: String::new(),
                schema_metadata: serde_json::Value::Null,
                schemas_linked: false,
                pinned: false,
            })
            .collect();
        all_scores.extend(candidates.iter().map(|c| f64::from(c.score)));
        let pos = candidates.iter().position(|c| c.name == *want);
        let top = candidates.first().expect("find_skills returned no skills");
        if pos == Some(0) {
            rank1 += 1;
            if let Some(second) = candidates.get(1) {
                margins.push(f64::from(top.score - second.score));
            }
        } else if top.score >= score_bar_for(top) {
            wrong_rank1_clearing_bar += 1;
        }
        if pos.is_some_and(|p| p < RETRIEVAL_TOP_K) {
            top_k += 1;
        }
        if let Some(p) = pos {
            let c = &candidates[p];
            expected_scores.push(f64::from(c.score));
            if c.score >= score_bar_for(c) {
                clears_bar += 1;
            }
        }
        println!(
            "  {} {want:<24} rank {:<4} score {:.3} (bar {:.2}) | top {}={:.3} | {query}",
            if pos == Some(0) { "ok  " } else { "MISS" },
            pos.map_or("-".to_string(), |p| (p + 1).to_string()),
            pos.map_or(0.0, |p| candidates[p].score),
            pos.map_or(0.0, |p| score_bar_for(&candidates[p])),
            top.name,
            top.score,
        );
    }
    let n = SKILL_CASES.len();
    all_scores.sort_by(f64::total_cmp);
    println!(
        "  rank-1 {} | top-{RETRIEVAL_TOP_K} {} | expected skill clears its bar {} | wrong rank-1 clearing its bar {}",
        pct(rank1, n),
        pct(top_k, n),
        pct(clears_bar, n),
        pct(wrong_rank1_clearing_bar, n),
    );
    println!(
        "  expected-skill score mean {:.3} | rank-1 margin mean {:.3} | all skill scores min {:.3} median {:.3} max {:.3}",
        mean(&expected_scores),
        mean(&margins),
        all_scores.first().expect("no skill scores"),
        all_scores[all_scores.len() / 2],
        all_scores[all_scores.len() - 1],
    );
}

async fn measure_schemas(env: &Env) {
    let mut ids = HashMap::new();
    for (key, name, description) in SCHEMAS {
        let created = handle_create_schema(
            &env.nodes,
            json!({
                "name": name,
                "description": description,
                "fields": [{ "name": "added_on", "type": "date" }]
            }),
        )
        .await
        .expect("schema creates");
        let id = created["schemaId"].as_str().expect("schemaId").to_string();
        env.embedding
            .embed_root_node(&id)
            .await
            .expect("schema embeds");
        ids.insert(id, *key);
    }

    println!(
        "\n--- schema retrieval (typed KNN, top-{MAX_SEMANTIC_SCHEMAS}, {} cases) ---",
        SCHEMA_CASES.len()
    );
    let thresholds = [0.1, 0.2, 0.3, 0.4, 0.5, 0.6];
    let mut hits: HashMap<(&str, usize), usize> = HashMap::new();
    let mut totals: HashMap<&str, usize> = HashMap::new();
    let (mut target_scores, mut best_other_scores, mut rank1) = (Vec::new(), Vec::new(), 0);
    let mut other_scores = Vec::new();
    for (prior, latest, want, label) in SCHEMA_CASES {
        let query = build_retrieval_query(prior, latest);
        let ranked = env
            .embedding
            .semantic_search_nodes_of_type(&query, "schema", SCHEMAS.len(), 0.0)
            .await
            .expect("schema search succeeds");
        let ranked: Vec<(&str, f64)> = ranked.iter().map(|(n, s)| (ids[&n.id], *s)).collect();
        let pos = ranked.iter().position(|(k, _)| k == want);
        let target = pos.map_or(0.0, |p| ranked[p].1);
        let best_other = ranked.iter().find(|(k, _)| k != want).map_or(0.0, |r| r.1);
        other_scores.extend(ranked.iter().filter(|(k, _)| k != want).map(|r| r.1));
        target_scores.push(target);
        best_other_scores.push(best_other);
        if pos == Some(0) {
            rank1 += 1;
        }
        for label in [*label, "all"] {
            *totals.entry(label).or_default() += 1;
            for (i, t) in thresholds.iter().enumerate() {
                if pos.is_some_and(|p| p < MAX_SEMANTIC_SCHEMAS) && target > *t {
                    *hits.entry((label, i)).or_default() += 1;
                }
            }
        }
        let kept = ranked
            .iter()
            .filter(|(_, s)| *s > SCHEMA_THRESHOLD)
            .take(MAX_SEMANTIC_SCHEMAS)
            .count();
        println!(
            "  {} {want:<16} rank {:<3} score {target:.3} | best other {best_other:.3} | kept@0.2 {kept} | [{label}] {latest}",
            if pos.is_some_and(|p| p < MAX_SEMANTIC_SCHEMAS) && target > SCHEMA_THRESHOLD { "ok  " } else { "MISS" },
            pos.map_or("-".to_string(), |p| (p + 1).to_string()),
        );
    }
    for label in ["single", "follow-up", "switch", "all"] {
        let row: Vec<String> = thresholds
            .iter()
            .enumerate()
            .map(|(i, t)| {
                format!(
                    "@{t}: {}",
                    pct(*hits.get(&(label, i)).unwrap_or(&0), totals[label])
                )
            })
            .collect();
        println!(
            "  recall@{MAX_SEMANTIC_SCHEMAS} {label:<9} {}",
            row.join("  ")
        );
    }
    other_scores.sort_by(f64::total_cmp);
    println!(
        "  rank-1 {} | target score mean {:.3} min {:.3} | best non-target mean {:.3} max {:.3} | any non-target min {:.3} median {:.3}",
        pct(rank1, SCHEMA_CASES.len()),
        mean(&target_scores),
        target_scores.iter().copied().fold(f64::INFINITY, f64::min),
        mean(&best_other_scores),
        best_other_scores.iter().copied().fold(0.0, f64::max),
        other_scores.first().expect("no off-target scores"),
        other_scores[other_scores.len() / 2],
    );
}

async fn measure_node_search(env: &Env) {
    // One corpus: the short notes, then the long documents, each long
    // document its own topic. Every query runs against all of it.
    let mut topics: Vec<String> = NOTES.iter().map(|(t, _)| t.to_string()).collect();
    let mut ids = Vec::new();
    for (_, note) in NOTES {
        ids.push(create_root(env, "text", note, json!({})).await);
    }
    for (i, doc) in LONG_DOCS.iter().enumerate() {
        topics.push(format!("long-{i}"));
        ids.push(create_root(env, "text", doc, json!({})).await);
    }

    let long_queries: Vec<(&str, usize)> = LONG_QUERIES
        .iter()
        .map(|(q, i)| (*q, NOTES.len() + i))
        .collect();
    report_node_search(env, "short notes", NODE_QUERIES, &ids, &topics).await;
    report_node_search(env, "long documents", &long_queries, &ids, &topics).await;

    println!("\n--- context-assembly neighbours (seed content as query, threshold {SEMANTIC_THRESHOLD}) ---");
    let (mut same_topic, mut returned, mut possible) = (0, 0, 0);
    for (seed_idx, (topic, note)) in NOTES.iter().enumerate() {
        let results = env
            .embedding
            .semantic_search_nodes(
                note,
                NEIGHBORS_PER_SEED + 1,
                SEMANTIC_THRESHOLD,
                None,
                false,
            )
            .await
            .expect("neighbour search succeeds");
        let neighbours: Vec<usize> = results
            .iter()
            .filter_map(|(n, _)| ids.iter().position(|i| *i == n.id))
            .filter(|i| *i != seed_idx)
            .take(NEIGHBORS_PER_SEED)
            .collect();
        returned += neighbours.len();
        same_topic += neighbours.iter().filter(|i| topics[**i] == *topic).count();
        possible += NOTES.iter().filter(|(t, _)| t == topic).count() - 1;
    }
    println!(
        "  same-topic recall {} | precision {}",
        pct(same_topic, possible),
        pct(same_topic, returned),
    );
}

/// Run `queries` through `semantic_search_nodes` and report rank-1, recall@5
/// per threshold, and the target versus off-topic score distribution.
async fn report_node_search(
    env: &Env,
    label: &str,
    queries: &[(&str, usize)],
    ids: &[String],
    topics: &[String],
) {
    println!(
        "\n--- node search: {label} (semantic_search_nodes, {} queries) ---",
        queries.len()
    );
    let thresholds = [0.2, 0.3, 0.4, 0.5, 0.6];
    let mut recall = vec![0; thresholds.len()];
    let (mut rank1, mut target_scores, mut other_scores) = (0, Vec::new(), Vec::new());
    for (query, want) in queries {
        let ranked = env
            .embedding
            .semantic_search_nodes(query, ids.len(), 0.0, None, false)
            .await
            .expect("node search succeeds");
        let ranked: Vec<(usize, f64)> = ranked
            .iter()
            .filter_map(|(n, s)| ids.iter().position(|i| *i == n.id).map(|i| (i, *s)))
            .collect();
        let pos = ranked.iter().position(|(i, _)| i == want);
        let target = pos.map_or(0.0, |p| ranked[p].1);
        target_scores.push(target);
        other_scores.extend(
            ranked
                .iter()
                .filter(|(i, _)| topics[*i] != topics[*want])
                .map(|r| r.1),
        );
        if pos == Some(0) {
            rank1 += 1;
        }
        for (i, t) in thresholds.iter().enumerate() {
            if pos.is_some_and(|p| p < 5) && target >= *t {
                recall[i] += 1;
            }
        }
        println!(
            "  {} rank {:<3} score {target:.3} | top {:.3} | {query}",
            if pos == Some(0) { "ok  " } else { "MISS" },
            pos.map_or("-".to_string(), |p| (p + 1).to_string()),
            ranked.first().map_or(0.0, |r| r.1),
        );
    }
    let n = queries.len();
    let row: Vec<String> = thresholds
        .iter()
        .enumerate()
        .map(|(i, t)| format!("@{t}: {}", pct(recall[i], n)))
        .collect();
    other_scores.sort_by(f64::total_cmp);
    println!("  rank-1 {} | recall@5 {}", pct(rank1, n), row.join("  "));
    println!(
        "  target score mean {:.3} min {:.3} | off-topic score min {:.3} median {:.3} p90 {:.3} max {:.3}",
        mean(&target_scores),
        target_scores.iter().copied().fold(f64::INFINITY, f64::min),
        other_scores.first().expect("no off-target scores"),
        other_scores[other_scores.len() / 2],
        other_scores[other_scores.len() * 9 / 10],
        other_scores[other_scores.len() - 1],
    );
}
