//! Two-stage skill-mediated routing (ADR-038).
//!
//! The model reaches capabilities through skills discovered by semantic
//! retrieval, with the LLM judging at two gates and an explicit clarification
//! fallback. Retrieval itself is a deterministic system step the model does
//! not perform, which is where the candidate count is bounded and the trust
//! filter applies.
//!
//! ```text
//! Stage 1 (model):  emit EITHER a search query OR a clarification request
//!                   — a structural choice, not a confidence number
//! [system step]:    semantic retrieval over the skill registry → top-K,
//!                   score-gated. NOT a model tool call.
//! Stage 2 (model):  judge the candidates against the intent → pick one and
//!                   act, or clarify with the candidates as concrete options
//! ```
//!
//! ADR-038 rejects the single-turn pull (handing the model a `search_skills`
//! tool and letting it call retrieval itself), because that removes the
//! system's ability to bound K and enforce the trust boundary.

use crate::agent_types::{SkillCandidate, ToolCallRaw, ToolDefinition};
use nodespace_core::models::SkillRole;
use nodespace_core::ops::context_ops::EXISTING_SCHEMAS_HEADER;
use serde::Deserialize;

use super::tools::Tool;

/// How many skill candidates retrieval may return.
///
/// This is the bound ADR-038 requires the system — not the model — to own.
/// Three is enough to offer a real choice at Stage 2 (and to name concrete
/// options in a clarification) while keeping the injected instruction payload
/// small, since each candidate carries its full instruction subtree.
pub const RETRIEVAL_TOP_K: usize = 3;

/// How many candidates retrieval is asked for: one more than Stage 2 judges by
/// default, so [`select_candidates`] has a write skill to restore when a
/// read-only one took its place.
pub const RETRIEVAL_FETCH: usize = RETRIEVAL_TOP_K + 1;

/// The candidates Stage 2 judges, chosen from retrieval's ranking.
///
/// The first [`RETRIEVAL_TOP_K`], plus the next one when a read-only skill
/// that does not lead the turn sits among them and the next one can write.
///
/// A read-only skill's vocabulary is the broadest in the registry — every
/// request is about something stored — so it places on turns it does not
/// lead, and each place it takes is one a write skill loses. That costs more
/// than a rank: a write tool is reachable from only one or two skills, so the
/// skill that falls to fourth takes the turn's write tool with it. Measured on
/// the locked embedding model, "keep track of decisions behind each feature"
/// ranked Graph Editing, Research & Search, Organization, Schema Creation: the
/// lookup placed second and `create_schema` left the surface.
///
/// The bound this relaxes is the one [`RETRIEVAL_TOP_K`] exists for, the
/// number of things a turn can be led to *do*. A read-only candidate adds
/// nothing to that, so it is not counted against the skills that do. The
/// payload bound moves by at most one candidate: never more than
/// [`RETRIEVAL_FETCH`] are judged.
///
/// A lookup that leads keeps its place and takes no extra: the turn is a
/// lookup, and the write skills behind it are the ordinary runners-up. A
/// second read-only skill placing behind it still counts as one that took a
/// write skill's place.
///
/// The procedure lane (ADR-038) adds the best procedure after those, outside
/// the bound: it takes no place from a tool skill.
///
/// `ranked` must be sorted by score, descending, as `agent_loop`'s `route`
/// leaves it.
pub fn select_candidates(ranked: Vec<SkillCandidate>) -> Vec<SkillCandidate> {
    let (procedures, ranked): (Vec<_>, Vec<_>) = ranked
        .into_iter()
        .partition(|c| c.role == SkillRole::Procedure);
    let mut chosen = select_tool_candidates(ranked);
    chosen.extend(procedures.into_iter().take(1));
    chosen
}

/// [`select_candidates`] over the tool lane alone.
fn select_tool_candidates(mut ranked: Vec<SkillCandidate>) -> Vec<SkillCandidate> {
    let leader_id = leading_tool_bearing_candidate(&ranked[..ranked.len().min(RETRIEVAL_TOP_K)])
        .map(|c| c.id.clone());
    let lookup_took_a_place = ranked.iter().take(RETRIEVAL_TOP_K).any(|c| {
        is_tool_bearing_contender(c) && !skill_is_mutating(c) && Some(&c.id) != leader_id.as_ref()
    });
    // The same eligibility the first three are held to: a fourth candidate
    // below its own bar would be kept and then never rendered or offered.
    let next_can_write = ranked
        .get(RETRIEVAL_TOP_K)
        .is_some_and(|c| is_tool_bearing_contender(c) && skill_is_mutating(c));
    let keep = if lookup_took_a_place && next_can_write {
        RETRIEVAL_FETCH
    } else {
        RETRIEVAL_TOP_K
    };
    ranked.truncate(keep);
    ranked
}

/// Minimum retrieval score for a **read-only** skill to be actionable.
///
/// The mechanical half of the Stage-2 gate. The model's yes/no on the
/// candidate is the other, independently-sourced half; both must pass.
///
/// **Deliberately permissive, and not yet tuned.** Tuning requires an eval
/// that can distinguish a model failure from a harness failure, which the
/// agent matrix currently cannot do. Until that lands, a stricter value would
/// be tightening against an untested target: it would silently hide the long
/// tail, and no measurement would show the loss. Raise this only alongside a
/// measurement that demonstrates the tail being hidden is genuinely noise.
///
/// Note this is a different constant from `skill_ops::SKILL_SEARCH_THRESHOLD`,
/// which stays at `0.0` on purpose: retrieval still *returns* the long tail so
/// the judged gate can see it, and this bar decides what is *actionable*.
/// One gate filters, the other judges.
pub const READ_SKILL_SCORE_BAR: f32 = 0.15;

/// Minimum retrieval score for a **mutating** skill to be actionable.
///
/// ADR-038: "the Stage-2 bar is a property of the matched skill, not a single
/// global constant. Mutating skills warrant a higher bar than read-only ones,
/// because the expensive error — firing the wrong *mutating* tool — changes
/// the graph." Bias the gate against that error.
///
/// **Deliberately permissive, and not yet tuned** — see
/// [`READ_SKILL_SCORE_BAR`]. The ordering (mutating strictly above read-only)
/// is the load-bearing property here; the absolute values are placeholders
/// awaiting a diagnostic eval.
pub const MUTATING_SKILL_SCORE_BAR: f32 = 0.30;

/// Minimum retrieval score for a skill that can **irreversibly remove user
/// data** to be actionable.
///
/// The top rung of the blast-radius ladder ADR-038 describes: read-only <
/// mutating < destructive. The ADR states the bar "is a property of the
/// matched skill, not a single global constant" and that the gates should be
/// biased against the expensive error; deleting the node a user meant to
/// update is the one error in this system that cannot be walked back, so it
/// sits above the general mutating bar rather than sharing it.
///
/// **This value is an untuned placeholder, and knowingly so.**
/// [`READ_SKILL_SCORE_BAR`] and [`MUTATING_SKILL_SCORE_BAR`] carry the same
/// caveat, for the same reason: tuning needs an eval that can separate a model
/// failure from a harness failure. This rung was added to stop a weak
/// destructive match winning rank-1 on requests with no deletion intent —
/// `Node Deletion` was measured as a retrieved candidate on 11 turns across 5
/// scenarios whose prompts marked a status, recorded a decision, set a due
/// date, and asked a question, ranking first on three of them. It was chosen
/// structurally (clearly above the mutating bar, well below the score a
/// genuine "delete X" match earns), not measured against live embeddings.
///
/// The *ordering* is the load-bearing property and is asserted in tests. If a
/// real deletion request is ever seen failing to route, this constant is the
/// first thing to revisit.
pub const DESTRUCTIVE_SKILL_SCORE_BAR: f32 = 0.45;

/// Stage-1's structural choice, recovered from which tool the model called.
///
/// ADR-038 rejects gating on a self-reported confidence number: a numeric
/// self-rating from a small model is not calibrated and invites anchoring.
/// The choice is therefore *which typed tool* the model calls, which
/// is also the channel measured strongest for structured output — a tool
/// schema, rather than prose the model must be trusted to follow.
#[derive(Debug, Clone, PartialEq)]
pub enum RouteDecision {
    /// The model formed a search query; run retrieval on it.
    Query(String),
    /// The model could not form a query and asked to clarify.
    Clarify {
        /// The specific question to put to the user.
        question: String,
        /// Concrete options to offer. ADR-038: a bare "what do you mean?" is
        /// the failure mode to avoid.
        options: Vec<String>,
    },
    /// The user asked about something they may have stored, or asked for it
    /// to be found. Carries the topic to look up, in their words.
    Lookup(String),
    /// The request bundles multiple distinct, unambiguous intents —
    /// one query per intent, each re-entering retrieval independently the
    /// same way a single `Query` does. Not for a single intent expressed
    /// verbosely, and not a substitute for `Clarify` when the request is
    /// genuinely ambiguous rather than compound.
    Multi(Vec<String>),
}

/// Wire name of the Stage-1 tool that emits a search query.
pub const ROUTE_QUERY_TOOL: &str = "route_query";
/// Wire name of the Stage-1 tool that requests clarification.
pub const ROUTE_CLARIFY_TOOL: &str = "route_clarify";
/// Wire name of the Stage-1 tool that emits multiple per-intent queries.
pub const ROUTE_MULTI_TOOL: &str = "route_multi";
/// Wire name of the Stage-1 tool that names a topic to look up.
pub const ROUTE_LOOKUP_TOOL: &str = "route_lookup";

/// Whether `message` has the shape of a lookup: it ends with a question
/// mark, or opens with a question word or a retrieval verb.
///
/// This does not decide a route. It decides three things around one:
///
/// - whether Stage 1 is asked about the message alone before it is asked
///   about the message in context. The model makes the call either way, and a
///   message misread here costs one short generation, or is routed exactly as
///   it was before this existed;
/// - whether Stage 2 is shown the list of the agent's skills
///   ([`render_skill_names_for_prompt`]), which only a question or a find
///   request can be about. A message misread here gets the list it had no use
///   for, or asks about skills without it and is answered from the routed
///   procedures alone;
/// - whether a prose reply is put back to act, in an intent that composed no
///   clarification (see the re-prompt in `agent_loop`). Here a misread changes
///   the reply: a request that only ends in a question mark, answered with a
///   prose question, is not put back. It was measured against the case it was
///   added for, a question answered correctly and then re-prompted into a
///   search, and that trade is recorded in ADR-038.
///
/// English only, and deliberately loose: "could you add a task?" and "find
/// and delete the old notes" both have the shape and are not lookups, which
/// the first pass says.
pub fn is_lookup_shaped(message: &str) -> bool {
    const OPENERS: [&str; 16] = [
        // question words
        "how", "what", "why", "when", "where", "who", "which", // retrieval verbs
        "find", "search", "look", "locate", "list", "show", "explain", "describe", "tell",
    ];
    let message = message.trim();
    // The full-width form is what a CJK keyboard types.
    if message.ends_with(['?', '？']) {
        return true;
    }
    let first_word: String = message
        .chars()
        .take_while(|c| c.is_alphabetic())
        .flat_map(char::to_lowercase)
        .collect();
    OPENERS.contains(&first_word.as_str())
}

/// Whether Stage 1 is asked about the message by itself before it is asked
/// about the message blended with the turns ahead of it.
///
/// Three conditions, all structural:
///
/// - there are turns ahead of it (`blended_query` differs from the message),
///   or the two passes would be the same question asked twice;
/// - the message is shaped like a lookup;
/// - it is not the answer to a clarifying question. That message has the
///   least routing signal of any by itself — "the client ones?" — and it is
///   the one turn that must be read in the light of what was asked.
///
/// The loop and the live Stage-1 tests both call this, so what the tests
/// measure is the gate the loop runs.
pub fn asks_about_the_message_first(
    blended_query: &str,
    message: &str,
    answering_a_clarification: bool,
) -> bool {
    !answering_a_clarification && blended_query != message && is_lookup_shaped(message)
}

/// Whether `message` asks about what the workspace holds: a question that
/// opens with a question word and is not about the assistant.
///
/// "How do we onboard a new reviewer?" is one. Four things are not, each for
/// a reason:
///
/// - a message that does not end in a question mark. "When the review is
///   done, mark it signed off" and "Which reminds me, add a note about the
///   offsite" open with the same words and ask for a write. Three openings
///   are read as a question without the mark, provided no word in the message
///   names a change: "tell me" / "let me know" before the question word,
///   "how many" / "how much", and a polite request to read ("could you check
///   the status of …", "please list …"). Left to Stage 1, those questions
///   were routed as queries and carried the write skills and tools, which was
///   about three times the prompt for the same answer;
/// - a suggestion: "How about a task for that?", "What about adding Harbour
///   as a planning cycle?", "What if we moved it?", "Why not close it?",
///   "Why don't we close it?";
/// - a question about the assistant, whether it says "you" ("Which skills do
///   you have?") or names what the assistant has ("What skills are
///   available?"). Nothing the user stored answers it, and the routing tools
///   send it elsewhere;
/// - a request that only ends in a question mark ("Could you add a task?"),
///   which opens with no question word.
///
/// English only, and narrower than [`is_lookup_shaped`] on purpose, because
/// this one does decide a route (see [`decision_on_the_message_alone`]). A
/// message it leaves out is routed as Stage 1 routed it.
///
/// Each test is of the words, not of what they mean, so it leaves out
/// questions that are about the workspace: one typed without its question
/// mark ("how do we onboard a new reviewer"), and one whose subject is a
/// skill, a tool or an assistant the user keeps records of ("Which tool do we
/// use for deploys?").
///
/// What it lets through that is not about the workspace is a general
/// question ("What's the weather like in Tokyo today?"), where a query from
/// Stage 1 becomes a lookup: the turn searches, finds nothing, and says so,
/// which is the outcome ADR-038 gives a request nothing stored can answer.
pub fn asks_what_the_workspace_holds(message: &str) -> bool {
    const QUESTION_WORDS: [&str; 7] = ["how", "what", "why", "when", "where", "who", "which"];
    // "How about …?", "What if …?", "Why not …?", "Why don't we …?": the
    // second word of a suggestion. "don" is what "don't" splits to.
    const SUGGESTING: [&str; 4] = ["about", "if", "not", "don"];
    const THE_ASSISTANT: [&str; 9] = [
        "you",
        "your",
        "yours",
        "yourself",
        "skill",
        "skills",
        "tool",
        "tools",
        "assistant",
    ];
    // The full-width form is what a CJK keyboard types.
    let marked = message.trim_end().ends_with(['?', '？']);
    let lowered = message.to_lowercase();
    let words: Vec<&str> = lowered
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
        .collect();
    let (opening, rest) = split_request_opening(&words);
    let Some((&first, after)) = rest.split_first() else {
        return false;
    };
    if THE_ASSISTANT.iter().any(|w| rest.contains(w)) {
        return false;
    }
    if QUESTION_WORDS.contains(&first) {
        if after
            .first()
            .is_some_and(|second| SUGGESTING.contains(second))
        {
            return false;
        }
        // Without a question mark a question word opens a clause as readily as
        // a question ("When the review is done, mark it signed off"). Three
        // things say it is a question anyway: "tell me" or "let me know" in
        // front of it, "how many" / "how much", which open no clause that
        // asks for a write, and no word in the message that asks for one.
        let counts = first == "how" && after.first().is_some_and(|w| ["many", "much"].contains(w));
        return marked || (!asks_for_a_write(&words) && (opening.asked || counts));
    }
    // A polite request to read, with no question mark and no question word:
    // "Could you check the status of tasks assigned to Anoop". Only when it
    // opens with a request and names a verb that reads, and nothing in it asks
    // for a write ("could you list the tasks and delete the old ones").
    opening.polite
        && !asks_for_a_write(&words)
        && match first {
            "find" | "list" | "show" | "search" | "look" => true,
            "check" => matches!(
                after,
                ["the", "status", ..]
                    | ["on", ..]
                    | ["how", ..]
                    | ["if", ..]
                    | ["whether", ..]
                    | ["what", ..]
            ),
            _ => false,
        }
}

/// How a message opens before the words it asks with.
struct RequestOpening {
    /// "tell me" or "let me know": a question is being put.
    asked: bool,
    /// "could you", "can you", "would you", "will you" or "please".
    polite: bool,
}

/// Split the openings that ask for something off the front of `words`.
fn split_request_opening<'a>(words: &'a [&'a str]) -> (RequestOpening, &'a [&'a str]) {
    let mut opening = RequestOpening {
        asked: false,
        polite: false,
    };
    let mut rest = words;
    loop {
        rest = match rest {
            ["could" | "can" | "would" | "will", "you", tail @ ..] => {
                opening.polite = true;
                tail
            }
            ["please", tail @ ..] => {
                opening.polite = true;
                tail
            }
            ["tell", "me", tail @ ..] => {
                opening.asked = true;
                tail
            }
            ["let", "me", "know", tail @ ..] => {
                opening.asked = true;
                tail
            }
            _ => return (opening, rest),
        };
    }
}

/// Whether any word of the message names a change to the graph. Used only
/// where the message has no question mark to say it is a question.
fn asks_for_a_write(words: &[&str]) -> bool {
    const WRITING: [&str; 17] = [
        "add", "create", "make", "update", "change", "set", "mark", "assign", "move", "delete",
        "remove", "archive", "rename", "close", "reopen", "link", "unassign",
    ];
    words.iter().any(|w| WRITING.contains(w))
}

/// The decision for a message Stage 1 was asked about by itself, with no
/// turns ahead of it blended in.
///
/// A question about how something is done names an action, and Stage 1 reads
/// the action. Measured on the locked model, each three times in three: "How
/// do we onboard a new reviewer?" came back as `route_query("onboard a new
/// reviewer")`, and so did the same question about a sponsor, about
/// offboarding a contractor, and about deciding which cycle a slipped spec
/// moves into. Retrieval then matched the skills that write, and the reply
/// was that nothing could be done.
///
/// No wording of the routing tools held. Telling Stage 1 in its prompt that
/// asking how is looking up sent questions about the assistant to a lookup
/// and stopped a compound request being split. Adding "how something is done"
/// to `route_lookup`'s description fixed all four and turned "find the record
/// for the Lisbon offsite" into a query; naming a record on that side put it
/// back and stopped the compound split again.
///
/// So the question is read off the message, as its shape already is
/// ([`asks_about_the_message_first`]): a query for a message that
/// [`asks_what_the_workspace_holds`] is a lookup, of the message itself. Every
/// other decision stands, a clarification and a split included.
pub fn decision_on_the_message_alone(
    message: &str,
    decision: Option<RouteDecision>,
) -> Option<RouteDecision> {
    match decision {
        Some(RouteDecision::Query(_)) if asks_what_the_workspace_holds(message) => {
            let topic = message.trim().trim_end_matches(['?', '？']).trim_end();
            Some(RouteDecision::Lookup(topic.to_string()))
        }
        other => other,
    }
}

/// The retrieval query for a lookup: the capability, then the topic.
///
/// A topic alone embeds nearest whichever skill shares a noun with it ("the
/// debounce logic" retrieved Node Deletion and Node Merge). Naming the
/// capability ahead of it is what puts the search skill first, and the
/// wording is the one that skill's description carries.
///
/// It carries the description's opening verbs as well as "search stored
/// knowledge". With that phrase alone, a topic that is a record's name and one
/// of its fields led with the search skill by 0.009 ("Kestrel Gateway sign off
/// date") or lost to Node Deletion by 0.001 ("Kestrel Sync sign off date"),
/// and a lookup that leads with the deletion skill is offered `delete_node`.
/// No wording of the search skill's description closed that without lifting
/// it on requests that are not lookups: naming a date or an owner there put it
/// ahead of Schema Creation on a request to start tracking something. The
/// prefix is on lookups only, and with the verbs those two lead by 0.032 and
/// 0.024; over the lookups measured beside them the smallest lead went from
/// -0.001 to 0.024.
pub fn lookup_retrieval_query(topic: &str) -> String {
    format!("find, look up, or search stored knowledge for {topic}")
}

/// Whether `query` asks for something to be added: it opens with a verb that
/// brings something into being and never asks for a removal.
///
/// An embedding weighs a request's nouns far above its verb, so an add whose
/// record is named after another skill's subject is retrieved as that skill's
/// request. Measured on the locked embedding model, "add cache invalidation
/// decision to architecture decisions" ranked Node Deletion, Play Workflow
/// State, Node Merge and Bulk Import first, and Node Creation sixth:
/// `create_node` was off the surface and `delete_node` was on it. No wording
/// of either description separated the two, the way none separated "remove
/// the resolved tickets" from "mark it resolved".
///
/// The verb is read here instead, off the query Stage 1 wrote, whose tool
/// description asks for the action to be kept. It decides no route. It
/// decides two things about the candidates Stage 2 judges, both in
/// [`retrieve_candidates`]: no skill that removes user data is among them,
/// and one that can create a record is.
///
/// What is added need not be a record: "add a priority field to the ticket
/// type" and "add this note to my reading list" open with one of these verbs
/// too. Neither asks for a removal, so leaving the removing skills out costs
/// them nothing. The second search can cost one a candidate: when it runs, the
/// skill it adds leads, and the one that was third is no longer judged. On
/// the requests of this kind that were measured it does not run, and each
/// keeps its leader.
///
/// English only, and narrow on purpose: "put", "make" and "set" also start
/// requests to change a record, so they are left out, and a request worded
/// with one is routed as it was before this existed.
pub fn is_add_shaped(query: &str) -> bool {
    const ADDING_VERBS: [&str; 9] = [
        "add", "create", "log", "record", "file", "insert", "register", "capture", "jot",
    ];
    let first_word: String = query
        .trim()
        .chars()
        .take_while(|c| c.is_alphabetic())
        .flat_map(char::to_lowercase)
        .collect();
    ADDING_VERBS.contains(&first_word.as_str())
}

/// How many candidates retrieval is asked for on a request to add something.
///
/// Twice [`RETRIEVAL_FETCH`], so that the ranking is still a full one after
/// [`without_destructive_skills`] takes the deletion and merge skills out of
/// it. Asked for [`RETRIEVAL_FETCH`], "add the merge queue decision to our
/// architecture decisions" was left with two skills to judge.
pub const ADD_RETRIEVAL_FETCH: usize = 2 * RETRIEVAL_FETCH;

/// The retrieval query for an add that found no skill able to create a
/// record: the capability, then the request.
///
/// The same shape as [`lookup_retrieval_query`], for the same reason. The
/// wording is the one the creating skill's description carries.
pub fn create_retrieval_query(query: &str) -> String {
    format!("create a new record: {query}")
}

/// Whether a skill can create a record.
pub fn skill_can_create_a_record(candidate: &SkillCandidate) -> bool {
    candidate
        .tools
        .iter()
        .any(|t| Tool::from_name(t) == Some(Tool::CreateNode))
}

/// `ranked` without the skills that can remove user data: the candidates of a
/// request to add something ([`is_add_shaped`]).
///
/// ADR-038 offers a destructive tool only from the skill that won retrieval,
/// and on an add that winner can be a deletion skill matched on a noun. A
/// request that opens with an adding verb is not one to remove anything, so
/// such a skill is not a candidate on it at all: it cannot lead the turn, and
/// the place it held goes to the next skill.
///
/// The skill is left out whole, with the tools it holds that remove nothing.
/// A skill that can both create and remove is therefore not offered on an
/// add, including one a chat pins: a pinned skill that can remove user data
/// needs a retrieval score to clear its bar, and it has none from this query.
pub fn without_destructive_skills(mut ranked: Vec<SkillCandidate>) -> Vec<SkillCandidate> {
    ranked.retain(|c| !skill_is_destructive(c));
    ranked
}

/// Whether none of the candidates Stage 2 would judge can create a record.
///
/// On an add this is the turn that cannot do what was asked: `create_node`
/// is reachable from two skills, and both missed the bound.
///
/// A procedure does not count: it never leads a turn, so the skill that can
/// create a record has to be one that can.
pub fn lacks_a_creating_skill(ranked: &[SkillCandidate]) -> bool {
    !select_candidates(ranked.to_vec())
        .iter()
        .any(can_lead_a_creation)
}

/// Whether a candidate is a tool skill that clears its bar and can create a
/// record.
fn can_lead_a_creation(candidate: &SkillCandidate) -> bool {
    candidate.role != SkillRole::Procedure
        && clears_score_gate(candidate)
        && skill_can_create_a_record(candidate)
}

/// Whether a routing query asks to start keeping a new kind of record: one
/// that opens by setting up or starting to track something, and names a kind
/// of thing and not one record.
///
/// The same defect [`is_add_shaped`] closes for adds, on the other side. The
/// kind's name is all an embedding has to go on, and when it is another
/// skill's subject the request is retrieved as that skill's. Measured on the
/// locked embedding model, "set up Deletion requests with a requester and a
/// due date" ranked Node Deletion first (0.920) and the skill that holds
/// `create_schema` second (0.853); "keep track of merge requests with a
/// reviewer and a status" ranked Node Merge, Graph Editing and Conflict
/// Journal ahead of it, so `create_schema` was not offered at all. About
/// twenty wordings of the descriptions involved were measured and none
/// separated them without a cost on requests about one record.
///
/// So the shape is read off the query Stage 1 wrote. It decides no route. It
/// decides one thing about the candidates Stage 2 judges, in
/// [`retrieve_candidates`]: a skill that can define a type leads them.
///
/// Two openings, each with its own test for a kind of thing:
///
/// - **A tracking phrase** ("keep track of", "start tracking", "begin
///   tracking", "start keeping track of", "track"), followed by a word that
///   can open the name of a kind. That leaves out every word that opens a
///   mention of one thing or of something that is not a record at all
///   ([`OPENS_NO_KIND`]), and a name in the possessive. "Start tracking the
///   login timeout bug", "track an expense", "keep track of my dentist
///   appointment", "track Priya's onboarding", "keep track of when the cycle
///   ends" and "track down the notes on auth" are not this shape.
/// - **"Set up"**, followed by a word that passes the same test, and then
///   the details each one carries, introduced by "with a" or "with an": "set
///   up Postmortems with a severity and a review date". "Set up" also opens
///   requests for one record ("set up a meeting with Priya") and for a view
///   ("set up a board of tickets by status"), which is why it needs both.
///
/// English only, and narrow on purpose. What it misses is routed as it was
/// before this existed: "keep track of the books I lend out" and "start
/// tracking our planning cycles" name a kind behind a word that usually
/// opens one thing. What it takes wrongly is one thing worded with a bare
/// noun: "set up staging with a seed script", "track progress on the Q4
/// cycle".
pub fn is_new_kind_shaped(query: &str) -> bool {
    const TRACKING_PHRASES: [&str; 5] = [
        "start keeping track of ",
        "keep track of ",
        "start tracking ",
        "begin tracking ",
        "track ",
    ];
    let lowered = query.trim().to_lowercase();
    if let Some(rest) = TRACKING_PHRASES
        .iter()
        .find_map(|phrase| lowered.strip_prefix(phrase))
    {
        return opens_a_kind(rest);
    }
    if let Some(rest) = lowered.strip_prefix("set up ") {
        return opens_a_kind(rest) && (rest.contains(" with a ") || rest.contains(" with an "));
    }
    false
}

/// Words that, right after a tracking phrase or "set up", do not open the
/// name of a kind of record: a determiner, pronoun or possessive that points
/// at one thing, a quantifier that counts single ones, a question word that
/// opens a clause, and "down" ("track down" is a lookup).
const OPENS_NO_KIND: [&str; 26] = [
    "a", "an", "the", "this", "that", "these", "those", "it", "them", "my", "our", "your", "his",
    "her", "their", "its", "one", "each", "every", "when", "whether", "what", "who", "how", "why",
    "down",
];

/// Whether `rest`, lowercased, opens with a word that can start the name of a
/// kind: not one of [`OPENS_NO_KIND`], and not a name in the possessive
/// ("priya's onboarding").
fn opens_a_kind(rest: &str) -> bool {
    let word: String = rest.chars().take_while(|c| !c.is_whitespace()).collect();
    let possessive = word.ends_with("'s") || word.ends_with("\u{2019}s");
    let word = word.trim_matches(|c: char| !c.is_alphanumeric());
    !word.is_empty() && !possessive && !OPENS_NO_KIND.contains(&word)
}

/// The retrieval query for a new kind of record that did not lead with a
/// skill able to define one: the capability, then the request.
///
/// The same shape as [`create_retrieval_query`]. The wording is the one the
/// type skill's description carries, and of the four measured it is the one
/// that put that skill above every first-search leader.
pub fn new_kind_retrieval_query(query: &str) -> String {
    format!("define a new entity type with custom fields: {query}")
}

/// Whether a skill can define a type.
pub fn skill_can_define_a_type(candidate: &SkillCandidate) -> bool {
    candidate
        .tools
        .iter()
        .any(|t| Tool::from_name(t) == Some(Tool::CreateSchema))
}

/// Whether the candidate that would lead the turn can define a type.
pub fn leads_with_a_type_skill(ranked: &[SkillCandidate]) -> bool {
    leading_tool_bearing_candidate(&select_candidates(ranked.to_vec()))
        .is_some_and(skill_can_define_a_type)
}

/// Retrieval's ranking for one routing query, through `retrieve`: a search for
/// a query, asked for a number of candidates.
///
/// A query of neither shape below is one search, returned as it came back.
///
/// A request to add something ([`is_add_shaped`]) differs in two ways:
///
/// - It is ranked without the skills that can remove user data
///   ([`without_destructive_skills`]), and asked for
///   [`ADD_RETRIEVAL_FETCH`] so the ranking is still full without them.
/// - When none of the candidates Stage 2 would then judge can create a record,
///   retrieval runs once more on [`create_retrieval_query`], and the best
///   match that can create and cannot remove joins the ranking.
///
/// A request for a new kind of record ([`is_new_kind_shaped`]) differs in one:
/// when the candidate that would lead cannot define a type, retrieval runs
/// once more on [`new_kind_retrieval_query`], and the best match that can
/// joins the ranking. The lead is what is checked, not a place among the
/// candidates, because the fields declared on a write tool come only from the
/// leading candidates ([`declare_write_tool_fields`]) and the leader is the
/// skill the turn is recorded as routed to.
///
/// Either way the skill that joins has the score its own search gave it. That
/// score is on the second query's scale, which names the skill's own
/// capability, so the skill usually leads the turn it was added to. Every
/// other candidate keeps the score it had: the second search adds one skill
/// to the ranking and removes none from it, though the skill it adds can take
/// the last place Stage 2 judges.
///
/// A second search that fails leaves the first ranking as it was.
///
/// `agent_loop`'s `route` and the live retrieval guards both call this, so
/// what the guards measure is the ranking a turn runs on.
pub async fn retrieve_candidates<F, Fut, E>(
    query: &str,
    retrieve: F,
) -> Result<Vec<SkillCandidate>, E>
where
    F: Fn(String, usize) -> Fut,
    Fut: std::future::Future<Output = Result<Vec<SkillCandidate>, E>>,
    E: std::fmt::Display,
{
    if is_add_shaped(query) {
        let ranked =
            without_destructive_skills(retrieve(query.to_string(), ADD_RETRIEVAL_FETCH).await?);
        if !lacks_a_creating_skill(&ranked) {
            return Ok(ranked);
        }
        // Asked for as many as the first search: only one of them is kept,
        // and a workspace's own types can fill the first places of a shorter
        // ranking.
        let second = retrieve(create_retrieval_query(query), ADD_RETRIEVAL_FETCH)
            .await
            .map(without_destructive_skills);
        return Ok(with_the_best_match(
            ranked,
            second,
            skill_can_create_a_record,
            query,
        ));
    }
    let ranked = retrieve(query.to_string(), RETRIEVAL_FETCH).await?;
    if !is_new_kind_shaped(query) || leads_with_a_type_skill(&ranked) {
        return Ok(ranked);
    }
    // Asked for as many as an add's second search, for the same reason:
    // only one of them is kept, and a workspace's own types can fill the
    // first places of a shorter ranking.
    let second = retrieve(new_kind_retrieval_query(query), ADD_RETRIEVAL_FETCH).await;
    Ok(with_the_best_match(
        ranked,
        second,
        skill_can_define_a_type,
        query,
    ))
}

/// `ranked` with the best match of a second search that `can` do what the
/// request's shape asks for, at the score that search gave it. A skill
/// already ranked is moved, not repeated. A second search that failed, or
/// held no such skill above its score bar, leaves `ranked` as it was.
fn with_the_best_match<E: std::fmt::Display>(
    mut ranked: Vec<SkillCandidate>,
    second: Result<Vec<SkillCandidate>, E>,
    can: impl Fn(&SkillCandidate) -> bool,
    query: &str,
) -> Vec<SkillCandidate> {
    match second {
        Ok(second) => {
            // A procedure never leads a turn, and the skill a shape adds is
            // there to lead it.
            let found = second
                .into_iter()
                .find(|c| c.role != SkillRole::Procedure && clears_score_gate(c) && can(c));
            if let Some(found) = found {
                tracing::debug!(
                    skill = %found.name,
                    score = found.score,
                    "the ranking lacked a skill for what the request's shape asks; added one"
                );
                ranked.retain(|c| c.id != found.id);
                ranked.push(found);
                ranked.sort_by(|a, b| b.score.total_cmp(&a.score));
            }
        }
        Err(e) => {
            tracing::warn!(
                error = %e,
                query,
                "the second retrieval for a request's shape failed; continuing without it"
            );
        }
    }
    ranked
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RouteQueryParams {
    query: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RouteClarifyParams {
    question: String,
    #[serde(default)]
    options: Vec<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RouteMultiParams {
    queries: Vec<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RouteLookupParams {
    topic: String,
}

/// The four tools offered at Stage 1, and only these four.
///
/// Offering a fixed, small set makes the model's tool choice a discriminated
/// output: there is no free text to parse, only a tool name and typed
/// arguments. The alternative — asking in prose for a `QUERY:`/`CLARIFY:`
/// prefix — relies on the weakest measured channel and needs a parser whose
/// failures are indistinguishable from model failures.
pub fn stage1_tool_definitions() -> Vec<ToolDefinition> {
    vec![
        ToolDefinition {
            name: ROUTE_QUERY_TOOL.to_string(),
            description:
                "Use when you understand what the user wants. Provide a short search query \
                 describing the capability needed to fulfil it, keeping both the specific nouns \
                 the user named (what kind of thing, what item) AND the action or \
                 distinguishing detail that determines what kind of capability is needed — a \
                 status word, a value, or a verb like update/create/delete/list — rather than \
                 replacing either with a paraphrase or generalizing to a category-level \
                 description. Describe the task using the user's own subject and intent, not a \
                 reinterpreted or flattened one."
                    .to_string(),
            parameters_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "query": {
                        "type": "string",
                        "description": "A short description of the capability needed, in plain \
                             language, built around the exact noun(s) the user used for the \
                             subject (e.g. keep 'albums', 'venues', 'equipment' — do not \
                             substitute a different word for the same thing, like 'watchlist' \
                             or 'queue' for a category of item the user named directly) AND the \
                             specific action or detail that distinguishes what the user wants \
                             done — e.g. 'update the equipment record worth 2400 to returned', \
                             not just 'equipment items'; do not collapse an update/find-a-\
                             specific-item request into a generic listing description."
                    }
                },
                "required": ["query"]
            }),
        },
        // The scoping sentence ("Only when looking it up is the one thing the
        // user wants…") is load-bearing, not tidying. Without it this tool's
        // presence changed two requests that have nothing to do with lookups,
        // measured on the locked model: "set up a way to track X, then
        // separately find what I wrote about Y" stopped being split and kept
        // only its first half, and "help me manage our roadmap" stopped being
        // clarified. Removing the tool restored both; so did this sentence.
        // The same goes for where questions about the agent are pointed: said
        // here, they route as a query; said on route_query instead, they did
        // not. Guarded in `tests/it/live_stage1_golden_prompts.rs`.
        ToolDefinition {
            name: ROUTE_LOOKUP_TOOL.to_string(),
            description:
                "Use when the user asks a question about anything they may have written down or \
                 stored — how something works, what something is, why or when something was \
                 decided, who or where — or asks for something to be found, looked up, or \
                 listed. The answer is looked up in what they have stored, so this is always \
                 clear enough to route: an unfamiliar term is something to search for, never a \
                 reason to ask. Only when looking it up is the one thing the user wants: a \
                 request that also asks for something to be created, changed, or set up is \
                 route_multi, and a request to create, change, or delete something is \
                 route_query. Not for a question about you, the assistant — your skills, your \
                 tools, what you can do: nothing the user stored answers that, so use \
                 route_query with 'describe your own skills'."
                    .to_string(),
            parameters_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "topic": {
                        "type": "string",
                        "description": "What to look up, in the user's own words, complete \
                             enough to search on by itself — e.g. 'how invoices get approved'."
                    }
                },
                "required": ["topic"]
            }),
        },
        ToolDefinition {
            name: ROUTE_CLARIFY_TOOL.to_string(),
            description: "Use ONLY when the request is too ambiguous to describe as a capability. \
                 Ask one specific question and offer concrete alternatives — never a bare \
                 'what do you mean?'."
                .to_string(),
            parameters_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "question": {
                        "type": "string",
                        "description": "One specific question that would resolve the ambiguity."
                    },
                    "options": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "Two or more concrete interpretations the user can pick between."
                    }
                },
                "required": ["question"]
            }),
        },
        ToolDefinition {
            name: ROUTE_MULTI_TOOL.to_string(),
            description: "Use when the request contains two or more DISTINCT, UNAMBIGUOUS intents \
                 — the user clearly wants several separate things done, not one thing described at \
                 length. Provide one short query per intent, each built the same way route_query's \
                 query is (keep the subject noun AND the action/detail for that specific intent). \
                 Do NOT use this for a single intent, however it's phrased, and do NOT use this \
                 in place of route_clarify when a single intent is itself ambiguous — those are \
                 different problems."
                .to_string(),
            parameters_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "queries": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "One short capability query per distinct intent, in the \
                             order the user mentioned them, each following the same rules as \
                             route_query's query (keep the subject noun and the action/detail). \
                             Two or more entries required — a single intent belongs in route_query \
                             instead."
                    }
                },
                "required": ["queries"]
            }),
        },
    ]
}

/// Recover Stage-1's decision from the model's tool call.
///
/// Returns `None` when the model called neither routing tool or emitted
/// unparseable arguments. The caller treats that as "no routing decision" and
/// falls through to the general tool surface rather than failing the turn:
/// a routing step that cannot decide must not cost the user their request.
pub fn parse_route_decision(tool_name: &str, arguments_json: &str) -> Option<RouteDecision> {
    match tool_name {
        ROUTE_QUERY_TOOL => {
            let p: RouteQueryParams = serde_json::from_str(arguments_json).ok()?;
            let q = p.query.trim();
            if q.is_empty() {
                return None;
            }
            Some(RouteDecision::Query(q.to_string()))
        }
        ROUTE_CLARIFY_TOOL => {
            let p: RouteClarifyParams = serde_json::from_str(arguments_json).ok()?;
            let question = p.question.trim();
            if question.is_empty() {
                return None;
            }
            Some(RouteDecision::Clarify {
                question: question.to_string(),
                options: p.options,
            })
        }
        ROUTE_LOOKUP_TOOL => {
            let p: RouteLookupParams = serde_json::from_str(arguments_json).ok()?;
            let topic = p.topic.trim();
            if topic.is_empty() {
                return None;
            }
            Some(RouteDecision::Lookup(topic.to_string()))
        }
        ROUTE_MULTI_TOOL => {
            let p: RouteMultiParams = serde_json::from_str(arguments_json).ok()?;
            let queries: Vec<String> = p
                .queries
                .iter()
                .map(|q| q.trim().to_string())
                .filter(|q| !q.is_empty())
                .collect();
            // Fewer than two non-empty queries is not a genuine compound
            // intent — either the model over-called this tool for a single
            // intent (fall through to unrouted retrieval on the raw message
            // rather than trust a one-element "multi"), or every entry was
            // blank (no usable decision at all).
            if queries.len() < 2 {
                return None;
            }
            Some(RouteDecision::Multi(queries))
        }
        _ => None,
    }
}

/// The `routing_decision` log tag for a Stage-1 turn that yielded no decision.
///
/// Both outcomes fall through to retrieval on the raw message, but they are
/// different model behaviours: `"none"` means the model called no routing tool
/// (or emitted unparseable route_query/route_clarify arguments), while
/// `"multi_rejected"` means it called `route_multi` without two usable queries
/// (including arguments that would not parse) — splitting a single intent
/// into a "compound" one, the exact over-selection route_multi's guard clauses
/// exist to prevent. Collapsing both into `"none"` would let a routing eval
/// score that failure as a clean single-intent turn.
pub fn undecided_routing_tag<'a>(called_tools: impl IntoIterator<Item = &'a str>) -> &'static str {
    if called_tools.into_iter().any(|t| t == ROUTE_MULTI_TOOL) {
        "multi_rejected"
    } else {
        "none"
    }
}

/// Whether a skill can change graph state, derived from the tools it may fire.
///
/// Blast radius is not a stored property. It is computed from the skill's
/// `tool_whitelist` through the tool registry's own write classification, so
/// it cannot drift from what the skill is actually able to do — adding a
/// mutating tool to a whitelist raises that skill's bar automatically, with
/// no second field to keep in sync.
///
/// An unrecognised tool name counts as mutating. Resolving a name that is not
/// in the registry — a typo, a renamed tool, or a future externally-registered
/// one — yields no write classification at all, and treating that absence as
/// read-only would put the *lower* bar on a skill whose blast radius is
/// unknown. ADR-038 says to bias against the expensive error, so the unknown
/// case belongs on the restrictive side.
///
/// This gates a safety bar, not availability. `stage2_tools` fails open in the
/// other direction on purpose: there, an unknown name simply is not offered,
/// and stranding the model with no tools would be the worse outcome.
pub fn skill_is_mutating(candidate: &SkillCandidate) -> bool {
    candidate
        .tools
        .iter()
        .any(|t| Tool::from_name(t).is_none_or(Tool::is_write))
}

/// Whether a skill can irreversibly remove user data, derived from the tools
/// it may fire.
///
/// Computed from the registry's own classification for the same reason
/// [`skill_is_mutating`] is: adding `delete_node` to a whitelist raises that
/// skill's bar automatically, with no second field to keep in sync.
///
/// Unlike [`skill_is_mutating`], an unrecognised tool name counts as **not**
/// destructive — see [`super::tools::removes_user_data_tool`] for why the
/// unknown case belongs on the opposite side here. In short: an unknown name
/// is already treated as mutating, and treating it as destructive too would
/// apply the strictest bar in the system to any skill with a typo in its
/// whitelist.
pub fn skill_is_destructive(candidate: &SkillCandidate) -> bool {
    candidate
        .tools
        .iter()
        .any(|t| super::tools::removes_user_data_tool(t))
}

/// The retrieval score a candidate must clear to be actionable.
///
/// Scales with blast radius per ADR-038: read-only < mutating < destructive.
/// Checked most-restrictive-first, since a destructive skill is also a
/// mutating one and must not stop at the lower rung.
pub fn score_bar_for(candidate: &SkillCandidate) -> f32 {
    if skill_is_destructive(candidate) {
        DESTRUCTIVE_SKILL_SCORE_BAR
    } else if skill_is_mutating(candidate) {
        MUTATING_SKILL_SCORE_BAR
    } else {
        READ_SKILL_SCORE_BAR
    }
}

/// Whether a candidate clears the **mechanical** half of the Stage-2 gate.
///
/// Passing this is necessary, not sufficient: ADR-038 requires two
/// independently-sourced signals, and the model's judgment supplies the other.
/// A candidate that clears this bar is offered to the model to judge; one that
/// does not is never actionable regardless of what the model says.
///
/// A skill the chat pins clears it without a score (ADR-090 §5). The score
/// stands for evidence that the request is about the skill, and a pin is
/// that evidence already: the chat was created for it.
///
/// Not a destructive skill. A pin says what the chat is for, not that this
/// message asks to remove something, and ADR-038 biases against the
/// expensive error: a pinned skill that can remove user data still has to
/// clear its own bar.
pub fn clears_score_gate(candidate: &SkillCandidate) -> bool {
    (candidate.pinned && !skill_is_destructive(candidate))
        || candidate.score >= score_bar_for(candidate)
}

/// The candidates Stage 2 judges in a chat that pins skills: `selected`, with
/// every pinned skill among them (ADR-090 §5).
///
/// A pinned skill that retrieval also selected is marked, not repeated. One
/// it did not select is added after the others, with the score retrieval gave
/// it (`retrieved_scores`, by skill id) or none. Its score still decides
/// whether it leads the turn, so a request on another topic is led by the
/// skill retrieval found for it.
///
/// With no pinned skill, `selected` is returned as it was.
pub fn with_pinned_skills(
    mut selected: Vec<SkillCandidate>,
    pinned: &[SkillCandidate],
    retrieved_scores: &std::collections::HashMap<String, f32>,
) -> Vec<SkillCandidate> {
    for skill in pinned {
        match selected.iter_mut().find(|c| c.id == skill.id) {
            Some(found) => found.pinned = true,
            None => selected.push(SkillCandidate {
                score: retrieved_scores.get(&skill.id).copied().unwrap_or(0.0),
                pinned: true,
                ..skill.clone()
            }),
        }
    }
    selected
}

/// The highest score among gate-clearing candidates that whitelist at least
/// one tool — "which *tool-bearing* candidate won retrieval". `NEG_INFINITY`
/// when there are none, which every `score >= max` comparison admits, so a
/// tool-less candidate set imposes no ceiling on anyone.
///
/// The invariant this exists to hold: **a candidate that whitelists no tool
/// must not influence which tool-bearing candidate wins.** A candidate with an
/// empty `tools` vec can never contribute a tool name or a descriptor, so it
/// has no stake in who else may — it can only raise the bar that decides them.
///
/// An empty whitelist is the observable property this keys on, and the only
/// one visible at this boundary: `SkillCandidate` carries no notion of
/// candidate kind. Today's producer of such candidates is schema-typed
/// retrieval hits, which describe a schema rather than a capability and carry
/// `tools: []` by construction. One clears the gate unconditionally at the
/// read rung while the lexical backstop pins it at a fixed confidence above
/// any cosine-derived score a real skill can reach — so on every turn where a
/// schema was named outright it took the top score away from a genuinely
/// matching skill. Both consumers below were separately broken by exactly
/// that, which is why the rule is one named thing rather than a coincidence
/// between call sites.
///
/// Computed by explicit max rather than by taking the first candidate: callers
/// do sort by score descending before truncating to `RETRIEVAL_TOP_K`
/// (`agent_loop`'s `route`), but a safety property should not depend on
/// another function's ordering staying that way. Ties keep every candidate at
/// the top score, which both consumers treat alike.
///
/// This narrows the population the max folds over; it does **not** relax the
/// global-max rule either consumer applies with the result.
/// Whether a candidate is in the running to lead the turn: it clears its own
/// blast-radius bar **and** whitelists at least one tool.
///
/// The predicate itself, named once. Both consumers below filter on it, and an
/// earlier revision wrote it out twice — which is the shape this file's own
/// history warns about, since the rule has already drifted between call sites
/// three times. A duplicated `&&` looks too cheap to be worth naming and is
/// exactly what silently diverges.
fn is_tool_bearing_contender(candidate: &SkillCandidate) -> bool {
    clears_score_gate(candidate) && !candidate.tools.is_empty()
}

/// Whether a candidate can lead the turn: a tool-bearing contender that is
/// not a procedure. A procedure scores on requests that belong to a tool
/// skill, and letting it lead would withhold that skill's destructive tool
/// (ADR-038).
fn can_lead(candidate: &SkillCandidate) -> bool {
    is_tool_bearing_contender(candidate) && candidate.role != SkillRole::Procedure
}

fn top_tool_bearing_score<'a>(candidates: impl Iterator<Item = &'a SkillCandidate>) -> f32 {
    candidates
        .filter(|c| can_lead(c))
        .map(|c| c.score)
        .fold(f32::NEG_INFINITY, f32::max)
}

/// The candidate whose whitelist leads the turn — the highest-scoring one that
/// both clears its own bar and actually carries tools.
///
/// The same invariant [`top_tool_bearing_score`] holds, in the shape a caller
/// wants when it needs the candidate itself rather than its score. Both exist
/// because a tool-less candidate has no stake in which tool-bearing candidate
/// wins: a schema-typed retrieval hit carries `tools: []` by construction, and
/// the lexical backstop pins it at a fixed confidence above any cosine-derived
/// score a real skill can reach, so on any turn naming a schema outright it
/// sorts first while contributing nothing to the offered surface.
///
/// Exposed rather than re-derived at each call site: three consumers have now
/// been written against this predicate, and the two that re-derived it were
/// both wrong in the same way before being corrected. A `.find(clears_score_gate)`
/// looks equivalent and is not.
///
/// Takes the max explicitly rather than the first match for the reason
/// [`top_tool_bearing_score`] gives — callers do sort by score descending, but
/// a rule about which candidate leads should not depend on another function's
/// ordering staying that way.
///
/// **Ties differ from [`top_tool_bearing_score`], deliberately.** That function
/// returns a *score*, and its consumers admit every candidate matching it
/// (`c.score >= top_score`), so tied candidates all contribute — see
/// `declare_write_tool_fields_unions_tied_top_scoring_candidates`. This returns
/// *one* candidate, because its consumer records a single name: `DecisionRecord`
/// has no shape for "these two tied". On a tie `max_by` yields the last maximal
/// element, so the answer is stable for a given input order but is not a
/// modelled preference between equals. Do not use this where the union matters;
/// use [`top_tool_bearing_score`] and compare against it.
pub fn leading_tool_bearing_candidate(candidates: &[SkillCandidate]) -> Option<&SkillCandidate> {
    candidates
        .iter()
        .filter(|c| can_lead(c))
        .max_by(|a, b| a.score.total_cmp(&b.score))
}

/// The search the system runs for a lookup that Stage 2 left unsearched, or
/// `None` when this turn's surface does not offer the tool to run it with.
///
/// `search_semantic`, because the topic is in the user's words and that is
/// what it matches on; `search_nodes` matches a keyword against titles and
/// would find nothing for a sentence. Three results come back with their
/// text rather than a snippet, which is what answering a question needs. Long
/// documents are cut off at the tool's own limit.
///
/// Only a tool on the surface is called. The lookup skill is retrieved for
/// every lookup, so the tool is there unless a registry was edited to remove
/// it — and then the turn is left as the model ended it.
pub fn lookup_call(topic: &str, surface: &[ToolDefinition]) -> Option<ToolCallRaw> {
    let tool = Tool::SearchSemantic.name();
    surface.iter().any(|t| t.name == tool).then(|| ToolCallRaw {
        // Unique per call: the id is replayed in the chat's history, and a
        // chat with several lookups must not carry the same one twice.
        id: format!("system_lookup_{}", uuid::Uuid::new_v4().simple()),
        function_name: tool.to_string(),
        arguments_json: serde_json::json!({
            "query": topic,
            "include_markdown": LOOKUP_FULL_RESULTS,
        })
        .to_string(),
        provider_extra: None,
    })
}

/// How many results a system-run lookup returns with their text.
const LOOKUP_FULL_RESULTS: u32 = 3;

/// Render the registry's skill names for the Stage-2 prompt, or `None` when
/// there are none to name.
///
/// A question about the agent's own skills is not a question about the user's
/// knowledge. Skills are system nodes, outside the default search scope, so a
/// search for one finds nothing and the model reports that no such skill
/// exists. The registry is the answer: this states it in the prompt, beside
/// the routed procedures, and says where such a question is answered from.
///
/// It states a fact rather than forbidding the search. "Find the skill for X"
/// reads as a find request, Stage 1 routes it as a lookup, and a lookup is
/// always searched; an instruction never to search would contradict the turn
/// it is shown on. What the model needs there is to know the search result is
/// not where the answer is.
///
/// `names` is the list Stage 1 already carries
/// ([`super::agent_loop::stage1_skill_names`]), so both stages name the same
/// skills, normalised the same way.
pub fn render_skill_names_for_prompt(names: &[String]) -> Option<String> {
    if names.is_empty() {
        return None;
    }
    Some(format!(
        "YOUR SKILLS: {}.\nA question about your own skills, or about what you can do, is \
         answered from this list and from any procedure shown below, including when the user \
         asks you to find one. Skills are not stored in the user's notes, so a search does not \
         return them.",
        names.join(", ")
    ))
}

/// Names of the candidates that clear the score gate, comma-separated, for the
/// `routed_skills` log field.
///
/// The same set `render_candidates_for_prompt` writes into Stage 2's prompt and
/// `stage2_tools` scopes the tool surface from, so a log reader can tell which
/// skill a turn was actually routed to rather than only how many candidates
/// retrieval returned.
///
/// A function rather than an inline expression at the log site specifically so
/// the rendered text is testable. The scraper consuming this field parses to
/// end of line — skill names contain spaces and commas, and `tracing` emits the
/// value unquoted — so its exact shape is a contract, and the first version of
/// that scraper silently matched nothing because the shape was assumed rather
/// than asserted.
pub fn routed_skill_names(candidates: &[SkillCandidate]) -> String {
    candidates
        .iter()
        .filter(|c| clears_score_gate(c))
        .map(|c| c.name.as_str())
        .collect::<Vec<_>>()
        .join(", ")
}

/// Every retrieved candidate's name and raw score, comma-separated, for the
/// `all_scores` log field — **not** filtered by [`clears_score_gate`], unlike
/// [`routed_skill_names`].
///
/// `routed_skill_names` and the OTel `routing.top_score` attribute together
/// answer "which skill won" and "how well did the winner score", but neither
/// distinguishes a tiebreak (the runner-up scored 0.81 against the winner's
/// 0.82) from a skill that scored badly on its own core use case (0.3, no
/// close competitor) — two defects with different fixes that look identical
/// from the winner's score alone. This is the field that tells them apart
/// from ordinary logs, without a dedicated measurement run.
///
/// Preserves retrieval's score-descending order (the order `route`'s `merged`
/// is already sorted into) rather than re-sorting, so the log line's order
/// matches rank.
pub fn all_candidate_scores(candidates: &[SkillCandidate]) -> String {
    candidates
        .iter()
        .map(|c| format!("{}={:.3}", c.name, c.score))
        .collect::<Vec<_>>()
        .join(", ")
}

/// The candidates a lookup turn can use: those that whitelist no write tool.
///
/// A lookup reads. The procedures of a skill that writes (Node Deletion,
/// Implementing a Task) ride along when retrieval places them second or third,
/// and with them the schema-writing and node-writing tools they whitelist:
/// thousands of tokens of prompt and tool schema a read cannot use, which a
/// local model pays for as time before its first token. When no candidate is
/// read-only, or none that is clears the score gate, the candidates are
/// returned as retrieved: narrowing must never leave the turn without a
/// procedure.
///
/// The saving is prompt text, not safety: destructive tools were already
/// offered only from the winning candidate. The cost is accepted: a message
/// Stage 1 labels a lookup but that also asks for a change ("find the task and
/// mark it done") runs the turn without the write skills, and the model says
/// it cannot, rather than acting.
pub fn read_only_candidates(candidates: Vec<SkillCandidate>) -> Vec<SkillCandidate> {
    let read_only: Vec<SkillCandidate> = candidates
        .iter()
        .filter(|c| !c.tools.iter().any(|t| super::tools::is_write_tool(t)))
        .cloned()
        .collect();
    if read_only.iter().any(clears_score_gate) {
        read_only
    } else {
        candidates
    }
}

/// Render retrieved candidates for injection into the Stage-2 prompt.
///
/// Delivered **in the prompt** rather than as a tool result. ADR-064 rule 4
/// reserves tool results for resolved facts rather than procedures, and the
/// supporting measurement for skill instructions was taken on prompt-rendered
/// text; the same payload returned as a tool result was observed to suppress
/// tool-calling substantially.
///
/// Only candidates clearing the score gate are rendered — the mechanical gate
/// runs before the judged one, so the model is never asked to judge a
/// candidate the system has already ruled out.
pub fn render_candidates_for_prompt(candidates: &[SkillCandidate]) -> Option<String> {
    let eligible: Vec<&SkillCandidate> =
        candidates.iter().filter(|c| clears_score_gate(c)).collect();
    if eligible.is_empty() {
        return None;
    }

    // Phrased as a direct instruction to act, not to deliberate. An earlier
    // wording ("Pick the ONE that fits and carry out its instructions") framed
    // the turn as a selection exercise, and the model answered it in kind:
    // it narrated which tool it would use — in one case emitting the tool-call
    // JSON inside a code block — instead of calling it. Measured on
    // mistral:7b, that wording produced zero tool calls where the unrouted
    // control produced one.
    //
    // "If none applies" is not a working escape on the locked model. A closing
    // instruction to judge each procedure by what it does and call
    // route_clarify when none fits was measured on gemma-4-e4b against a
    // lexical false-positive top candidate ("mark it resolved" → Conflict
    // Resolution): 0 of 15 clarified, identical to this wording, at every step
    // from the first action through two successful lookups. The model keeps
    // reading rather than noticing no offered tool can write. The fix for that
    // case lives in retrieval (the skill descriptions), not here.
    let mut out = String::from(
        "REFERENCE — procedures relevant to this request. Use whichever applies and IGNORE the \
         rest. Do not describe, quote, or summarise any of it. Your reply must be the tool call \
         itself. If none applies, answer the user normally.\n",
    );
    for (i, c) in eligible.iter().enumerate() {
        out.push_str(&format!("\n--- Candidate {}: {}\n", i + 1, c.name));
        if !c.use_for.is_empty() {
            out.push_str(&format!("Purpose: {}\n", c.use_for));
        }
        if !c.instructions.is_empty() {
            out.push_str(&format!("\n{}\n", c.instructions));
        }
        if let Some(meta) = render_schema_metadata(&c.schema_metadata) {
            // Shared heading with context_ops.rs's resident workspace-context
            // block — see EXISTING_SCHEMAS_HEADER's doc comment. This
            // per-candidate rendering is a second, independent site that
            // shows the same schema metadata; a fix applied only to the
            // resident copy left this one to reinforce the exact
            // contamination the resident copy was changed to guard against.
            out.push_str(&format!("\n{EXISTING_SCHEMAS_HEADER}\n{meta}\n"));
        }
    }
    Some(out)
}

/// Wire names of tools that both require routed guidance (see
/// [`super::tools::Tool::requires_routed_guidance`]) and have that guidance
/// actually available: an eligible candidate whitelists the tool AND a
/// `EXISTING SCHEMAS` block will reach this turn's prompt.
///
/// The block reaches the prompt from **either** of two independent sites, and
/// the tool's required parameter cannot tell them apart — it points at a
/// heading, and one shared constant renders that heading at both:
///
/// - **Per-candidate**: this candidate's own `schema_metadata`, rendered into
///   the Stage-2 candidate block by [`render_candidates_for_prompt`].
/// - **Workspace context**: schemas retrieved semantically for the turn by
///   `context_ops::build_workspace_context`, already resident in the system
///   prompt (`agent_loop`'s `session.dynamic_context`).
///
/// Consulting only the first was a reachability bug rather than a
/// conservative approximation: `resolve_query` is whitelisted by a skill
/// (Graph Editing) that links to no schema, so `skill_ops` falls back
/// to "all non-core schemas" for its `schema_metadata`. In a workspace with
/// no custom schema that fallback is empty, and the tool was withheld from
/// every turn — including turns whose workspace context *did* carry the
/// block. The gate's purpose is to never offer a tool whose required
/// parameter points at something the model cannot see; a block the model can
/// see satisfies that purpose whichever site rendered it.
///
/// Both checks require a type *listed*, not merely a heading: each renderer
/// can emit the heading over nothing — `render_candidates_for_prompt` from
/// its header text alone, and `context_ops`'s from stopping once its
/// character budget is spent. Either would strand the model with a required
/// `node_type` pointing at an empty list.
///
/// **Core types are out of scope by construction.** Every path that fills the
/// block today drops `is_core` schemas (`skill_ops`'s *unscoped* non-core
/// fallback — the branch that applies to `resolve_query`'s unscoped
/// whitelisting skill — and `context_ops::non_core_schema_hits`),
/// so a bare-value update against `task`/`text` renders no block from either
/// source and the tool stays withheld. That matches `resolve_query`'s own
/// description, whose examples are all custom-type (an amount, an invoice, a
/// code); it is a deliberate boundary, not an oversight this function should
/// paper over. See `def_resolve_query` for the one latent path that would
/// widen it.
///
/// `render_disabled: true` (mirrors the caller's `session.routing_disabled`)
/// suppresses only the *candidate* path — that flag exists because injecting
/// the candidate block suppresses tool-calling on some models, which says
/// nothing about the resident workspace block that is present regardless.
pub fn tools_with_available_guidance<'a>(
    candidates: &'a [SkillCandidate],
    render_disabled: bool,
    workspace_context: &str,
) -> std::collections::HashSet<&'a str> {
    // The heading alone is not the guidance — at least one type has to be
    // listed under it. `context_ops`'s renderer emits the heading and then
    // breaks out of its per-schema loop once the character budget is spent,
    // so a budget exhausted by the first line leaves a bare heading behind.
    // Matching on the heading alone would offer the tool while its required
    // `node_type` pointed at an empty list — precisely the strand this gate
    // exists to prevent.
    // Both renderers emit one `- <type_id>...` line per type (see
    // `entity_types_block::EntityTypeDescriptor::render_line`), so the first
    // non-blank line after the heading is the check.
    let workspace_has_block = workspace_context
        .split_once(EXISTING_SCHEMAS_HEADER)
        .and_then(|(_, after)| after.lines().find(|l| !l.trim().is_empty()))
        .is_some_and(|first| first.trim_start().starts_with("- "));
    candidates
        .iter()
        .filter(|c| clears_score_gate(c))
        .filter(|c| {
            // A whitelisting candidate still has to clear the score gate: the
            // workspace block makes the *parameter* answerable, it does not
            // make an unmatched skill's tools eligible.
            workspace_has_block
                || (!render_disabled && render_schema_metadata(&c.schema_metadata).is_some())
        })
        .flat_map(|c| c.tools.iter().map(|t| t.as_str()))
        .collect()
}

/// Render a candidate's `schema_metadata` into the compact form the model
/// already sees elsewhere, or `None` when there is nothing to show.
///
/// Delegates to the shared renderer in `nodespace-core`. This block and the
/// workspace-context one are concatenated into a single system prompt under the
/// same heading, so they must describe a type identically; they previously did
/// not, and the model followed guidance whose referent only one of them
/// emitted. The `schema_metadata` JSON this decodes is produced from the same
/// descriptor type, making the encode/decode one reversible mapping rather than
/// two hand-written projections.
fn render_schema_metadata(meta: &serde_json::Value) -> Option<String> {
    let descriptors = nodespace_core::ops::entity_types_block::descriptors_from_json(meta);
    nodespace_core::ops::entity_types_block::render_entity_types(&descriptors)
}

/// Wire names the eligible candidates permit, with the destructive-tool rule
/// applied. The single source of truth for that rule: [`stage2_tools`] scopes
/// from it and [`destructive_tools_withheld`] reports against it, so the log
/// cannot claim something different from what the model was offered.
fn stage2_permitted_names(candidates: &[SkillCandidate]) -> std::collections::HashSet<&str> {
    // Only a tool-bearing candidate's score decides whose destructive tools
    // are trusted — see [`top_tool_bearing_score`] for the invariant and for
    // what counting tool-less candidates broke here (a genuinely-matching
    // deletion skill silently lost `delete_node` whenever the query named a
    // schema outright: directionally safe but functionally wrong, the tool
    // vanishing for no reason the model or user can see).
    let top_score = top_tool_bearing_score(candidates.iter());

    candidates
        .iter()
        .filter(|c| clears_score_gate(c))
        .flat_map(|c| {
            let is_top = c.score >= top_score;
            c.tools
                .iter()
                .map(|t| t.as_str())
                .filter(move |t| is_top || !super::tools::removes_user_data_tool(t))
        })
        .collect()
}

/// The tools Stage 2 may offer, restricted to what the eligible candidates'
/// whitelists permit: the union of them for ordinary tools, and — for tools
/// that irreversibly remove user data — only the whitelist of the candidate
/// that actually won retrieval. See [`stage2_permitted_names`] for why.
///
/// This is the trust boundary ADR-038 places on the system side: the model
/// judges *among* what retrieval surfaced, and can only fire what the matched
/// skills permit. A skill's `tool_whitelist` was previously read only by
/// external ACP agents; this gives it a local-agent consumer.
///
/// Falls back to the full surface when nothing was retrieved, so an
/// unavailable embedding service degrades to today's behaviour rather than
/// leaving the model with no tools at all — minus any tool whose required
/// parameters depend on the `EXISTING SCHEMAS` block (see
/// [`super::tools::Tool::requires_routed_guidance`] and
/// [`fail_open_surface`]). That exclusion is about *eligibility*, not about
/// the block being absent: workspace context renders it independently of
/// routing, so it may well be present on a fail-open turn. But fail-open
/// means retrieval matched nothing, and ADR-038 puts the trust boundary at
/// what retrieval surfaced. The model still has every other tool to answer
/// the request with.
pub fn stage2_tools(candidates: &[SkillCandidate], all: &[ToolDefinition]) -> Vec<ToolDefinition> {
    stage2_scoped_tools(candidates, all).unwrap_or_else(|| fail_open_surface(all))
}

/// [`stage2_tools`], or `None` where it would fail open to the full surface.
///
/// The turn-end guards need to know which of the two the model ran on: a
/// scoped surface means retrieval chose the capabilities, so a turn that
/// could not finish on it may have been handed the wrong ones.
pub fn stage2_scoped_tools(
    candidates: &[SkillCandidate],
    all: &[ToolDefinition],
) -> Option<Vec<ToolDefinition>> {
    // The union is right for ordinary tools and wrong for destructive ones.
    // A skill contributes its whole whitelist to the surface merely by being
    // one of the (up to three) candidates above its bar — so a weak
    // second/third-place `Node Deletion` put `delete_node` in front of the
    // model on requests that were recording a decision or setting a due date,
    // while contributing nothing the turn actually needed. That is the
    // opposite of ADR-038's "the expensive error is gated hardest": riding
    // along in retrieval cost the destructive skill nothing.
    //
    // So destructive tools are admitted only from the candidate that actually
    // *won* retrieval this turn. A skill that best matches a deletion request
    // still offers `delete_node`; one that merely placed does not.
    //
    // Note this narrows only which candidates may contribute a destructive
    // tool. It is not a second score gate — the winner still had to clear
    // `DESTRUCTIVE_SKILL_SCORE_BAR` to be eligible at all.
    let permitted = stage2_permitted_names(candidates);

    if permitted.is_empty() {
        return None;
    }
    let scoped: Vec<ToolDefinition> = all
        .iter()
        .filter(|t| permitted.contains(t.name.as_str()))
        .cloned()
        .collect();

    // A whitelist naming only tools this build does not register would strand
    // the model with nothing to call. Fail open to the full surface.
    (!scoped.is_empty()).then_some(scoped)
}

/// Add `route_clarify` to a Stage-2 surface when the registry offers it and
/// the surface does not already carry it.
///
/// [`stage2_tools`] scopes to the candidates' whitelists, so a turn whose
/// retrieval surfaced only skills that do not whitelist `route_clarify` had no
/// way to say "none of these fits" — the case where the top candidate won on a
/// shared word rather than on what it does. `route_clarify` performs nothing,
/// so offering it widens no trust boundary; it is ADR-038's Stage-2 clarify
/// branch. The caller decides *whether* to offer it — the clarification
/// contract allows one per intent.
pub fn with_stage2_clarify(
    mut tools: Vec<ToolDefinition>,
    all: &[ToolDefinition],
) -> Vec<ToolDefinition> {
    if tools.iter().any(|t| t.name == ROUTE_CLARIFY_TOOL) {
        return tools;
    }
    if let Some(clarify) = all.iter().find(|t| t.name == ROUTE_CLARIFY_TOOL) {
        tools.push(clarify.clone());
    }
    tools
}

/// Names of destructive tools that a gate-clearing candidate whitelisted but
/// [`stage2_tools`] withheld, because that candidate did not win retrieval.
///
/// Purely for the log line — it re-reads the same rule `stage2_tools` applies
/// rather than reimplementing it, so the two cannot disagree about what was
/// withheld. Empty on the overwhelming majority of turns.
///
/// This exists because the failure it reports is otherwise invisible. When
/// scoping removes the tool a turn needed, the model has no evidence a better
/// tool ever existed: best case it asks the user a confused clarifying
/// question, worst case it reaches for the wrong tool from the narrowed set.
/// Neither the daemon log nor the turn output distinguished that from a model
/// that simply failed to call the tool, so a routing defect read as a model
/// defect.
/// Destructive candidates that retrieval returned but
/// [`DESTRUCTIVE_SKILL_SCORE_BAR`] rejected, as `(name, score)`.
///
/// Distinct from [`destructive_tools_withheld`], which reports a skill that
/// *was* eligible but did not win. This reports one that never became
/// eligible: it would have cleared the mutating bar and been actionable before
/// the destructive rung existed.
///
/// Exists to make a specific, acknowledged risk measurable in the field
/// instead of waiting for a user to report it. Two changes landed together
/// that push the same quantity in opposite directions: the bar a deletion
/// match must clear went **up** to `DESTRUCTIVE_SKILL_SCORE_BAR`, while the
/// `Node Deletion` description was narrowed — and a narrower description
/// embeds *further* from an indirectly-phrased real request ("get rid of that
/// old meeting note"). Both moves are individually justified and the bar is a
/// documented placeholder, but neither was measured against live embeddings,
/// so the band a genuine deletion has to land in is not known.
///
/// A turn appearing here is the signal that the bar is too high: the user
/// asked for something the system read as deletion, and it was dropped
/// entirely rather than offered. Scores logged alongside the names so the
/// distribution can be read off existing logs rather than needing a new eval
/// run to discover it.
pub fn destructive_candidates_below_bar(candidates: &[SkillCandidate]) -> Vec<(&str, f32)> {
    candidates
        .iter()
        .filter(|c| skill_is_destructive(c) && !clears_score_gate(c))
        .map(|c| (c.name.as_str(), c.score))
        .collect()
}

pub fn destructive_tools_withheld(candidates: &[SkillCandidate]) -> Vec<&str> {
    let offered: std::collections::HashSet<&str> = stage2_permitted_names(candidates);
    let mut withheld: Vec<&str> = candidates
        .iter()
        .filter(|c| clears_score_gate(c))
        .flat_map(|c| c.tools.iter().map(|t| t.as_str()))
        .filter(|t| super::tools::removes_user_data_tool(t))
        .filter(|t| !offered.contains(t))
        .collect();
    withheld.sort_unstable();
    withheld.dedup();
    withheld
}

/// Declares `field_values` sub-properties (see
/// [`super::tools::with_declared_field_values`]) on any tool in `tools`
/// shaped for it, sourced from the retrieved-schema data of whichever
/// candidate(s) attain the TURN'S GLOBAL top score among those that cleared
/// the gate — not the top score among only the candidates whitelisting that
/// specific tool. Precise about this because the two read very differently:
/// a tool whitelisted ONLY by a candidate that is not (one of) the turn's
/// overall highest scorer(s) is declared from **no** descriptors and stays
/// on the bare-object fallback, even if that candidate is the sole one
/// offering the tool at all.
///
/// The distractor scenario is why this is the global max rather than a
/// per-tool one: a turn that offers `create_node` alongside `create_schema`
/// specifically so the model can choose wrongly. `create_node` there is
/// whitelisted only by the lower-scoring Node Creation candidate — a
/// per-tool max would find Node Creation as "the top scorer among
/// create_node's own whitelisters" (trivially, being the only one) and
/// declare its fields anyway, making the wrong tool easier to use on
/// exactly the turn that must not reward it. The global-max rule is what
/// actually excludes it: `create_node` only gets declared fields when a
/// candidate whitelisting it ALSO happens to be the turn's overall best
/// match, not merely the best match among candidates that want that
/// specific tool.
///
/// The rule is enforced by `declare_write_tool_fields_leaves_a_lower_scored_candidates_tool_bare`
/// below, which is its only mechanical constraint. `dev-schema-creation.toml`
/// (`packages/agent/goldens/`) shaped the scenario and measures the model
/// never calling `create_node`, but it does NOT exercise this function:
/// the golden runner builds its tool list straight from the TOML and lists
/// routing as out of scope, so it constructs no candidates and asserts
/// nothing here. Treat it as the motivating case, not as a gate that would
/// catch a regression in this code.
///
/// **The known cost of that choice**, caught in review: on a genuinely
/// compound turn — two independently-relevant skills clear the gate for two
/// DIFFERENT, non-competing tools, e.g. a request that both creates a
/// ticket and links it to a sprint — `stage2_tools` legitimately offers
/// both tools (it unions every cleared candidate's whitelist, uncapped by
/// score), but this function will decline to declare fields for whichever
/// tool's candidate is not the turn's single highest scorer, even though
/// that candidate is in no sense a distractor. That turn's lower-priority
/// tool falls back to the pre-existing bare-object-plus-prose shape — not a
/// regression relative to today's production (every write tool is on that
/// shape today), just a missed improvement for that turn. Nothing in the
/// corpus exercises a genuine two-different-tools compound turn, so there is
/// no measured guidance on how to tell it apart mechanically from the
/// distractor case using only candidate score and tool whitelist — doing so
/// well likely needs per-skill retrieval scoping (ranking each tool's
/// candidates independently rather than by one global score) to distinguish
/// "beaten by a better candidate for the same tool" from "a different tool
/// entirely, just scored lower."
/// Pinned by `declare_write_tool_fields_does_not_declare_a_non_top_scoring_but_uncontested_tool`
/// below so this is a documented, deliberate trade-off rather than a latent
/// surprise.
///
/// Ties at the top score are unioned rather than one arbitrarily shadowing
/// the other (see `tools::declared_field_values_properties`) — the fixture
/// this snapshot gate exercises scores its two candidates identically on
/// purpose, and both legitimately whitelist `create_node`.
///
/// Deliberately a separate step from [`stage2_tools`], not folded into it —
/// this injects retrieved-schema CONTENT into the tool surface, the same
/// class of payload `agent_loop.rs`'s `candidate_block` skips injecting into
/// the prompt when `session.routing_disabled`. The routing-reliability
/// matrix (`tests/it/live_openai_compat_routing.rs`) found that injecting that
/// content suppressed tool-calling outright on some served models,
/// independent of the block's content — the finding was about *any*
/// retrieved-schema payload reaching the model, not specifically the
/// prompt-text channel it happened to be measured through. `stage2_tools`'s
/// own tool-list SCOPING (which tools are offered at all) is a different,
/// already-proven-safe mechanism and stays unconditional; this CONTENT step
/// is the caller's responsibility to gate the same way `candidate_block` is,
/// so a model probed unsafe for one retrieved-schema channel is not handed
/// materially the same content through another, unmeasured one.
pub fn declare_write_tool_fields(
    candidates: &[SkillCandidate],
    tools: Vec<ToolDefinition>,
) -> Vec<ToolDefinition> {
    let cleared: Vec<&SkillCandidate> =
        candidates.iter().filter(|c| clears_score_gate(c)).collect();
    // Only a tool-bearing candidate's score decides whose descriptors are
    // declared — see [`top_tool_bearing_score`] for the invariant and for
    // what counting tool-less candidates broke here (every write tool left on
    // the bare-object fallback whenever the query named a schema outright: the
    // schema the user named was exactly what stopped its own fields being
    // declared).
    //
    // The global-max rule this applies the result to stays load-bearing for
    // the distractor case (see the trade-off above and its two pinning tests).
    let max_score = top_tool_bearing_score(cleared.iter().copied());
    tools
        .into_iter()
        .map(|tool| {
            let descriptors: Vec<_> = cleared
                .iter()
                .filter(|c| c.score >= max_score && c.tools.iter().any(|t| t == &tool.name))
                .flat_map(|c| {
                    nodespace_core::ops::entity_types_block::descriptors_from_json(
                        &c.schema_metadata,
                    )
                })
                .collect();
            super::tools::with_declared_field_values(tool, &descriptors)
        })
        .collect()
}

/// The type ids `candidates` carry in their `schema_metadata`, deduplicated
/// in presentation order — several candidates can carry the same type, and a
/// duplicated option is not a wider choice.
///
/// Decoded through `entity_types_block`'s shared descriptor, the one
/// [`render_candidates_for_prompt`] renders from, so a list built here names
/// a type exactly when the block would.
pub fn type_ids<'a>(candidates: impl Iterator<Item = &'a SkillCandidate>) -> Vec<String> {
    let mut seen: Vec<String> = Vec::new();
    for c in candidates {
        for d in nodespace_core::ops::entity_types_block::descriptors_from_json(&c.schema_metadata)
        {
            if !seen.contains(&d.type_id) {
                seen.push(d.type_id);
            }
        }
    }
    seen
}

/// The types a routed turn is held to, when its matched skills all link to
/// their schemas. `None` when the turn has no such set, which leaves its tool
/// schemas and its dispatch as they are.
///
/// **Whether there is a set** is decided by the candidates that can act: the
/// ones that clear their score bar and whitelist a tool. Every one of them
/// must carry its linked schemas ([`SkillCandidate::schemas_linked`]). A
/// skill that links to none carries retrieval's fallback instead — the type
/// the query names, or the first few custom types — which is a guess at
/// relevance: holding a turn to a guess would refuse a correct call on a type
/// the guess left out. One unlinked candidate is enough to leave the whole
/// turn open, because its tools may act on any type. A candidate that
/// whitelists no tool cannot act, so it cannot leave the turn open.
///
/// **What is in the set** is every type [`render_candidates_for_prompt`]
/// lists: the `schema_metadata` of every candidate that clears its score bar,
/// tool-bearing or not. The enforced set is therefore exactly the set the
/// candidate block shows. A schema-typed retrieval hit is how a custom type
/// the request names outright reaches that block; leaving it out of the set
/// would refuse a call on the one type the user asked for by name. A core
/// type is never a schema hit, so naming `task` does not add it: a held turn
/// offers a core type only when a skill links to it.
///
/// A tool-less candidate that is a skill rather than a schema hit carries the
/// unlinked fallback, and those types join the block and the set alike. That
/// widens the set and refuses nothing the block shows.
///
/// The caller gates this like [`declare_write_tool_fields`]: a turn whose
/// candidate block is withheld was never shown the set.
pub fn offered_types(candidates: &[SkillCandidate]) -> Option<Vec<String>> {
    let mut contenders = candidates
        .iter()
        .filter(|c| is_tool_bearing_contender(c))
        .peekable();
    contenders.peek()?;
    if contenders.any(|c| !c.schemas_linked) {
        return None;
    }

    let offered = type_ids(candidates.iter().filter(|c| clears_score_gate(c)));
    // Linked metadata that decodes to no type would be an `enum` nothing
    // satisfies: every call refused, with no id to name in the refusal.
    (!offered.is_empty()).then_some(offered)
}

/// Put `offered` (see [`offered_types`]) on the existing-type parameter of
/// every tool in `tools` that has one, as an `enum`.
pub fn hold_to_offered_types(
    tools: Vec<ToolDefinition>,
    offered: &[String],
) -> Vec<ToolDefinition> {
    tools
        .into_iter()
        .map(|tool| super::tools::with_offered_types(tool, offered))
        .collect()
}

/// The full tool surface, minus tools whose required parameters depend on the
/// `EXISTING SCHEMAS` block. What [`stage2_tools`] falls back to wherever
/// [`stage2_scoped_tools`] cannot scope, so no fail-open case can drift.
///
/// The exclusion is an *eligibility* judgement, not a claim that the block is
/// absent. Workspace context can render it independently of routing (see
/// [`tools_with_available_guidance`]), so on this path the block may well be
/// in the prompt. But fail-open means retrieval matched nothing: no skill
/// vouched for this turn, and ADR-038 puts the trust boundary at what
/// retrieval surfaced. Handing over a tool whose whole purpose is resolving
/// an ambiguous reference — when the system could not even identify which
/// capability the request needs — widens the surface at exactly the moment
/// there is least reason to trust it. Every other tool remains available.
pub fn fail_open_surface(all: &[ToolDefinition]) -> Vec<ToolDefinition> {
    all.iter()
        .filter(|t| !super::tools::requires_routed_guidance_tool(&t.name))
        .cloned()
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn candidate(name: &str, score: f32, tools: &[&str]) -> SkillCandidate {
        SkillCandidate {
            id: format!("skill-{name}"),
            name: name.to_string(),
            use_for: format!("{name} description"),
            score,
            tools: tools.iter().map(|t| t.to_string()).collect(),
            instructions: format!("{name} instructions"),
            schema_metadata: json!([]),
            schemas_linked: false,
            role: Default::default(),
            pinned: false,
        }
    }

    #[test]
    fn a_lookup_keeps_the_read_only_candidates_in_their_order() {
        let kept = read_only_candidates(vec![
            candidate("Node Deletion", 0.99, &["search_nodes", "delete_node"]),
            candidate("Research & Search", 0.97, &["search_nodes", "get_node"]),
            candidate("Implementing a Task", 0.96, &["update_task_status"]),
            candidate("Play Workflow State", 0.95, &["get_workflow_state"]),
        ]);
        let names: Vec<&str> = kept.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(names, ["Research & Search", "Play Workflow State"]);
    }

    #[test]
    fn a_lookup_with_no_read_only_candidate_keeps_what_was_retrieved() {
        let retrieved = vec![
            candidate("Node Deletion", 0.99, &["delete_node"]),
            candidate("Organization", 0.97, &["create_relationship"]),
        ];
        let kept = read_only_candidates(retrieved.clone());
        assert_eq!(kept.len(), retrieved.len());
    }

    #[test]
    fn a_read_only_candidate_below_the_score_gate_does_not_replace_the_retrieved_set() {
        let retrieved = vec![
            candidate("Node Deletion", 0.99, &["delete_node"]),
            candidate("Research & Search", 0.01, &["search_nodes"]),
        ];
        assert!(!clears_score_gate(&retrieved[1]));
        let kept = read_only_candidates(retrieved);
        assert_eq!(
            kept.len(),
            2,
            "narrowing must not leave a procedure the gate rejects"
        );
    }

    #[test]
    fn a_query_that_opens_with_an_adding_verb_is_add_shaped() {
        for query in [
            "add cache invalidation decision to architecture decisions",
            "Add the cache invalidation decision to our architecture decisions.",
            "  create a task for the removal of the legacy auth module",
            "log a bug about the merge conflict in the sync engine",
            "record the decision to drop the v1 API",
            "file a bug: the conflict journal shows stale entries",
        ] {
            assert!(is_add_shaped(query), "{query:?}");
        }
    }

    #[test]
    fn a_query_that_removes_changes_or_looks_up_is_not_add_shaped() {
        for query in [
            "delete the cache invalidation decision",
            "remove all the resolved incidents",
            "get rid of that old meeting note",
            "merge the two Sarah Chen records",
            "mark the incident as resolved",
            "put the login timeout bug on the release blockers list",
            "make the launch task high priority",
            "start tracking planning cycles",
            "search stored knowledge for the merge gate",
            // The verb has to be the whole first word.
            "address the review comments",
            "logging is too noisy",
            "",
        ] {
            assert!(!is_add_shaped(query), "{query:?}");
        }
    }

    /// A retrieval double: the rankings to return, by query, and a record of
    /// every search asked for.
    struct Searches {
        rankings: Vec<(String, Vec<SkillCandidate>)>,
        asked: std::sync::Mutex<Vec<(String, usize)>>,
    }

    impl Searches {
        fn new(rankings: Vec<(&str, Vec<SkillCandidate>)>) -> Self {
            Self {
                rankings: rankings
                    .into_iter()
                    .map(|(q, r)| (q.to_string(), r))
                    .collect(),
                asked: std::sync::Mutex::new(Vec::new()),
            }
        }

        async fn run(&self, query: &str) -> Result<Vec<SkillCandidate>, String> {
            retrieve_candidates(query, |q, limit| async move {
                self.asked.lock().unwrap().push((q.clone(), limit));
                let ranking = self
                    .rankings
                    .iter()
                    .find(|(known, _)| *known == q)
                    .map(|(_, r)| r.clone())
                    .ok_or_else(|| format!("no ranking for {q:?}"))?;
                Ok(ranking.into_iter().take(limit).collect())
            })
            .await
        }

        fn asked(&self) -> Vec<(String, usize)> {
            self.asked.lock().unwrap().clone()
        }
    }

    const ADD: &str = "add cache invalidation decision to architecture decisions";

    /// The ranking measured for [`ADD`] on the locked embedding model.
    fn collision_ranking() -> Vec<SkillCandidate> {
        vec![
            candidate("Node Deletion", 0.816, &["delete_node", "search_nodes"]),
            candidate("Play Workflow State", 0.801, &["get_workflow_state"]),
            candidate("Node Merge", 0.787, &["merge_conflict", "get_conflict"]),
            candidate("Bulk Import", 0.768, &["create_nodes_from_markdown"]),
            candidate(
                "Conflict Journal",
                0.767,
                &["list_conflicts", "dismiss_conflict"],
            ),
            candidate("Node Creation", 0.763, &["create_node", "update_node"]),
        ]
    }

    #[tokio::test]
    async fn a_query_that_is_not_an_add_is_one_search_returned_as_it_came() {
        let query = "delete the cache invalidation decision";
        let searches = Searches::new(vec![(query, collision_ranking())]);
        let ranked = searches.run(query).await.unwrap();
        assert_eq!(
            names(&ranked),
            [
                "Node Deletion",
                "Play Workflow State",
                "Node Merge",
                "Bulk Import"
            ],
            "a removal keeps the skill that removes, in the lead"
        );
        assert_eq!(searches.asked(), [(query.to_string(), RETRIEVAL_FETCH)]);
    }

    #[tokio::test]
    async fn an_add_is_ranked_without_the_skills_that_remove_user_data() {
        let searches = Searches::new(vec![(ADD, collision_ranking())]);
        let ranked = searches.run(ADD).await.unwrap();
        assert!(
            !ranked.iter().any(skill_is_destructive),
            "ranked: {:?}",
            names(&ranked)
        );
        assert!(
            !stage2_permitted_names(&select_candidates(ranked)).contains("delete_node"),
            "delete_node must not be offered on an add"
        );
    }

    #[tokio::test]
    async fn an_add_asks_for_enough_to_fill_the_ranking_without_them() {
        // Node Creation is sixth: asked for RETRIEVAL_FETCH it is never
        // returned, and the two skills dropped leave two to judge.
        let searches = Searches::new(vec![(ADD, collision_ranking())]);
        let ranked = searches.run(ADD).await.unwrap();
        assert_eq!(searches.asked()[0], (ADD.to_string(), ADD_RETRIEVAL_FETCH));
        assert_eq!(
            names(&ranked),
            [
                "Play Workflow State",
                "Bulk Import",
                "Conflict Journal",
                "Node Creation"
            ]
        );
    }

    #[tokio::test]
    async fn an_add_with_no_creating_skill_among_the_judged_gets_one_from_a_second_search() {
        let second = create_retrieval_query(ADD);
        let searches = Searches::new(vec![
            (ADD, collision_ranking()),
            (
                &second,
                vec![
                    candidate("Schema Creation", 0.921, &["create_schema"]),
                    candidate("Node Creation", 0.908, &["create_node", "update_node"]),
                    candidate("Graph Editing", 0.881, &["update_node", "create_node"]),
                ],
            ),
        ]);
        let judged = select_candidates(searches.run(ADD).await.unwrap());

        // The best match that can create, not the best match: Schema Creation
        // leads the second search and holds no create_node.
        assert_eq!(judged[0].name, "Node Creation");
        assert_eq!(
            judged[0].score, 0.908,
            "it keeps the score its search gave it"
        );
        assert_eq!(
            judged.iter().filter(|c| c.name == "Node Creation").count(),
            1,
            "a skill already ranked below the bound is moved, not repeated"
        );
        // Everyone else keeps their score, and nobody new arrives with it.
        // Four are judged: Play Workflow State is a read-only skill that no
        // longer leads, so the next one that can write is kept too.
        assert_eq!(
            names(&judged),
            [
                "Node Creation",
                "Play Workflow State",
                "Bulk Import",
                "Conflict Journal"
            ]
        );
        assert_eq!(judged[1].score, 0.801);
        assert!(stage2_permitted_names(&judged).contains("create_node"));
        assert_eq!(
            searches.asked(),
            [
                (ADD.to_string(), ADD_RETRIEVAL_FETCH),
                (second, ADD_RETRIEVAL_FETCH)
            ]
        );
    }

    #[tokio::test]
    async fn the_second_search_does_not_bring_back_a_skill_that_removes() {
        // A skill that can create and delete leads the second search. It was
        // dropped from the first ranking, and must not return through the
        // second with a score that leads the turn.
        let second = create_retrieval_query(ADD);
        let searches = Searches::new(vec![
            (ADD, collision_ranking()),
            (
                &second,
                vec![
                    candidate("Housekeeping", 0.95, &["create_node", "delete_node"]),
                    candidate("Node Creation", 0.908, &["create_node"]),
                ],
            ),
        ]);
        let judged = select_candidates(searches.run(ADD).await.unwrap());
        assert_eq!(judged[0].name, "Node Creation");
        assert!(
            !judged.iter().any(skill_is_destructive),
            "judged: {:?}",
            names(&judged)
        );
        assert!(!stage2_permitted_names(&judged).contains("delete_node"));
    }

    /// The cost of ranking the added skill on the second query's scale, in a
    /// compound request: it can outrank the deletion skill the other half
    /// retrieved, and a removing tool is offered only from the leader.
    #[tokio::test]
    async fn in_a_compound_request_the_added_skill_can_outrank_the_other_halfs_deletion() {
        let removal = "delete the old draft";
        let second = create_retrieval_query(ADD);
        let searches = Searches::new(vec![
            (ADD, collision_ranking()),
            (
                &second,
                vec![candidate("Node Creation", 0.908, &["create_node"])],
            ),
            (
                removal,
                vec![candidate("Node Deletion", 0.86, &["delete_node"])],
            ),
        ]);
        // The merge `agent_loop`'s `route` makes of a compound request's
        // queries: every ranking, deduplicated, by score.
        let mut merged = searches.run(ADD).await.unwrap();
        merged.extend(searches.run(removal).await.unwrap());
        merged.sort_by(|a, b| b.score.total_cmp(&a.score));
        let judged = select_candidates(merged);

        assert_eq!(names(&judged)[..2], ["Node Creation", "Node Deletion"]);
        let offered = stage2_permitted_names(&judged);
        assert!(offered.contains("create_node"));
        assert!(
            !offered.contains("delete_node"),
            "the deletion skill is a candidate and does not lead, so its tool is withheld"
        );
        assert_eq!(destructive_tools_withheld(&judged), ["delete_node"]);
    }

    #[tokio::test]
    async fn an_add_that_already_reaches_a_creating_skill_is_not_searched_twice() {
        let query = "add a spec for the CSV import pipeline";
        let searches = Searches::new(vec![(
            query,
            vec![
                candidate("Bulk Import", 0.875, &["create_nodes_from_markdown"]),
                candidate("Node Creation", 0.791, &["create_node"]),
                candidate("Schema Creation", 0.780, &["create_schema"]),
            ],
        )]);
        let ranked = searches.run(query).await.unwrap();
        assert_eq!(
            names(&ranked),
            ["Bulk Import", "Node Creation", "Schema Creation"]
        );
        assert_eq!(searches.asked().len(), 1);
    }

    #[tokio::test]
    async fn a_second_search_that_fails_leaves_the_first_ranking() {
        // No ranking is registered for the second query, so that search errs.
        let searches = Searches::new(vec![(ADD, collision_ranking())]);
        let ranked = searches.run(ADD).await.unwrap();
        assert_eq!(searches.asked().len(), 2);
        assert_eq!(
            names(&ranked),
            [
                "Play Workflow State",
                "Bulk Import",
                "Conflict Journal",
                "Node Creation"
            ]
        );
    }

    #[tokio::test]
    async fn a_first_search_that_fails_is_the_callers_error() {
        let searches = Searches::new(vec![]);
        assert!(searches.run(ADD).await.is_err());
    }

    #[test]
    fn a_query_that_sets_up_or_starts_tracking_a_kind_of_thing_is_new_kind_shaped() {
        for query in [
            "set up Postmortems with a severity and review date",
            "Set up Runbooks with a service and a last reviewed date.",
            "set up Retrospectives with an owner and a follow-up date",
            "keep track of incident postmortems with severity and review date",
            "keep track of decisions behind each feature",
            "start tracking planning cycles",
            "begin tracking design decisions",
            "  start keeping track of production incidents",
            "track planning cycles",
        ] {
            assert!(is_new_kind_shaped(query), "{query:?}");
        }
    }

    #[test]
    fn a_query_about_one_record_or_a_view_is_not_new_kind_shaped() {
        for query in [
            // A tracking phrase, then a word that points at one thing.
            "start tracking the login timeout bug",
            "track this task's progress",
            "start tracking the offline sync spec's sign-off",
            "keep track of the Q4 cycle's status",
            "keep track of it",
            "track an expense",
            "start tracking a bug in the importer",
            // A possessive, a quantifier, a clause, and a lookup idiom.
            "keep track of my dentist appointment on Friday",
            "start tracking our planning cycles",
            "start tracking Priya's onboarding",
            "track Acme\u{2019}s renewal",
            "track one expense",
            "track each open ticket",
            "keep track of when the Q4 cycle ends",
            "track whether the deploy finished",
            "keep track of what we decided about caching",
            "start keeping track of who owes me money",
            "track down the notes on the auth redesign",
            // "Set up" with an article, or with no details named.
            "set up a meeting with Priya on Friday",
            "set up a board of tickets by status",
            "set up the staging database with a seed script",
            "set up Postmortems",
            "set up our retro with an agenda",
            // The phrase has to open the query, whole.
            "we should start tracking production incidents",
            "tracking is broken on the dashboard",
            "tracker for the venues I book",
            "add a task to keep track of renewals",
            "track",
            "",
        ] {
            assert!(!is_new_kind_shaped(query), "{query:?}");
        }
    }

    const NEW_KIND: &str = "set up Deletion requests with a requester and a due date";

    /// The ranking measured for [`NEW_KIND`] on the locked embedding model.
    fn new_kind_collision_ranking() -> Vec<SkillCandidate> {
        vec![
            candidate("Node Deletion", 0.920, &["delete_node", "search_nodes"]),
            candidate("Schema Creation", 0.853, &["create_schema"]),
            candidate("Node Creation", 0.831, &["create_node", "update_node"]),
            candidate("Play Authoring", 0.814, &["update_play"]),
        ]
    }

    #[tokio::test]
    async fn a_new_kind_another_skill_leads_gets_a_type_skill_from_a_second_search() {
        let second = new_kind_retrieval_query(NEW_KIND);
        let searches = Searches::new(vec![
            (NEW_KIND, new_kind_collision_ranking()),
            (
                &second,
                vec![
                    candidate("Schema Creation", 0.993, &["create_schema"]),
                    candidate("Node Creation", 0.914, &["create_node", "update_node"]),
                ],
            ),
        ]);
        let judged = select_candidates(searches.run(NEW_KIND).await.unwrap());

        assert_eq!(
            names(&judged),
            ["Schema Creation", "Node Deletion", "Node Creation"],
            "the type skill is moved to the lead, not repeated"
        );
        assert_eq!(
            judged[0].score, 0.993,
            "it has the score its search gave it"
        );
        assert_eq!(judged[2].score, 0.831, "and nobody else's score moved");
        let offered = stage2_permitted_names(&judged);
        assert!(offered.contains("create_schema"));
        assert!(
            !offered.contains("delete_node"),
            "the deletion skill no longer leads, so its tool is withheld"
        );
        assert_eq!(
            searches.asked(),
            [
                (NEW_KIND.to_string(), RETRIEVAL_FETCH),
                (second, ADD_RETRIEVAL_FETCH)
            ]
        );
    }

    #[tokio::test]
    async fn a_new_kind_a_type_skill_already_leads_is_not_searched_twice() {
        let query = "start tracking planning cycles";
        let searches = Searches::new(vec![(
            query,
            vec![
                candidate("Schema Creation", 0.872, &["create_schema"]),
                candidate("Graph Editing", 0.828, &["update_node", "create_node"]),
            ],
        )]);
        let ranked = searches.run(query).await.unwrap();
        assert_eq!(names(&ranked), ["Schema Creation", "Graph Editing"]);
        assert_eq!(searches.asked().len(), 1);
    }

    #[tokio::test]
    async fn a_type_skill_that_places_without_leading_is_searched_for_again() {
        // Third place offers `create_schema`, and still leaves the turn routed
        // to the record skill with that skill's fields on the write tools.
        let query = "keep track of merge requests with a reviewer and a status";
        let second = new_kind_retrieval_query(query);
        let searches = Searches::new(vec![
            (
                query,
                vec![
                    candidate("Graph Editing", 0.906, &["update_node", "create_node"]),
                    candidate("Conflict Journal", 0.879, &["list_conflicts"]),
                    candidate("Schema Creation", 0.866, &["create_schema"]),
                ],
            ),
            (
                &second,
                vec![candidate("Schema Creation", 1.007, &["create_schema"])],
            ),
        ]);
        let ranked = searches.run(query).await.unwrap();
        assert_eq!(
            names(&ranked),
            ["Schema Creation", "Graph Editing", "Conflict Journal"]
        );
    }

    #[tokio::test]
    async fn a_new_kind_whose_second_search_fails_or_finds_no_type_skill_keeps_its_ranking() {
        // No ranking is registered for the second query, so that search errs.
        let failing = Searches::new(vec![(NEW_KIND, new_kind_collision_ranking())]);
        let ranked = failing.run(NEW_KIND).await.unwrap();
        assert_eq!(failing.asked().len(), 2);
        assert_eq!(ranked[0].name, "Node Deletion");

        let second = new_kind_retrieval_query(NEW_KIND);
        let without = Searches::new(vec![
            (NEW_KIND, new_kind_collision_ranking()),
            (
                &second,
                vec![
                    candidate("Node Creation", 0.95, &["create_node"]),
                    // Below the mutating bar: not a skill the turn may use.
                    candidate("Schema Creation", 0.2, &["create_schema"]),
                ],
            ),
        ]);
        let ranked = without.run(NEW_KIND).await.unwrap();
        assert_eq!(
            names(&ranked),
            [
                "Node Deletion",
                "Schema Creation",
                "Node Creation",
                "Play Authoring"
            ]
        );
        assert_eq!(ranked[1].score, 0.853);
    }

    #[tokio::test]
    async fn a_request_about_one_record_that_opens_like_a_new_kind_is_one_search() {
        let query = "keep track of the Q4 cycle's status";
        let searches = Searches::new(vec![(
            query,
            vec![
                candidate("Graph Editing", 0.870, &["update_node", "create_node"]),
                candidate("Schema Creation", 0.784, &["create_schema"]),
            ],
        )]);
        let ranked = searches.run(query).await.unwrap();
        assert_eq!(names(&ranked), ["Graph Editing", "Schema Creation"]);
        assert_eq!(searches.asked().len(), 1);
    }

    #[test]
    fn a_creating_skill_below_its_score_bar_does_not_count_as_reached() {
        let weak = candidate("Node Creation", 0.2, &["create_node"]);
        assert!(lacks_a_creating_skill(&[weak]));
        let strong = candidate("Node Creation", 0.8, &["create_node"]);
        assert!(!lacks_a_creating_skill(&[strong]));
    }

    fn tool(name: &str) -> ToolDefinition {
        ToolDefinition {
            name: name.to_string(),
            description: String::new(),
            parameters_schema: json!({}),
        }
    }

    #[test]
    fn stage1_offers_exactly_the_four_routing_tools() {
        let defs = stage1_tool_definitions();
        let names: Vec<&str> = defs.iter().map(|d| d.name.as_str()).collect();
        assert_eq!(
            names,
            vec![
                ROUTE_QUERY_TOOL,
                ROUTE_LOOKUP_TOOL,
                ROUTE_CLARIFY_TOOL,
                ROUTE_MULTI_TOOL
            ]
        );
    }

    #[test]
    fn route_query_parses_into_a_query_decision() {
        let d = parse_route_decision(ROUTE_QUERY_TOOL, r#"{"query":"create a schema"}"#);
        assert_eq!(d, Some(RouteDecision::Query("create a schema".into())));
    }

    #[test]
    fn route_clarify_parses_question_and_options() {
        let d = parse_route_decision(
            ROUTE_CLARIFY_TOOL,
            r#"{"question":"Which did you mean?","options":["Track debts","Search notes"]}"#,
        );
        assert_eq!(
            d,
            Some(RouteDecision::Clarify {
                question: "Which did you mean?".into(),
                options: vec!["Track debts".into(), "Search notes".into()],
            })
        );
    }

    #[test]
    fn clarify_without_options_still_parses() {
        // `options` is optional on the wire so a model omitting it does not
        // cost the turn; the contract's "be specific" requirement is enforced
        // by the tool description, not by rejecting the call.
        let d = parse_route_decision(ROUTE_CLARIFY_TOOL, r#"{"question":"Which one?"}"#);
        assert!(matches!(d, Some(RouteDecision::Clarify { .. })));
    }

    #[test]
    fn unknown_tool_or_blank_query_yields_no_decision() {
        assert!(parse_route_decision("search_nodes", r#"{"query":"x"}"#).is_none());
        assert!(parse_route_decision(ROUTE_QUERY_TOOL, r#"{"query":"   "}"#).is_none());
        assert!(parse_route_decision(ROUTE_QUERY_TOOL, "not json").is_none());
    }

    #[test]
    fn route_multi_parses_two_or_more_queries() {
        let d = parse_route_decision(
            ROUTE_MULTI_TOOL,
            r#"{"queries":["log a $42 lunch expense","remind me to follow up with Sarah Friday"]}"#,
        );
        assert_eq!(
            d,
            Some(RouteDecision::Multi(vec![
                "log a $42 lunch expense".into(),
                "remind me to follow up with Sarah Friday".into(),
            ]))
        );
    }

    #[test]
    fn route_multi_with_fewer_than_two_usable_queries_yields_no_decision() {
        // A single-element array is not a genuine compound intent — the model
        // over-called route_multi for what should have been route_query.
        // Falling through to "no decision" (unrouted retrieval on the raw
        // message) beats trusting a one-element "multi".
        assert!(parse_route_decision(ROUTE_MULTI_TOOL, r#"{"queries":["one thing"]}"#).is_none());
        // Blank entries don't count toward the two-or-more requirement.
        assert!(
            parse_route_decision(ROUTE_MULTI_TOOL, r#"{"queries":["one thing","   "]}"#).is_none()
        );
        assert!(parse_route_decision(ROUTE_MULTI_TOOL, r#"{"queries":[]}"#).is_none());
    }

    #[test]
    fn a_rejected_route_multi_is_tagged_apart_from_no_decision() {
        // The over-called route_multi above yields no decision, but the log tag
        // must still say route_multi was attempted — otherwise the routing eval
        // scores a single intent split into "multi" as a clean turn.
        assert_eq!(undecided_routing_tag([ROUTE_MULTI_TOOL]), "multi_rejected");
        assert_eq!(
            undecided_routing_tag(["search_nodes", ROUTE_MULTI_TOOL]),
            "multi_rejected"
        );
        assert_eq!(undecided_routing_tag([ROUTE_QUERY_TOOL]), "none");
        assert_eq!(undecided_routing_tag(std::iter::empty()), "none");
    }

    #[test]
    fn route_multi_trims_and_drops_blank_entries_among_valid_ones() {
        let d = parse_route_decision(
            ROUTE_MULTI_TOOL,
            r#"{"queries":["  log expense  ","","  set reminder  "]}"#,
        );
        assert_eq!(
            d,
            Some(RouteDecision::Multi(vec![
                "log expense".into(),
                "set reminder".into(),
            ]))
        );
    }

    #[test]
    fn blast_radius_derives_from_the_tool_whitelist() {
        assert!(!skill_is_mutating(&candidate(
            "research",
            0.5,
            &["search_nodes", "get_node"]
        )));
        assert!(skill_is_mutating(&candidate(
            "schema",
            0.5,
            &["create_schema", "get_node"]
        )));
        // A single write tool among reads is enough to raise the bar.
        assert!(skill_is_mutating(&candidate(
            "deletion",
            0.5,
            &["search_nodes", "delete_node"]
        )));
    }

    #[test]
    fn an_unrecognised_whitelist_tool_is_treated_as_mutating() {
        // Fail-safe: a typo, a renamed tool, or a future externally-registered
        // one is of unknown blast radius, so it takes the higher bar rather
        // than defaulting to read-only.
        let ghost = candidate("ghost", 0.2, &["delete_everything_v2"]);
        assert!(skill_is_mutating(&ghost));
        assert!(
            !clears_score_gate(&ghost),
            "0.2 clears the read bar but must not clear the mutating one"
        );
    }

    #[test]
    fn mutating_skills_carry_a_strictly_higher_bar() {
        // Compile-time: ADR-038 requires the expensive error — firing the
        // wrong *mutating* tool — to be gated harder than a wrong read. Tuning
        // the values is expected; inverting the ordering is not, so it fails
        // the build rather than a test run.
        const _: () = assert!(MUTATING_SKILL_SCORE_BAR > READ_SKILL_SCORE_BAR);
        let read = candidate("research", 0.2, &["search_nodes"]);
        let write = candidate("schema", 0.2, &["create_schema"]);
        // Identical score, different verdict — the bar is a property of the
        // matched skill, not a single global constant.
        assert!(clears_score_gate(&read));
        assert!(!clears_score_gate(&write));
    }

    #[test]
    fn destructive_skills_carry_a_strictly_higher_bar_than_other_mutations() {
        // Compile-time, matching the read/mutating assertion above: the values
        // are expected to be tuned, the ladder's ordering is not.
        const _: () = assert!(DESTRUCTIVE_SKILL_SCORE_BAR > MUTATING_SKILL_SCORE_BAR);

        // Identical score, three different verdicts — one rung per blast radius.
        let read = candidate("research", 0.35, &["search_nodes"]);
        let mutating = candidate("editing", 0.35, &["update_node"]);
        let destructive = candidate("deletion", 0.35, &["delete_node"]);
        assert!(clears_score_gate(&read));
        assert!(clears_score_gate(&mutating));
        assert!(
            !clears_score_gate(&destructive),
            "a skill that can irreversibly remove user data must clear a higher bar than one \
             that merely writes"
        );
    }

    /// The invariant itself, asserted on the helper that owns it rather than
    /// only through its two consumers.
    ///
    /// `stage2_permitted_names` and `declare_write_tool_fields` each pin the
    /// tool-less case against their own user-visible outcome, which is the
    /// right test for them but leaves the rule specified only as a property
    /// of two callers. The rule is meant to be inheritable — a third consumer
    /// gets it by calling this — so it is pinned here directly.
    ///
    /// The below-bar case is the one no consumer test reaches: both use
    /// gate-clearing candidates throughout, so nothing else exercises the
    /// `clears_score_gate` half of the conjunction rejecting a candidate that
    /// *does* bear tools.
    #[test]
    fn top_tool_bearing_score_ignores_tool_less_and_below_bar_candidates() {
        // A tool-less candidate scoring above everything must not raise the
        // result: the winner is the top *tool-bearing* candidate, not the top
        // candidate.
        let schema_hit = candidate("Meeting Note", 1.0, &[]);
        let skill = candidate("Node Creation", 0.7, &["create_node"]);
        let score = top_tool_bearing_score([&schema_hit, &skill].into_iter());
        assert_eq!(
            score, 0.7,
            "a candidate whitelisting no tool has no stake in which tool-bearing candidate wins"
        );

        // A tool-bearing candidate *below its own bar* is equally out of the
        // running — it can never be offered, so it cannot set the bar for one
        // that can. 0.20 clears the read bar but not the mutating bar its
        // `create_node` whitelist earns it.
        let below_bar = candidate("Weak Creation", 0.20, &["create_node"]);
        assert!(!clears_score_gate(&below_bar));
        let reader = candidate("Research", 0.18, &["search_nodes"]);
        let score = top_tool_bearing_score([&below_bar, &reader].into_iter());
        assert_eq!(
            score, 0.18,
            "a tool-bearing candidate that cannot clear its own bar must not raise the score a \
             gate-clearing candidate is measured against"
        );

        // No tool-bearing candidate at all folds to the seed. `NEG_INFINITY`
        // is what makes the empty set impose no ceiling rather than silently
        // excluding everyone: any real score clears each consumer's
        // `score >= max` comparison against it.
        let other = candidate("Sprint", 0.9, &[]);
        assert_eq!(
            top_tool_bearing_score([&schema_hit, &other].into_iter()),
            f32::NEG_INFINITY
        );
    }

    #[test]
    fn a_destructive_skill_that_only_places_cannot_put_delete_node_in_reach() {
        // The #2240 regression. `Node Deletion` was retrieved as a candidate on
        // turns that recorded a decision, marked a status, and set a due date.
        // Because the surface was the union across every eligible candidate, it
        // contributed `delete_node` merely by placing — while the tool the turn
        // actually needed was absent unless some other candidate happened to
        // whitelist it.
        let all = vec![
            tool("create_node"),
            tool("search_nodes"),
            tool("delete_node"),
        ];
        let cands = vec![
            candidate("Node Creation", 0.8, &["create_node", "search_nodes"]),
            candidate("Node Deletion", 0.5, &["delete_node", "search_nodes"]),
        ];
        let scoped = stage2_tools(&cands, &all);
        let names: Vec<&str> = scoped.iter().map(|t| t.name.as_str()).collect();

        assert!(
            names.contains(&"create_node"),
            "the winning skill's own tool must still be offered: {names:?}"
        );
        assert!(
            !names.contains(&"delete_node"),
            "a destructive skill that placed but did not win retrieval must not put delete_node \
             in reach: {names:?}"
        );
        // The runner-up's non-destructive tools are unaffected — this narrows
        // destructive admission only, it does not drop losing candidates.
        assert!(names.contains(&"search_nodes"));
    }

    #[test]
    fn a_destructive_skill_that_wins_retrieval_still_offers_delete_node() {
        // The regression the change could plausibly cause. Deletion must keep
        // working for requests that actually ask for it.
        let all = vec![tool("create_node"), tool("delete_node"), tool("get_node")];
        let cands = vec![
            candidate("Node Deletion", 0.7, &["delete_node", "get_node"]),
            candidate("Node Creation", 0.5, &["create_node"]),
        ];
        let scoped = stage2_tools(&cands, &all);
        let names: Vec<&str> = scoped.iter().map(|t| t.name.as_str()).collect();
        assert!(
            names.contains(&"delete_node"),
            "a genuine deletion match must still be able to delete: {names:?}"
        );
    }

    #[test]
    fn a_zero_tool_candidate_cannot_withhold_the_winning_skill_s_destructive_tool() {
        // Retrieval returns schema-kind candidates (`tools: []`) in the same
        // list this consumes, and the lexical backstop that recovers a schema
        // named outright in the query pins it above a genuinely-matching
        // skill's cosine score. Scoring highest must not make it the "winner"
        // for a decision it has no stake in — it owns no tools at all.
        let all = vec![tool("delete_node"), tool("search_nodes")];
        let cands = vec![
            candidate("Meeting Note", 0.95, &[]),
            candidate("Node Deletion", 0.7, &["delete_node", "search_nodes"]),
        ];
        let scoped = stage2_tools(&cands, &all);
        let names: Vec<&str> = scoped.iter().map(|t| t.name.as_str()).collect();
        assert!(
            names.contains(&"delete_node"),
            "a skill that won retrieval among tool-bearing candidates must keep its destructive \
             tool regardless of a co-occurring zero-tool candidate's score: {names:?}"
        );
        // And the log agrees nothing was withheld — the two read the same rule.
        assert!(destructive_tools_withheld(&cands).is_empty());
    }

    #[test]
    fn destructive_admission_does_not_depend_on_candidate_order() {
        // `stage2_tools` picks the winner by explicit max, so a caller that
        // stopped sorting by score cannot silently widen the destructive
        // surface.
        let all = vec![tool("create_node"), tool("delete_node")];
        let ascending = vec![
            candidate("Node Deletion", 0.5, &["delete_node"]),
            candidate("Node Creation", 0.8, &["create_node"]),
        ];
        let descending = vec![
            candidate("Node Creation", 0.8, &["create_node"]),
            candidate("Node Deletion", 0.5, &["delete_node"]),
        ];
        let names = |c: &[SkillCandidate]| -> Vec<String> {
            let mut n: Vec<String> = stage2_tools(c, &all)
                .iter()
                .map(|t| t.name.clone())
                .collect();
            n.sort();
            n
        };
        assert_eq!(names(&ascending), names(&descending));
        assert!(!names(&ascending).contains(&"delete_node".to_string()));
    }

    #[test]
    fn a_lone_destructive_candidate_is_never_left_with_an_empty_surface() {
        // It wins by default (it is the only eligible candidate), so nothing is
        // withheld and the existing fail-open branches stay untouched.
        let all = vec![tool("delete_node"), tool("create_node")];
        let cands = vec![candidate("Node Deletion", 0.9, &["delete_node"])];
        let scoped = stage2_tools(&cands, &all);
        let names: Vec<&str> = scoped.iter().map(|t| t.name.as_str()).collect();
        assert_eq!(names, vec!["delete_node"]);
    }

    #[test]
    fn a_destructive_skill_below_its_bar_is_ineligible_everywhere() {
        // `clears_score_gate` is consulted independently by three sites, so the
        // rank-1 half of the fix has to hold at each of them rather than only
        // at the one that scopes tools.
        let all = vec![tool("delete_node"), tool("search_nodes")];
        // Above MUTATING (0.30), below DESTRUCTIVE — the band the rank-1
        // `Node Deletion` turns landed in.
        let cands = vec![candidate("Node Deletion", 0.35, &["delete_node"])];

        assert_eq!(
            routed_skill_names(&cands),
            "",
            "must not be logged as routed"
        );
        assert!(
            render_candidates_for_prompt(&cands).is_none(),
            "must not be rendered into the Stage-2 prompt"
        );
        // No eligible candidate at all -> the existing fail-open branch, not a
        // scoped surface built from an ineligible skill.
        let scoped = stage2_tools(&cands, &all);
        let names: Vec<&str> = scoped.iter().map(|t| t.name.as_str()).collect();
        assert!(names.contains(&"search_nodes"));
    }

    #[test]
    fn an_unrecognised_tool_name_is_mutating_but_not_destructive() {
        // Deliberate asymmetry — see `removes_user_data_tool`. An unknown name
        // must not attract the strictest bar in the system, and a later
        // "consistency" cleanup that flips it would quietly raise the bar on
        // every skill with a typo'd whitelist.
        let unknown = candidate("plugin", 0.9, &["some_external_tool"]);
        assert!(skill_is_mutating(&unknown));
        assert!(!skill_is_destructive(&unknown));
        assert_eq!(score_bar_for(&unknown), MUTATING_SKILL_SCORE_BAR);
    }

    #[test]
    fn destructive_candidates_rejected_by_the_bar_are_reported_for_the_log() {
        // The band between the mutating and destructive bars — where a real
        // deletion request would be silently dropped if the bar is too high.
        let cands = vec![
            candidate("Node Deletion", 0.35, &["delete_node"]),
            candidate("Node Creation", 0.9, &["create_node"]),
        ];
        let rejected = destructive_candidates_below_bar(&cands);
        assert_eq!(rejected.len(), 1);
        assert_eq!(rejected[0].0, "Node Deletion");

        // A deletion match that clears the bar is not "rejected" — it is
        // simply routed, and must not show up as evidence the bar is wrong.
        let ok = vec![candidate("Node Deletion", 0.9, &["delete_node"])];
        assert!(destructive_candidates_below_bar(&ok).is_empty());

        // Nor does a non-destructive skill below its own (lower) bar.
        let unrelated = vec![candidate("Research & Search", 0.01, &["search_nodes"])];
        assert!(destructive_candidates_below_bar(&unrelated).is_empty());
    }

    #[test]
    fn withheld_destructive_tools_are_reported_for_the_log() {
        let placed = vec![
            candidate("Node Creation", 0.8, &["create_node"]),
            candidate("Node Deletion", 0.5, &["delete_node"]),
        ];
        assert_eq!(destructive_tools_withheld(&placed), vec!["delete_node"]);

        // Nothing withheld when the destructive skill wins, and nothing
        // withheld on an ordinary turn — this must stay quiet in the log.
        let won = vec![
            candidate("Node Deletion", 0.8, &["delete_node"]),
            candidate("Node Creation", 0.5, &["create_node"]),
        ];
        assert!(destructive_tools_withheld(&won).is_empty());
        let ordinary = vec![candidate("Node Creation", 0.8, &["create_node"])];
        assert!(destructive_tools_withheld(&ordinary).is_empty());
    }

    #[test]
    fn candidates_below_their_bar_are_never_rendered() {
        let cands = vec![
            candidate("weak", 0.01, &["search_nodes"]),
            candidate("strong", 0.9, &["search_nodes"]),
        ];
        let rendered = render_candidates_for_prompt(&cands).expect("one candidate is eligible");
        assert!(rendered.contains("strong"));
        assert!(!rendered.contains("weak instructions"));
    }

    #[test]
    fn routed_skill_names_lists_only_gate_clearing_candidates() {
        let cands = vec![
            candidate("Research & Search", 0.9, &["search_nodes"]),
            candidate("Below The Bar", 0.01, &["search_nodes"]),
            candidate("Node Creation", 0.9, &["search_nodes"]),
        ];
        assert_eq!(
            routed_skill_names(&cands),
            "Research & Search, Node Creation"
        );
    }

    #[test]
    fn routed_skill_names_is_empty_when_nothing_clears_the_gate() {
        // Distinct from "retrieval returned nothing" only in the candidate
        // count logged alongside it; the scraper omits the marker either way.
        let cands = vec![candidate("weak", 0.001, &["create_schema"])];
        assert_eq!(routed_skill_names(&cands), "");
    }

    #[test]
    fn routed_skill_names_matches_what_is_rendered_into_the_prompt() {
        // The field exists to answer "which skill was this turn routed to", so
        // it has to name the same set the model actually sees. Asserting the
        // two agree keeps the log honest if either filter later changes.
        let cands = vec![
            candidate("Graph Editing", 0.9, &["search_nodes"]),
            candidate("Excluded", 0.001, &["search_nodes"]),
        ];
        let rendered = render_candidates_for_prompt(&cands).expect("one is eligible");
        for name in routed_skill_names(&cands).split(", ") {
            assert!(
                rendered.contains(name),
                "{name} logged as routed but absent from the prompt block"
            );
        }
        assert!(!rendered.contains("Excluded"));
    }

    #[test]
    fn no_eligible_candidates_renders_nothing() {
        let cands = vec![candidate("weak", 0.001, &["create_schema"])];
        assert!(render_candidates_for_prompt(&cands).is_none());
    }

    fn names(candidates: &[SkillCandidate]) -> Vec<&str> {
        candidates.iter().map(|c| c.name.as_str()).collect()
    }

    #[test]
    fn a_lookup_that_does_not_lead_does_not_cost_a_write_skill_its_place() {
        // The measured shape: the lookup placed second and Schema Creation
        // fell to fourth, taking `create_schema` off the surface.
        let ranked = vec![
            candidate("Graph Editing", 0.891, &["update_node", "search_nodes"]),
            candidate("Research & Search", 0.888, &["search_semantic", "get_node"]),
            candidate("Organization", 0.882, &["create_relationship"]),
            candidate("Schema Creation", 0.859, &["create_schema"]),
            candidate("Node Deletion", 0.840, &["delete_node"]),
        ];
        assert_eq!(
            names(&select_candidates(ranked)),
            vec![
                "Graph Editing",
                "Research & Search",
                "Organization",
                "Schema Creation"
            ],
            "one more is kept, and only one"
        );
    }

    #[test]
    fn a_lookup_that_leads_takes_no_extra_candidate() {
        let ranked = vec![
            candidate("Research & Search", 0.94, &["search_semantic"]),
            candidate("Schema Creation", 0.80, &["create_schema"]),
            candidate("Node Deletion", 0.79, &["delete_node"]),
            candidate("Node Creation", 0.78, &["create_node"]),
        ];
        assert_eq!(select_candidates(ranked).len(), RETRIEVAL_TOP_K);
    }

    #[test]
    fn write_skills_alone_keep_the_usual_bound() {
        let ranked = vec![
            candidate("Node Creation", 0.9, &["create_node"]),
            candidate("Graph Editing", 0.8, &["update_node"]),
            candidate("Schema Creation", 0.7, &["create_schema"]),
            candidate("Organization", 0.6, &["create_relationship"]),
        ];
        assert_eq!(select_candidates(ranked).len(), RETRIEVAL_TOP_K);
    }

    #[test]
    fn a_second_read_only_skill_behind_a_leading_lookup_still_yields_a_place() {
        // The leader takes no extra for itself. The read-only skill behind it
        // did take a write skill's place, so that place is given back.
        let ranked = vec![
            candidate("Research & Search", 0.94, &["search_semantic"]),
            candidate(
                "Play Workflow State",
                0.80,
                &["get_workflow_state", "search_nodes"],
            ),
            candidate("Node Creation", 0.79, &["create_node"]),
            candidate("Schema Creation", 0.78, &["create_schema"]),
        ];
        assert_eq!(
            names(&select_candidates(ranked)),
            vec![
                "Research & Search",
                "Play Workflow State",
                "Node Creation",
                "Schema Creation"
            ]
        );
    }

    #[test]
    fn a_fourth_candidate_below_its_bar_is_not_kept() {
        // It would never be rendered or offered: the extra place is for a
        // skill the turn can actually use.
        let ranked = vec![
            candidate("Graph Editing", 0.9, &["update_node"]),
            candidate("Research & Search", 0.8, &["search_semantic"]),
            candidate("Organization", 0.7, &["create_relationship"]),
            candidate("Schema Creation", 0.2, &["create_schema"]),
        ];
        assert_eq!(select_candidates(ranked).len(), RETRIEVAL_TOP_K);
    }

    #[test]
    fn a_displaced_read_only_skill_is_not_restored() {
        // The extra place exists to give a write tool back. A read-only skill
        // in fourth has none to give.
        let ranked = vec![
            candidate("Graph Editing", 0.9, &["update_node"]),
            candidate("Research & Search", 0.8, &["search_semantic"]),
            candidate("Organization", 0.7, &["create_relationship"]),
            candidate(
                "Play Workflow State",
                0.6,
                &["get_workflow_state", "search_nodes"],
            ),
        ];
        assert_eq!(select_candidates(ranked).len(), RETRIEVAL_TOP_K);
    }

    #[test]
    fn a_lookup_outside_the_window_changes_nothing() {
        let ranked = vec![
            candidate("Node Creation", 0.9, &["create_node"]),
            candidate("Graph Editing", 0.8, &["update_node"]),
            candidate("Schema Creation", 0.7, &["create_schema"]),
            candidate("Research & Search", 0.6, &["search_semantic"]),
        ];
        assert_eq!(
            names(&select_candidates(ranked)),
            vec!["Node Creation", "Graph Editing", "Schema Creation"]
        );
    }

    fn procedure(name: &str, score: f32, tools: &[&str]) -> SkillCandidate {
        SkillCandidate {
            role: SkillRole::Procedure,
            ..candidate(name, score, tools)
        }
    }

    #[test]
    fn a_procedure_takes_no_place_from_a_tool_skill() {
        // It outscores every tool skill, and is kept in addition to the three
        // the window holds.
        let ranked = vec![
            procedure("Completing a Task", 0.95, &["update_node"]),
            candidate("Node Deletion", 0.9, &["delete_node"]),
            candidate("Graph Editing", 0.8, &["update_node"]),
            candidate("Organization", 0.7, &["create_relationship"]),
        ];
        assert_eq!(
            names(&select_candidates(ranked)),
            vec![
                "Node Deletion",
                "Graph Editing",
                "Organization",
                "Completing a Task"
            ]
        );
    }

    #[test]
    fn only_the_best_procedure_is_kept() {
        let ranked = vec![
            procedure("Writing a Plan", 0.9, &["create_node"]),
            procedure("Writing a Spec", 0.8, &["create_node"]),
            candidate("Graph Editing", 0.7, &["update_node"]),
        ];
        assert_eq!(
            names(&select_candidates(ranked)),
            vec!["Graph Editing", "Writing a Plan"]
        );
    }

    #[test]
    fn a_procedure_never_leads_the_turn() {
        // Above the deletion skill, it would otherwise make that skill not the
        // top score and withhold `delete_node`.
        let candidates = vec![
            procedure("Completing a Task", 0.95, &["update_node"]),
            candidate("Node Deletion", 0.9, &["delete_node"]),
        ];
        assert_eq!(
            leading_tool_bearing_candidate(&candidates).map(|c| c.name.as_str()),
            Some("Node Deletion")
        );
        assert!(stage2_permitted_names(&candidates).contains("delete_node"));
    }

    #[test]
    fn a_procedure_does_not_count_as_a_skill_that_can_create_a_record() {
        // Recording a Decision holds `create_node`, and still leaves the add
        // without a tool skill that leads it.
        let ranked = vec![
            candidate("Graph Editing", 0.9, &["update_node"]),
            procedure("Recording a Decision", 0.85, &["create_node"]),
        ];
        assert!(lacks_a_creating_skill(&ranked));
        let with_creator = vec![
            candidate("Node Creation", 0.9, &["create_node"]),
            procedure("Recording a Decision", 0.85, &["create_node"]),
        ];
        assert!(!lacks_a_creating_skill(&with_creator));
    }

    #[test]
    fn fewer_candidates_than_the_bound_are_all_kept() {
        let ranked = vec![
            candidate("Graph Editing", 0.9, &["update_node"]),
            candidate("Research & Search", 0.8, &["search_semantic"]),
        ];
        assert_eq!(select_candidates(ranked).len(), 2);
        assert!(select_candidates(Vec::new()).is_empty());
    }

    #[test]
    fn a_question_or_a_retrieval_verb_makes_a_message_lookup_shaped() {
        for message in [
            "How do we onboard a new sponsor?",
            "what is the merge gate",
            "  Why did we pick sqlite  ",
            "could you add a task?",
            "the retry policy, how does it work?",
            "find the record for the Lisbon offsite",
            "Explain our retry policy",
            "list the specs we have on sync",
            "tell me how we onboard a sponsor",
            "Look up the release checklist",
        ] {
            assert!(is_lookup_shaped(message), "{message:?}");
        }
        for message in [
            "Add a task to renew the domain",
            "however you like, mark it done",
            "whatever",
            "finding nothing, mark it done",
            "",
        ] {
            assert!(!is_lookup_shaped(message), "{message:?}");
        }
    }

    #[test]
    fn the_message_is_asked_about_first_only_as_a_question_with_turns_ahead_of_it() {
        let blended = "PRIOR CONTEXT: …\nCURRENT REQUEST: How do we onboard a sponsor?";
        let question = "How do we onboard a sponsor?";
        assert!(asks_about_the_message_first(blended, question, false));
        // The first turn of a chat: the blended view is the message.
        assert!(!asks_about_the_message_first(question, question, false));
        // Not a question.
        assert!(!asks_about_the_message_first(
            "PRIOR CONTEXT: …",
            "mark it paid",
            false
        ));
        // An answer to a clarifying question, whatever its punctuation.
        assert!(!asks_about_the_message_first(
            blended,
            "the client ones?",
            true
        ));
    }

    #[test]
    fn a_full_width_question_mark_is_a_question_mark() {
        assert!(is_lookup_shaped("これは何ですか？"));
    }

    #[test]
    fn route_lookup_parses_into_a_lookup_decision() {
        assert_eq!(
            parse_route_decision(
                ROUTE_LOOKUP_TOOL,
                r#"{"topic":" how invoices get approved "}"#
            ),
            Some(RouteDecision::Lookup(
                "how invoices get approved".to_string()
            )),
            "the topic is trimmed"
        );
    }

    #[test]
    fn a_lookup_with_no_topic_yields_no_decision() {
        // Nothing to search for: fall through to retrieval on the raw message
        // rather than run a lookup for the empty string.
        assert_eq!(
            parse_route_decision(ROUTE_LOOKUP_TOOL, r#"{"topic":"  "}"#),
            None
        );
        assert_eq!(parse_route_decision(ROUTE_LOOKUP_TOOL, "{}"), None);
    }

    #[test]
    fn a_question_about_the_workspace_opens_with_a_question_word_and_is_not_about_the_assistant() {
        for message in [
            "How do we onboard a new reviewer?",
            "  how do we decide which cycle a slipped spec moves into？ ",
            "Who approves a change to a cycle's capacity?",
            "When did we sign off Kestrel?",
            "What's the weather like in Tokyo today?",
            // Asked without a question mark: after "tell me", as "how many",
            // or as a polite request to read.
            "Tell me how many tasks are assigned Anoop",
            "Let me know what is due on Friday",
            "How many tasks are assigned to Norbert",
            "Could you check the status of tasks assigned to Anoop",
            "Please list the tasks assigned to Anoop",
            "Can you show me the open tasks",
        ] {
            assert!(asks_what_the_workspace_holds(message), "{message:?}");
        }
        for message in [
            // About the assistant.
            "Which skills do you have?",
            "what can you do?",
            "which of your skills would I use to delete something?",
            "What are you able to do, yourself?",
            "What skills are available?",
            "Which tools are there for deleting something?",
            "How does this assistant work?",
            // Suggestions.
            "How about a task for that?",
            "What about adding Harbour as a planning cycle?",
            "What if we moved the review to Friday?",
            "Why not close the Q4 cycle?",
            "Why don't we close the Q4 cycle?",
            // A skill, a tool or an assistant the user keeps records of:
            // left out by the word, and routed as Stage 1 routes it.
            "Which tool do we use for deploys?",
            // A question word, and no question.
            "When the review is done, mark it signed off",
            "Which reminds me, add a note about the offsite",
            "What I need is a new task to call the vendor",
            "how do we decide which cycle a slipped spec moves into",
            // Without a question mark, and asking for a change as well.
            "Tell me when it is Friday and mark the task done",
            "How many tasks are open, close the old ones",
            "Could you list the tasks and delete the old ones",
            "Could you check off the pricing task",
            "Could you check the status of the task and mark it done",
            // No question word.
            "Could you add Harbour as a planning cycle?",
            "find the record for the Lisbon offsite",
            "mark it booked",
            "?",
            "",
        ] {
            assert!(!asks_what_the_workspace_holds(message), "{message:?}");
        }
    }

    #[test]
    fn a_query_for_a_question_about_the_workspace_is_a_lookup_of_the_message() {
        assert_eq!(
            decision_on_the_message_alone(
                " How do we onboard a new reviewer? ",
                Some(RouteDecision::Query("onboard a new reviewer".to_string())),
            ),
            Some(RouteDecision::Lookup(
                "How do we onboard a new reviewer".to_string()
            ))
        );
    }

    #[test]
    fn every_other_decision_on_the_message_alone_stands() {
        let query = Some(RouteDecision::Query("describe your own skills".to_string()));
        assert_eq!(
            decision_on_the_message_alone("Which skills do you have?", query.clone()),
            query,
            "a question about the assistant stays a query"
        );
        let write = Some(RouteDecision::Query("add a planning cycle".to_string()));
        assert_eq!(
            decision_on_the_message_alone(
                "Could you add Harbour as a planning cycle?",
                write.clone()
            ),
            write,
            "a request that only ends in a question mark stays a query"
        );
        let lookup = Some(RouteDecision::Lookup("the merge gate".to_string()));
        assert_eq!(
            decision_on_the_message_alone("What is the merge gate?", lookup.clone()),
            lookup
        );
        let split = Some(RouteDecision::Multi(vec![
            "track deprecations".to_string(),
            "find notes on rate limiting".to_string(),
        ]));
        assert_eq!(
            decision_on_the_message_alone("How do we do both?", split.clone()),
            split
        );
        assert_eq!(
            decision_on_the_message_alone("How do we do it?", None),
            None
        );
        let suggestion = Some(RouteDecision::Query("add a task".to_string()));
        assert_eq!(
            decision_on_the_message_alone("How about a task for that?", suggestion.clone()),
            suggestion,
            "a suggestion that opens with a question word stays a query"
        );
    }

    #[test]
    fn a_lookup_is_retrieved_as_the_capability_then_the_topic() {
        assert_eq!(
            lookup_retrieval_query("the merge gate"),
            "find, look up, or search stored knowledge for the merge gate"
        );
    }

    #[test]
    fn the_system_lookup_searches_the_topic_and_reads_three_results() {
        let call = lookup_call("how invoices get approved", &[tool("search_semantic")])
            .expect("search_semantic is on the surface");
        assert_eq!(call.function_name, "search_semantic");
        let again = lookup_call("how invoices get approved", &[tool("search_semantic")])
            .expect("search_semantic is on the surface");
        assert_ne!(call.id, again.id, "each call carries its own id");
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&call.arguments_json).unwrap(),
            serde_json::json!({"query": "how invoices get approved", "include_markdown": 3})
        );
    }

    #[test]
    fn the_system_lookup_calls_only_a_tool_the_surface_offers() {
        assert!(lookup_call("anything", &[tool("search_nodes"), tool("get_node")]).is_none());
        assert!(lookup_call("anything", &[]).is_none());
    }

    #[test]
    fn skill_names_render_as_one_list_with_where_to_answer_from() {
        let rendered = render_skill_names_for_prompt(&[
            "Node Creation".to_string(),
            "Research & Search".to_string(),
        ])
        .expect("names render");
        assert!(rendered.starts_with("YOUR SKILLS: Node Creation, Research & Search.\n"));
        assert!(
            rendered.contains("a search does not return them"),
            "must say a skill is not found by searching: {rendered}"
        );
    }

    #[test]
    fn no_skill_names_render_nothing() {
        // An empty list must not claim the agent has no skills.
        assert!(render_skill_names_for_prompt(&[]).is_none());
    }

    #[test]
    fn all_candidate_scores_includes_every_candidate_regardless_of_the_gate() {
        // Unlike `routed_skill_names`, a below-bar candidate must still
        // appear — this field exists specifically to show the scores the
        // gate filtered out, so a tiebreak or a low-scoring-with-no-
        // competitor situation is distinguishable from the log alone.
        let cands = vec![
            candidate("Research & Search", 0.9, &["search_nodes"]),
            candidate("Below The Bar", 0.01, &["search_nodes"]),
        ];
        assert_eq!(
            all_candidate_scores(&cands),
            "Research & Search=0.900, Below The Bar=0.010"
        );
    }

    #[test]
    fn all_candidate_scores_is_empty_when_retrieval_returned_nothing() {
        assert_eq!(all_candidate_scores(&[]), "");
    }

    #[test]
    fn rendered_candidates_carry_their_instruction_subtree() {
        let cands = vec![candidate("Schema Creation", 0.9, &["create_schema"])];
        let rendered = render_candidates_for_prompt(&cands).unwrap();
        assert!(rendered.contains("Schema Creation instructions"));
        assert!(rendered.contains("Purpose: Schema Creation description"));
    }

    #[test]
    fn with_stage2_clarify_adds_the_registered_tool_once() {
        let all = vec![tool("list_conflicts"), tool(ROUTE_CLARIFY_TOOL)];
        let names = |ts: &[ToolDefinition]| ts.iter().map(|t| t.name.clone()).collect::<Vec<_>>();

        let added = with_stage2_clarify(vec![tool("list_conflicts")], &all);
        assert_eq!(names(&added), ["list_conflicts", ROUTE_CLARIFY_TOOL]);

        // A skill that already whitelists it must not see it twice.
        let again = with_stage2_clarify(added, &all);
        assert_eq!(names(&again), ["list_conflicts", ROUTE_CLARIFY_TOOL]);

        // A registry without it adds nothing rather than inventing a def.
        let absent = with_stage2_clarify(vec![tool("list_conflicts")], &[tool("list_conflicts")]);
        assert_eq!(names(&absent), ["list_conflicts"]);
    }

    #[test]
    fn stage2_tools_are_scoped_to_eligible_candidate_whitelists() {
        let all = vec![
            tool("search_nodes"),
            tool("get_node"),
            tool("create_schema"),
            tool("delete_node"),
        ];
        let cands = vec![candidate("research", 0.9, &["search_nodes", "get_node"])];
        let scoped = stage2_tools(&cands, &all);
        let names: Vec<&str> = scoped.iter().map(|t| t.name.as_str()).collect();
        assert_eq!(names, vec!["search_nodes", "get_node"]);
        assert!(
            !names.contains(&"delete_node"),
            "a read-only skill must not put a destructive tool in reach"
        );
    }

    /// A tool shaped like `create_node`/`update_node` — a `field_values`
    /// object parameter alongside others — for exercising
    /// `declare_write_tool_fields` without depending on the real registry.
    fn write_tool(name: &str) -> ToolDefinition {
        ToolDefinition {
            name: name.to_string(),
            description: String::new(),
            parameters_schema: json!({
                "type": "object",
                "properties": {
                    "field_values": { "type": "object" }
                }
            }),
        }
    }

    fn schema_metadata_for(type_id: &str, fields: &[(&str, &str)]) -> serde_json::Value {
        use nodespace_core::ops::entity_types_block::{
            EntityFieldDescriptor, EntityTypeDescriptor,
        };
        json!([EntityTypeDescriptor {
            type_id: type_id.to_string(),
            name: Some(type_id.to_string()),
            fields: fields
                .iter()
                .map(|(name, field_type)| EntityFieldDescriptor {
                    name: name.to_string(),
                    field_type: field_type.to_string(),
                    item_type: None,
                    enum_values: Vec::new(),
                    required: false,
                    description: None,
                })
                .collect(),
            relationships: vec![],
            title_template: None,
        }
        .to_json()])
    }

    fn field_values_property_names(tool: &ToolDefinition) -> Vec<String> {
        tool.parameters_schema["properties"]["field_values"]["properties"]
            .as_object()
            .map(|m| m.keys().cloned().collect())
            .unwrap_or_default()
    }

    /// The single-candidate case: its own retrieved schema declares the
    /// write tool's `field_values` fields. Calls `declare_write_tool_fields`
    /// directly, not through `stage2_tools` — the two are deliberately
    /// separate steps (see `declare_write_tool_fields`'s doc comment), and a
    /// production turn only reaches this one when `!routing_disabled`.
    #[test]
    fn declare_write_tool_fields_declares_from_the_sole_candidates_schema() {
        let mut c = candidate("Node Creation", 0.85, &["create_node"]);
        c.schema_metadata =
            schema_metadata_for("ticket", &[("status", "text"), ("assignee", "text")]);
        let declared = declare_write_tool_fields(&[c], vec![write_tool("create_node")]);
        let names = field_values_property_names(&declared[0]);
        assert!(names.contains(&"status".to_string()));
        assert!(names.contains(&"assignee".to_string()));
    }

    /// The distractor case `dev-schema-creation.toml` (packages/agent/goldens/)
    /// measures: a write tool whitelisted only by a LOWER-scoring candidate
    /// than the turn's actual top match must stay on the bare-object
    /// fallback, not gain declarations that would make the wrong tool easier
    /// to reach for.
    #[test]
    fn declare_write_tool_fields_leaves_a_lower_scored_candidates_tool_bare() {
        let mut top = candidate("Schema Creation", 0.9, &["create_schema"]);
        top.schema_metadata = schema_metadata_for("ticket", &[("status", "text")]);
        let mut distractor = candidate("Node Creation", 0.5, &["create_node"]);
        distractor.schema_metadata = schema_metadata_for("ticket", &[("status", "text")]);

        let declared = declare_write_tool_fields(
            &[top, distractor],
            vec![write_tool("create_schema"), write_tool("create_node")],
        );
        let create_node = declared
            .iter()
            .find(|t| t.name == "create_node")
            .expect("create_node must still be present in the input list");
        assert!(
            field_values_property_names(create_node).is_empty(),
            "the distractor's tool must not receive field declarations"
        );
    }

    /// The documented trade-off from `declare_write_tool_fields`'s doc
    /// comment, pinned rather than left as an untested claim: two
    /// candidates that whitelist two DIFFERENT tools (not competing for the
    /// same tool, unlike the distractor case above) still leave the
    /// lower-scored candidate's tool undeclared, because the selection is
    /// the turn's GLOBAL top score, not a per-tool one. This is deliberate
    /// (see the doc comment for why the distractor case requires it), but
    /// it means a genuinely compound turn — both tools legitimately wanted
    /// — only gets one of them declared. Falls back to the bare-object
    /// shape for the other, not a regression relative to production before
    /// per-candidate field declaration existed, just an unrealized
    /// improvement: per-skill retrieval scoping (see the doc comment above)
    /// would let both tools' top candidates get their fields declared
    /// independently instead of only the turn's single global top scorer.
    #[test]
    fn declare_write_tool_fields_does_not_declare_a_non_top_scoring_but_uncontested_tool() {
        let mut top = candidate("Relationship Management", 0.9, &["create_relationship"]);
        top.schema_metadata = schema_metadata_for("adr", &[("supersedes", "text")]);
        let mut other = candidate("Graph Editing", 0.6, &["update_node"]);
        other.schema_metadata = schema_metadata_for("ticket", &[("status", "text")]);

        let declared = declare_write_tool_fields(
            &[top, other],
            vec![write_tool("create_relationship"), write_tool("update_node")],
        );
        let update_node = declared
            .iter()
            .find(|t| t.name == "update_node")
            .expect("update_node must still be present in the input list");
        assert!(
            field_values_property_names(update_node).is_empty(),
            "documented trade-off: a lower-scored candidate's own, uncontested tool still \
             stays undeclared under the global-max rule — if this now fails because the rule \
             changed to a per-tool max, re-verify declare_write_tool_fields_leaves_a_lower_scored_candidates_tool_bare \
             (the distractor case) still passes, since the two tests pull the rule in opposite directions"
        );
    }

    /// A zero-tool candidate must not raise the bar that decides whose
    /// descriptors are declared. It whitelists no tool, so it can never be
    /// the match that contributes one — letting it set `max_score` only ever
    /// silenced candidates that could.
    ///
    /// This is the `stage2_permitted_names` defect in its sibling: a
    /// schema-typed hit is pinned by the lexical backstop above any
    /// cosine-derived score and clears the gate unconditionally at the read
    /// rung, so before the fix it stripped declarations from every write
    /// tool on exactly the turns where the user named a schema.
    #[test]
    fn declare_write_tool_fields_ignores_a_higher_scoring_zero_tool_candidate() {
        let schema_hit = candidate("Meeting Note", 1.0, &[]);
        let mut skill = candidate("Node Creation", 0.7, &["create_node"]);
        skill.schema_metadata = schema_metadata_for("ticket", &[("status", "text")]);

        let declared =
            declare_write_tool_fields(&[schema_hit, skill], vec![write_tool("create_node")]);
        assert!(
            field_values_property_names(&declared[0]).contains(&"status".to_string()),
            "a tool-bearing candidate that tops every other tool-bearing candidate must still \
             have its fields declared, whatever a tool-less candidate scored"
        );
    }

    /// The degenerate shape the tool-bearing filter introduces: with no
    /// tool-bearing candidate at all, `max_score` folds to its
    /// `f32::NEG_INFINITY` seed, which `c.score >= max_score` admits for
    /// anything. Safe, but via the *second* conjunct rather than the first —
    /// a zero-tool candidate never matches `c.tools.iter().any(...)`, so
    /// nothing is declared.
    /// Pinned because the first conjunct silently stops carrying the weight
    /// here, and a later edit to the second one could quietly start
    /// fabricating declarations from candidates that whitelist nothing.
    #[test]
    fn declare_write_tool_fields_declares_nothing_when_every_candidate_is_tool_less() {
        let mut a = candidate("Meeting Note", 1.0, &[]);
        a.schema_metadata = schema_metadata_for("ticket", &[("status", "text")]);
        let mut b = candidate("Sprint", 0.9, &[]);
        b.schema_metadata = schema_metadata_for("adr", &[("supersedes", "text")]);

        let declared = declare_write_tool_fields(&[a, b], vec![write_tool("create_node")]);
        assert!(
            field_values_property_names(&declared[0]).is_empty(),
            "candidates whitelisting no tool must not have their schemas declared onto one"
        );
    }

    /// Two candidates tied at the top score both whitelisting the same tool:
    /// their schemas are unioned rather than one arbitrarily winning — the
    /// snapshot-gate fixture (`prompt_assembly_snapshot.rs`) scores "Node
    /// Creation" and "Schema Creation" identically on purpose.
    #[test]
    fn declare_write_tool_fields_unions_tied_top_scoring_candidates() {
        let mut a = candidate("A", 0.85, &["create_node"]);
        a.schema_metadata = schema_metadata_for("ticket", &[("status", "text")]);
        let mut b = candidate("B", 0.85, &["create_node"]);
        b.schema_metadata = schema_metadata_for("adr", &[("supersedes", "text")]);

        let declared = declare_write_tool_fields(&[a, b], vec![write_tool("create_node")]);
        let names = field_values_property_names(&declared[0]);
        assert!(names.contains(&"status".to_string()));
        assert!(names.contains(&"supersedes".to_string()));
    }

    /// No cleared candidate must leave every tool's `field_values` on the
    /// static bare-object fallback — there is no retrieved schema to declare
    /// from, and the function must not panic or fabricate a declaration on
    /// an empty candidate list (this exercises `declare_write_tool_fields`
    /// directly with `&[]`, unlike `stage2_tools`'s own fail-open path,
    /// which never reaches this function at all — see
    /// `an_ineligible_candidate_does_not_widen_the_tool_surface` and
    /// neighbouring tests below for `stage2_tools`'s fail-open behaviour).
    #[test]
    fn declare_write_tool_fields_is_a_no_op_with_no_cleared_candidates() {
        let declared =
            declare_write_tool_fields(&[], vec![write_tool("create_node"), tool("search_nodes")]);
        let create_node = declared
            .iter()
            .find(|t| t.name == "create_node")
            .expect("create_node must still be present in the input list");
        assert!(field_values_property_names(create_node).is_empty());
    }

    /// `agent_loop.rs` gates this function behind `!session.routing_disabled`
    /// the same way it gates `candidate_block` prompt injection — this pins
    /// that `stage2_tools` itself never calls it, so a caller that forgets
    /// the gate cannot accidentally get it "for free" from tool scoping.
    #[test]
    fn stage2_tools_alone_never_declares_field_values() {
        let mut c = candidate("Node Creation", 0.85, &["create_node"]);
        c.schema_metadata = schema_metadata_for("ticket", &[("status", "text")]);
        let scoped = stage2_tools(&[c], &[write_tool("create_node")]);
        assert!(
            field_values_property_names(&scoped[0]).is_empty(),
            "stage2_tools must not itself declare field_values — that is declare_write_tool_fields's job, gated separately by the caller"
        );
    }

    #[test]
    fn an_ineligible_candidate_does_not_widen_the_tool_surface() {
        let all = vec![tool("search_nodes"), tool("delete_node")];
        // Deletion is mutating, so 0.2 is below its bar though above the read bar.
        let cands = vec![
            candidate("research", 0.9, &["search_nodes"]),
            candidate("deletion", 0.2, &["delete_node"]),
        ];
        let scoped = stage2_tools(&cands, &all);
        let names: Vec<&str> = scoped.iter().map(|t| t.name.as_str()).collect();
        assert_eq!(names, vec!["search_nodes"]);
    }

    #[test]
    fn empty_retrieval_falls_back_to_the_full_tool_surface() {
        let all = vec![tool("search_nodes"), tool("create_schema")];
        assert_eq!(stage2_tools(&[], &all).len(), 2);
    }

    #[test]
    fn whitelist_naming_only_unregistered_tools_fails_open() {
        let all = vec![tool("search_nodes")];
        let cands = vec![candidate("ghost", 0.9, &["tool_that_does_not_exist"])];
        assert_eq!(
            stage2_tools(&cands, &all).len(),
            1,
            "stranding the model with zero tools is worse than a wide surface"
        );
    }

    #[test]
    fn fail_open_on_empty_retrieval_excludes_resolve_query() {
        // No candidates at all — the emptiest fail-open case. resolve_query's
        // required node_type parameter depends on the EXISTING SCHEMAS
        // block, which only renders alongside a scoped whitelist; handing the
        // tool over here would strand the model with an instruction ("copy
        // the id from the EXISTING SCHEMAS block") pointing at nothing.
        let all = vec![
            tool("search_nodes"),
            tool("resolve_query"),
            tool("get_node"),
        ];
        let scoped = stage2_tools(&[], &all);
        let names: Vec<&str> = scoped.iter().map(|t| t.name.as_str()).collect();
        assert!(!names.contains(&"resolve_query"));
        assert!(names.contains(&"search_nodes"));
        assert!(names.contains(&"get_node"));
    }

    #[test]
    fn a_graph_editing_candidate_below_its_mutating_bar_fails_open_without_resolve_query() {
        // The regression case #1840 calls out explicitly: a Graph Editing
        // candidate scoring in [READ_SKILL_SCORE_BAR, MUTATING_SKILL_SCORE_BAR)
        // — below the bar its own blast radius requires, but above the
        // read-only bar. It must not clear the gate, and the fail-open
        // surface it falls through to must still exclude resolve_query.
        let all = vec![
            tool("search_nodes"),
            tool("resolve_query"),
            tool("update_node"),
        ];
        let cands = vec![candidate(
            "Graph Editing",
            0.2,
            &["update_node", "search_nodes", "resolve_query"],
        )];
        assert!(
            (READ_SKILL_SCORE_BAR..MUTATING_SKILL_SCORE_BAR).contains(&0.2),
            "fixture score must sit in the gap this test targets"
        );
        assert!(!clears_score_gate(&cands[0]));

        let scoped = stage2_tools(&cands, &all);
        let names: Vec<&str> = scoped.iter().map(|t| t.name.as_str()).collect();
        assert!(!names.contains(&"resolve_query"));
        // And the prompt block that would justify offering it is absent too.
        assert!(render_candidates_for_prompt(&cands).is_none());
    }

    #[test]
    fn a_graph_editing_candidate_that_clears_its_bar_offers_resolve_query_with_its_guidance() {
        // The positive case: when Graph Editing genuinely clears the mutating
        // bar, resolve_query is offered — and the same eligibility filter
        // that scopes stage2_tools also renders the EXISTING SCHEMAS
        // block resolve_query's description depends on, so the tool never
        // reaches the model without its guidance.
        let all = vec![
            tool("search_nodes"),
            tool("resolve_query"),
            tool("update_node"),
        ];
        let mut c = candidate(
            "Graph Editing",
            0.9,
            &["update_node", "search_nodes", "resolve_query"],
        );
        c.schema_metadata = json!([{"type_id": "invoice", "fields": []}]);

        let scoped = stage2_tools(&[c.clone()], &all);
        let names: Vec<&str> = scoped.iter().map(|t| t.name.as_str()).collect();
        assert!(names.contains(&"resolve_query"));

        let rendered =
            render_candidates_for_prompt(&[c]).expect("cleared candidate renders a block");
        assert!(rendered.contains("EXISTING SCHEMAS"));
    }

    #[test]
    fn whitelist_naming_only_unregistered_tools_still_excludes_resolve_query_on_fail_open() {
        // The other fail-open branch: a whitelist that resolves to nothing
        // registered. Falls through to the same guidance-free surface, so the
        // exclusion must hold here too, not just on empty retrieval.
        let all = vec![tool("search_nodes"), tool("resolve_query")];
        let cands = vec![candidate("ghost", 0.9, &["tool_that_does_not_exist"])];
        let scoped = stage2_tools(&cands, &all);
        let names: Vec<&str> = scoped.iter().map(|t| t.name.as_str()).collect();
        assert!(!names.contains(&"resolve_query"));
        assert!(names.contains(&"search_nodes"));
    }

    /// A workspace context carrying a rendered entity-types block, as
    /// `context_ops::build_workspace_context` produces it.
    fn workspace_with_block() -> String {
        format!("Collections: none\n{EXISTING_SCHEMAS_HEADER}\n- invoice (amount, status)\n")
    }

    #[test]
    fn tools_with_available_guidance_requires_a_rendered_entity_types_block() {
        // A candidate clearing the gate but with empty schema_metadata (no
        // typed entities) whitelists resolve_query yet renders no entity-
        // types sub-block for it — render_candidates_for_prompt's header text
        // alone would make `candidate_block` `Some`, but that's not the same
        // as resolve_query having guidance available. With no workspace block
        // either, neither site supplies it.
        let cands = vec![candidate("Graph Editing", 0.9, &["resolve_query"])];
        assert!(tools_with_available_guidance(&cands, false, "").is_empty());
    }

    #[test]
    fn tools_with_available_guidance_includes_a_tool_whose_candidate_renders_real_entity_types() {
        let mut c = candidate("Graph Editing", 0.9, &["resolve_query"]);
        c.schema_metadata = json!([{"type_id": "invoice", "fields": []}]);
        let cands = vec![c];
        let available = tools_with_available_guidance(&cands, false, "");
        assert!(available.contains("resolve_query"));
    }

    #[test]
    fn tools_with_available_guidance_is_empty_when_rendering_is_disabled() {
        // render_disabled mirrors session.routing_disabled: even a candidate
        // with real schema_metadata contributes nothing once injection is
        // suppressed for this turn — absent a workspace block, which is a
        // separate site that flag says nothing about.
        let mut c = candidate("Graph Editing", 0.9, &["resolve_query"]);
        c.schema_metadata = json!([{"type_id": "invoice", "fields": []}]);
        let cands = vec![c];
        assert!(tools_with_available_guidance(&cands, true, "").is_empty());
    }

    #[test]
    fn tools_with_available_guidance_excludes_a_candidate_below_its_score_bar() {
        // Real schema_metadata doesn't matter if the candidate never clears
        // the gate in the first place. resolve_query alone is a read tool
        // (READ_SKILL_SCORE_BAR = 0.15), so the fixture score must sit below
        // that, not just below the mutating bar.
        let mut c = candidate("Graph Editing", 0.01, &["resolve_query"]);
        c.schema_metadata = json!([{"type_id": "invoice", "fields": []}]);
        let cands = vec![c];
        assert!(!clears_score_gate(&cands[0]));
        assert!(tools_with_available_guidance(&cands, false, "").is_empty());
    }

    #[test]
    fn a_workspace_entity_types_block_supplies_guidance_a_candidate_lacks() {
        // The reachability bug this gate had: Graph Editing whitelists
        // resolve_query but links to no schema, so `skill_ops` falls
        // back to "all non-core schemas" — empty in a workspace whose custom
        // schemas didn't match, leaving `schema_metadata` empty even though
        // the resident workspace context did carry the block. The tool's
        // required `node_type` is answerable from that block, so withholding
        // it here stranded the model with no way to resolve an indirect
        // target.
        let cands = vec![candidate("Graph Editing", 0.9, &["resolve_query"])];
        assert!(
            tools_with_available_guidance(&cands, false, "").is_empty(),
            "no block at either site: still withheld"
        );
        let available = tools_with_available_guidance(&cands, false, &workspace_with_block());
        assert!(
            available.contains("resolve_query"),
            "workspace context carries the block, so the required parameter is answerable"
        );
    }

    #[test]
    fn a_workspace_block_does_not_rescue_a_candidate_below_its_score_bar() {
        // The workspace block makes the *parameter* answerable; it does not
        // make an unmatched skill's tools eligible. Routing's trust boundary
        // (ADR-038) still decides which skills may act.
        let cands = vec![candidate("Graph Editing", 0.01, &["resolve_query"])];
        assert!(!clears_score_gate(&cands[0]));
        assert!(
            tools_with_available_guidance(&cands, false, &workspace_with_block()).is_empty(),
            "score gate still governs eligibility"
        );
    }

    #[test]
    fn a_workspace_block_survives_candidate_injection_being_disabled() {
        // `routing_disabled` exists because injecting the *candidate* block
        // suppresses tool-calling on some served models. The resident
        // workspace block is present regardless of that flag, so the tool's
        // required parameter is still answerable and the tool stays offered.
        let cands = vec![candidate("Graph Editing", 0.9, &["resolve_query"])];
        let available = tools_with_available_guidance(&cands, true, &workspace_with_block());
        assert!(available.contains("resolve_query"));
    }

    #[test]
    fn a_workspace_block_without_the_shared_heading_does_not_count() {
        // Matching is on the shared heading constant, not on any schema-ish
        // prose: a workspace context listing collections and playbooks but no
        // entity types leaves the required parameter unanswerable.
        let cands = vec![candidate("Graph Editing", 0.9, &["resolve_query"])];
        let no_block = "Collections: Invoices, Venues\nActive playbooks: none\n";
        assert!(tools_with_available_guidance(&cands, false, no_block).is_empty());
    }

    #[test]
    fn a_heading_with_no_type_listed_under_it_does_not_count() {
        // `context_ops`'s renderer pushes the heading, then breaks out of its
        // per-schema loop once the character budget is spent — a budget
        // exhausted by the very first type line leaves the heading behind with
        // nothing under it. Matching the heading alone would offer the tool
        // while its required `node_type` pointed at an empty list, which is
        // the strand this gate exists to prevent.
        let cands = vec![candidate("Graph Editing", 0.9, &["resolve_query"])];
        let truncated = format!("COLLECTIONS: Invoices\n\n{EXISTING_SCHEMAS_HEADER}\n");
        assert!(tools_with_available_guidance(&cands, false, &truncated).is_empty());
    }

    #[test]
    fn a_heading_followed_by_prose_rather_than_a_type_line_does_not_count() {
        // Guards the shape of the check itself: both renderers emit `- <id>`
        // lines, so anything else under the heading is not a type listing.
        let cands = vec![candidate("Graph Editing", 0.9, &["resolve_query"])];
        let prose = format!("{EXISTING_SCHEMAS_HEADER}\n(none recorded yet)\n");
        assert!(tools_with_available_guidance(&cands, false, &prose).is_empty());
    }

    #[test]
    fn a_malformed_metadata_entry_does_not_discard_the_others() {
        let mut c = candidate("Node Creation", 0.9, &["create_node"]);
        c.schema_metadata = json!([
            {"fields": [{"name": "orphan", "type": "text"}]},
            {"type_id": "task", "fields": [{"name": "title", "type": "text"}]}
        ]);
        let rendered = render_candidates_for_prompt(&[c]).unwrap();
        assert!(
            rendered.contains("- task -> title: text"),
            "an entry missing type_id must not take the valid ones with it: {rendered}"
        );
    }

    #[test]
    fn schema_metadata_marks_required_and_renders_title_template() {
        let mut c = candidate("Node Creation", 0.9, &["create_node"]);
        c.schema_metadata = json!([{
            "type_id": "invoice",
            "title_template": "{reference} - {amount}",
            "fields": [
                {"name": "reference", "type": "text"},
                {"name": "amount", "type": "number", "required": true}
            ]
        }]);
        let rendered = render_candidates_for_prompt(&[c]).unwrap();
        // Same notation the workspace-context block uses; both land in one prompt.
        assert!(
            rendered.contains("- invoice -> reference: text; amount: number, required"),
            "got: {rendered}"
        );
        // create_node's description promises the template is shown here.
        assert!(
            rendered.contains("[title_template: {reference} - {amount}]"),
            "got: {rendered}"
        );
    }

    #[test]
    fn a_zero_field_type_renders_without_a_dangling_separator() {
        let mut c = candidate("Node Creation", 0.9, &["create_node"]);
        c.schema_metadata = json!([
            {"type_id": "marker", "fields": []},
            {"type_id": "invoice", "fields": [{"name": "amount", "type": "number"}]}
        ]);
        let rendered = render_candidates_for_prompt(&[c]).unwrap();
        // No trailing ": " promising a field list that never arrives.
        assert!(rendered.contains("- marker\n"), "got: {rendered}");
        assert!(!rendered.contains("- marker:"), "got: {rendered}");
        // The populated form is unaffected.
        assert!(
            rendered.contains("- invoice -> amount: number"),
            "got: {rendered}"
        );
    }

    #[test]
    fn metadata_with_no_usable_entries_renders_no_section() {
        let mut c = candidate("Node Creation", 0.9, &["create_node"]);
        c.schema_metadata = json!([{"fields": []}]);
        let rendered = render_candidates_for_prompt(&[c]).unwrap();
        assert!(!rendered.contains("EXISTING SCHEMAS"));
    }

    /// This site's heading must be the exact shared constant, not an
    /// independently-worded copy — see EXISTING_SCHEMAS_HEADER's doc
    /// comment (#1846: two independently-maintained copies of this heading,
    /// one carrying an anti-copy clause and the other not, let contamination
    /// persist after the first was fixed).
    #[test]
    fn schema_metadata_section_uses_the_shared_header() {
        let mut c = candidate("Node Creation", 0.9, &["create_node"]);
        c.schema_metadata = json!([{"type_id": "invoice", "fields": []}]);
        let rendered = render_candidates_for_prompt(&[c]).unwrap();
        assert!(
            rendered.contains(EXISTING_SCHEMAS_HEADER),
            "got: {rendered}"
        );
    }

    #[test]
    fn schema_metadata_renders_enum_values() {
        let mut c = candidate("Node Creation", 0.9, &["create_node"]);
        c.schema_metadata = json!([{
            "type_id": "task",
            "fields": [
                {"name": "title", "type": "text"},
                {"name": "status", "type": "enum", "enum_values": ["todo", "done"]}
            ]
        }]);
        let rendered = render_candidates_for_prompt(&[c]).unwrap();
        assert!(rendered.contains("- task -> title: text; status: enum {todo, done}"));
    }

    /// Every eligible candidate carries its own copy of the entity block, so
    /// one Stage-2 prompt repeats it `RETRIEVAL_TOP_K` times — plus once more
    /// from the workspace-context path, which `agent_loop` concatenates into
    /// the same prompt under the same heading.
    ///
    /// Measured 2026-07-31 for the duplication question in #1848: **4 copies
    /// per prompt** (3 routing + 1 workspace context), costing ~141 redundant
    /// tokens on a 2-schema workspace and scaling with schema count. The copies
    /// are identical rather than differently scoped, because the built-in
    /// skills that write records link to no schema: their rows of
    /// `skill_pipeline::SKILL_SEEDS` have an empty `applies_to`, so each takes
    /// the same fallback.
    ///
    /// The assertion here is the invariant behind that measurement — one copy
    /// per eligible candidate — not the token figure, which is a dated finding
    /// rather than a target to hold constant.
    #[test]
    fn every_eligible_candidate_carries_its_own_entity_block() {
        let meta = json!([
            {
                "type_id": "invoice",
                "name": "Invoice",
                "fields": [
                    {"name": "reference", "type": "text"},
                    {"name": "amount", "type": "number", "required": true},
                    {"name": "status", "type": "enum", "enum_values": ["draft", "sent", "paid"]}
                ],
                "title_template": "{reference}"
            },
            {
                "type_id": "customer",
                "name": "Customer",
                "fields": [
                    {"name": "name", "type": "text", "required": true},
                    {"name": "email", "type": "text"}
                ]
            }
        ]);

        // All RETRIEVAL_TOP_K candidates eligible, each carrying the same
        // scoped schemas — the shape that produces the most copies.
        let candidates: Vec<SkillCandidate> = ["Node Creation", "Graph Editing", "Organization"]
            .iter()
            .map(|n| {
                let mut c = candidate(n, 0.9, &["create_node"]);
                c.schema_metadata = meta.clone();
                c
            })
            .collect();
        assert_eq!(candidates.len(), RETRIEVAL_TOP_K);

        let block = render_candidates_for_prompt(&candidates).expect("all are eligible");

        assert_eq!(
            block.matches("EXISTING SCHEMAS").count(),
            RETRIEVAL_TOP_K,
            "each eligible candidate carries its own copy of the entity block"
        );
    }

    /// Production `schema_metadata` carries a display `name` alongside the id,
    /// and the block must then read exactly as the workspace-context one does —
    /// `- id "Name" -> fields`. Both are concatenated under a single heading,
    /// so a type described two ways there reads to the model as two types.
    ///
    /// The other fixtures in this module omit `name`, which is why this case
    /// needs its own test: the format the agent actually ships would otherwise
    /// go unasserted.
    #[test]
    fn a_named_type_renders_in_the_workspace_context_format() {
        let mut c = candidate("Node Creation", 0.9, &["create_node"]);
        c.schema_metadata = json!([{
            "type_id": "invoice",
            "name": "Invoice",
            "fields": [
                {"name": "amount", "type": "number", "required": true}
            ],
            "title_template": "{reference}"
        }]);
        let rendered = render_candidates_for_prompt(&[c]).unwrap();
        assert!(
            rendered.contains(
                "- invoice \"Invoice\" -> amount: number, required [title_template: {reference}]"
            ),
            "named type must render as `- id \"Name\" -> fields`, got: {rendered}"
        );
    }

    /// A candidate carrying `types` as its linked schemas.
    fn linked(name: &str, score: f32, tools: &[&str], types: &[&str]) -> SkillCandidate {
        let mut c = candidate(name, score, tools);
        c.schema_metadata = json!(types
            .iter()
            .map(|id| json!({"type_id": id, "fields": []}))
            .collect::<Vec<_>>());
        c.schemas_linked = true;
        c
    }

    #[test]
    fn offered_types_are_the_union_of_every_linked_candidates_schemas() {
        // `issue` is linked by both skills and offered once; a subtype arrives
        // in the metadata like any other linked type.
        let candidates = [
            linked("Triage", 0.9, &["update_node"], &["issue", "bug"]),
            linked("Planning", 0.8, &["create_node"], &["task", "issue"]),
        ];
        assert_eq!(
            offered_types(&candidates),
            Some(vec![
                "issue".to_string(),
                "bug".to_string(),
                "task".to_string()
            ])
        );
    }

    #[test]
    fn one_unlinked_tool_bearing_candidate_leaves_the_turn_without_a_set() {
        // Node Creation carries the fallback for a skill that links to nothing:
        // the type the query named. Its tools may act on any type.
        let mut unlinked = candidate("Node Creation", 0.8, &["create_node"]);
        unlinked.schema_metadata = json!([{"type_id": "invoice", "fields": []}]);
        let candidates = [
            linked("Triage", 0.9, &["update_node"], &["issue"]),
            unlinked,
        ];
        assert_eq!(offered_types(&candidates), None);
    }

    #[test]
    fn an_unrouted_turn_has_no_offered_set() {
        assert_eq!(offered_types(&[]), None);
    }

    /// A candidate below its score bar is not on the turn: an unlinked one
    /// does not leave the turn open, and a linked one adds no type.
    #[test]
    fn a_candidate_below_its_bar_has_no_say_in_the_offered_set() {
        let below_bar_unlinked = candidate("Node Creation", 0.05, &["create_node"]);
        let below_bar_linked = linked("Planning", 0.05, &["create_node"], &["task"]);
        let candidates = [
            linked("Triage", 0.9, &["update_node"], &["issue"]),
            below_bar_unlinked,
            below_bar_linked,
        ];
        assert_eq!(offered_types(&candidates), Some(vec!["issue".to_string()]));
    }

    /// A schema-typed retrieval hit whitelists no tool, so it cannot act: being
    /// unlinked, it does not leave the turn open, and alone it makes no set.
    /// Its type is listed in the candidate block, though — it is how a type
    /// the request names outright gets there — so a held turn offers it.
    #[test]
    fn a_tool_less_candidate_adds_its_type_but_cannot_open_or_make_the_set() {
        let mut schema_hit = candidate("Invoice", 1.0, &[]);
        schema_hit.schema_metadata = json!([{"type_id": "invoice", "fields": []}]);

        let with_a_linked_skill = [
            schema_hit.clone(),
            linked("Triage", 0.9, &["update_node"], &["issue"]),
        ];
        assert_eq!(
            offered_types(&with_a_linked_skill),
            Some(vec!["invoice".to_string(), "issue".to_string()])
        );
        assert_eq!(offered_types(&[schema_hit]), None);
    }

    /// The enforced set and the candidate block are read from the same
    /// candidates: a held turn offers a type exactly when the block lists it.
    #[test]
    fn the_offered_set_is_exactly_the_types_the_candidate_block_lists() {
        let mut schema_hit = candidate("Invoice", 1.0, &[]);
        schema_hit.schema_metadata = json!([{"type_id": "invoice", "fields": []}]);
        let candidates = [
            schema_hit,
            linked("Triage", 0.9, &["update_node"], &["issue", "bug"]),
            linked("Planning", 0.8, &["create_node"], &["task", "issue"]),
            // Below its bar: neither rendered nor offered.
            linked("Archiving", 0.05, &["delete_node"], &["archive"]),
        ];

        let block = render_candidates_for_prompt(&candidates).unwrap();
        let listed: Vec<&str> = block
            .lines()
            .filter_map(|l| l.strip_prefix("- "))
            .map(|l| l.split_whitespace().next().unwrap())
            .collect();
        let mut listed_once: Vec<&str> = Vec::new();
        for id in listed {
            if !listed_once.contains(&id) {
                listed_once.push(id);
            }
        }

        assert_eq!(offered_types(&candidates).unwrap(), listed_once);
        assert_eq!(listed_once, ["invoice", "issue", "bug", "task"]);
    }

    fn pinned(mut candidate: SkillCandidate) -> SkillCandidate {
        candidate.pinned = true;
        candidate.score = 0.0;
        candidate
    }

    fn no_scores() -> std::collections::HashMap<String, f32> {
        std::collections::HashMap::new()
    }

    /// A pinned skill retrieval did not return joins the candidates, and is
    /// rendered and offered with no score to its name.
    #[test]
    fn a_pinned_skill_is_a_candidate_beside_what_retrieval_selected() {
        let selected = vec![candidate("research", 0.9, &["search_nodes"])];
        let authoring = pinned(linked("authoring", 0.0, &["update_play"], &["play"]));

        let candidates =
            with_pinned_skills(selected, std::slice::from_ref(&authoring), &no_scores());

        assert_eq!(names(&candidates), ["research", "authoring"]);
        assert!(clears_score_gate(&candidates[1]));
        let rendered = render_candidates_for_prompt(&candidates).expect("both are eligible");
        assert!(rendered.contains("authoring instructions"), "{rendered}");
        assert!(rendered.contains("play"), "{rendered}");
        let permitted = stage2_permitted_names(&candidates);
        assert!(permitted.contains("update_play") && permitted.contains("search_nodes"));
    }

    /// The same skill, unpinned and with no score, is not a candidate: the
    /// pin is what clears the gate.
    #[test]
    fn an_unpinned_skill_with_no_score_does_not_clear_the_gate() {
        assert!(!clears_score_gate(&linked(
            "authoring",
            0.0,
            &["update_play"],
            &["play"]
        )));
    }

    /// Retrieval found the pinned skill too: it appears once, with the score
    /// retrieval gave it, marked pinned.
    #[test]
    fn a_pinned_skill_retrieval_also_selected_is_not_repeated() {
        let selected = vec![
            candidate("research", 0.9, &["search_nodes"]),
            linked("authoring", 0.2, &["update_play"], &["play"]),
        ];
        let authoring = pinned(selected[1].clone());

        let candidates = with_pinned_skills(selected, &[authoring], &no_scores());

        assert_eq!(names(&candidates), ["research", "authoring"]);
        assert_eq!(candidates[1].score, 0.2);
        // 0.2 is below a write skill's bar; the pin still makes it eligible.
        assert!(candidates[1].pinned && clears_score_gate(&candidates[1]));
    }

    /// A pinned skill retrieval ranked but did not select keeps that score,
    /// so it leads the turn only when it outscored the others.
    #[test]
    fn a_pinned_skill_keeps_the_score_retrieval_gave_it() {
        let selected = vec![candidate("research", 0.6, &["search_nodes"])];
        let authoring = pinned(linked("authoring", 0.0, &["update_play"], &["play"]));
        let scores = std::collections::HashMap::from([(authoring.id.clone(), 0.8)]);

        let candidates = with_pinned_skills(selected, &[authoring], &scores);

        assert_eq!(candidates[1].score, 0.8);
        assert_eq!(
            leading_tool_bearing_candidate(&candidates).map(|c| c.name.as_str()),
            Some("authoring")
        );
    }

    /// A chat that pins nothing gets exactly what retrieval selected.
    #[test]
    fn no_pinned_skill_leaves_the_candidates_as_they_were() {
        let selected = vec![
            candidate("research", 0.9, &["search_nodes"]),
            candidate("weak", 0.01, &["update_node"]),
        ];
        let candidates = with_pinned_skills(selected.clone(), &[], &no_scores());

        assert_eq!(names(&candidates), names(&selected));
        assert!(candidates.iter().all(|c| !c.pinned));
        assert!(!clears_score_gate(&candidates[1]));
    }

    /// A pinned skill counts toward the offered set like any linked
    /// candidate: alone it holds the turn to its types, and beside an
    /// unlinked tool-bearing skill the turn stays open.
    #[test]
    fn a_pinned_skill_counts_toward_the_offered_set() {
        let authoring = pinned(linked("authoring", 0.0, &["update_play"], &["play"]));
        assert_eq!(
            offered_types(std::slice::from_ref(&authoring)),
            Some(vec!["play".to_string()])
        );

        let with_linked = [
            linked("Triage", 0.9, &["update_node"], &["issue"]),
            authoring.clone(),
        ];
        assert_eq!(
            offered_types(&with_linked),
            Some(vec!["issue".to_string(), "play".to_string()])
        );

        let with_unlinked = [candidate("research", 0.9, &["search_nodes"]), authoring];
        assert_eq!(offered_types(&with_unlinked), None);
    }

    /// A pin does not stand in for the destructive bar: a skill that can
    /// remove user data is offered only when retrieval scored the request as
    /// one for it, pinned or not.
    #[test]
    fn a_pin_does_not_clear_the_destructive_bar() {
        let deletion = pinned(candidate("deletion", 0.0, &["delete_node"]));
        assert!(skill_is_destructive(&deletion));
        assert!(!clears_score_gate(&deletion));
        assert!(stage2_permitted_names(std::slice::from_ref(&deletion)).is_empty());

        let scored = SkillCandidate {
            score: DESTRUCTIVE_SKILL_SCORE_BAR,
            ..deletion
        };
        assert!(clears_score_gate(&scored));
    }

    /// Linked metadata that names no type would make an `enum` nothing
    /// satisfies, so it yields no set rather than an empty one.
    #[test]
    fn linked_metadata_naming_no_type_yields_no_set() {
        assert_eq!(
            offered_types(&[linked("Triage", 0.9, &["update_node"], &[])]),
            None
        );
    }

    #[test]
    fn hold_to_offered_types_puts_the_set_on_tools_that_name_an_existing_type() {
        let offered = vec!["issue".to_string(), "bug".to_string()];
        let held =
            hold_to_offered_types(Tool::ALL.iter().map(|t| t.definition()).collect(), &offered);

        for (tool, def) in Tool::ALL.iter().zip(&held) {
            let unheld = tool.definition();
            match tool.existing_type_parameter() {
                Some(parameter) => assert_eq!(
                    def.parameters_schema["properties"][parameter]["enum"],
                    json!(offered),
                    "{} should hold {parameter} to the offered types",
                    def.name
                ),
                None => assert_eq!(
                    def.parameters_schema, unheld.parameters_schema,
                    "{} names no existing type and must be unchanged",
                    def.name
                ),
            }
            assert_eq!(def.description, unheld.description);
        }
    }
}
