//! ReAct (Reason + Act) loop and session management for the local agent.
//!
//! Orchestrates the conversation cycle: build prompts, call inference,
//! parse tool calls, execute tools, feed results back, and repeat until
//! the model produces a final response or hits iteration limits.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, OnceLock};
use std::time::Instant;

use opentelemetry::trace::{Span, TraceContextExt, Tracer};
use opentelemetry::KeyValue;
use regex::Regex;
use sha2::{Digest, Sha256};
use tokio::sync::RwLock;
use tokio_util::sync::CancellationToken;

use crate::agent_types::{
    AgentSession, AgentToolExecutor, AgentTurnResult, ChatInferenceEngine, ChatMessage,
    ChatModelSpec, InferenceError, InferenceRequest, InferenceUsage, LocalAgentStatus, Role,
    StreamingChunk, ToolCallRaw, ToolExecutionRecord,
};
use crate::local_agent::otlp_tracer::TRACER_NAME;
use crate::local_agent::prompt_templates;
use crate::local_agent::response_processing::{normalize_response, normalize_response_traced};
use crate::local_agent::routing::{self, RouteDecision};
use crate::local_agent::tools::{is_cross_turn_guarded_tool, requires_routed_guidance_tool};
use crate::prompt_assembler::{PromptAssembler, TemplateContext, EMERGENCY_FALLBACK_PROMPT};

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Maximum number of tool-call iterations per turn.
const MAX_TOOL_ITERATIONS: usize = 5;

/// Consecutive tool calls with unparseable JSON arguments tolerated before the
/// turn gives up and produces a final response from what it already has.
///
/// Two, not one: a single malformed call followed by a correct retry is normal
/// recovery and observed in practice, so tripping on the first would abort turns
/// that were about to succeed.
const MAX_CONSECUTIVE_PARSE_FAILURES: usize = 2;

/// Token ceiling for the Stage-1 routing turn.
///
/// Stage 1 emits one short tool call — a search query, or a question with a
/// couple of options — and nothing else. A tight ceiling bounds the latency
/// the extra turn adds and gives a runaway generation nowhere to go.
pub const STAGE1_MAX_TOKENS: u32 = 256;

/// The Stage-1 system prompt.
///
/// Deliberately small. Stage 1 makes one structural choice and the two tool
/// schemas carry the shape of that choice, so prose here would duplicate the
/// channel that already decides it — the failure ADR-064 rule 5 names. It says
/// who to be, what the single decision is, and that clarifying is the
/// exception.
pub const STAGE1_SYSTEM_PROMPT: &str =
    "You are routing a user's request to the right capability.\n\
    Call route_query with a short description of the capability the request needs.\n\
    Call route_multi ONLY if the request contains two or more distinct, unambiguous things to do \
    — not one thing phrased at length.\n\
    Call route_clarify ONLY if the request is too ambiguous to describe at all.\n\
    Prefer route_query: most requests can be described even when phrased indirectly, and most \
    requests are a single intent even when they mention several details.\n\
    Call exactly one tool. Do not answer the user.";

/// Opening phrase of a routing clarification.
///
/// The session is rebuilt from persisted messages every turn, so the
/// clarification contract ("at most one per intent") cannot rely on in-memory
/// state — whether we already clarified has to be answerable from the history
/// alone. Clarifications are composed here and always open with this phrase,
/// so their own text is the record. Matching on text is imprecise in general,
/// but it is exact for strings this module wrote itself, and it avoids
/// widening the persisted session shape for one boolean.
const CLARIFICATION_OPENER: &str = "I can take that a couple of ways";

/// Longest canonical-args string stored verbatim as a completed write's identity.
///
/// `create_nodes_from_markdown` carries an entire import in its arguments, and
/// the identity is persisted into the chat node's own message history — so
/// storing them verbatim would write that content a second time and grow the
/// node without bound. Past this length [`canonical_args_identity`] substitutes
/// a digest, which keeps the identity at constant size.
///
/// Note this is a *representation* threshold, not a guard-coverage one: a call
/// above it is still guarded. Truncation, by contrast, would not be safe — a
/// truncated string could compare equal to a *different* call sharing a long
/// prefix, turning a size limit into a wrong-suppression bug. A digest has no
/// such failure mode, which is why it is the form used above the cap.
pub const CANONICAL_ARGS_MAX_CHARS: usize = 4096;

/// Prefix marking an identity that is a digest rather than literal canonical
/// args. Canonical JSON always begins with `{`, so the two forms are mutually
/// unambiguous and a digest can never be mistaken for a call's real arguments.
const CANONICAL_ARGS_DIGEST_PREFIX: &str = "sha256:";

/// Normalise a tool call's raw JSON arguments so equal calls compare equal.
///
/// Round-tripping through serde sorts nothing by itself, but it does normalise
/// whitespace and re-serialises `serde_json::Value`'s `BTreeMap`-backed objects
/// in sorted key order, so `{"b":1,"a":2}` and `{"a":2,"b":1}` produce the same
/// string. Unparseable arguments are returned unchanged: they cannot be
/// normalised, and an exact-match comparison on the raw text is still correct.
///
/// Also resolves the `node_id` → `id` parameter alias (see the `serde(alias)`
/// attributes in `tools.rs`). Serde applies that alias when *deserialising into
/// a params struct*, which happens after this function runs — so without this
/// step `{"id":"x"}` and `{"node_id":"x"}` would yield two different identities
/// for one logical call, and a repeat that switched spelling would slip the
/// guard. Normalising here rather than per-tool means any future tool adopting
/// the same alias is covered without a matching fix.
///
/// Shared by the per-turn duplicate detector and the cross-turn write guard, so
/// the normalisation rules cannot drift apart. Note the two feed it different
/// inputs: the loop-breaker canonicalises the raw emitted text, while the guard
/// canonicalises the parsed arguments to match what the write record persists.
/// That difference is deliberate — the loop-breaker only needs to recognise a
/// stuck model, whereas the guard's comparison must be exact.
/// Truncate `s` to at most `max_chars` characters (char-boundary safe via
/// `.chars()`, unlike a `[..N]` byte slice which can panic mid-character on
/// any non-ASCII content) for a log/span preview. Returns the preview and
/// whether truncation actually happened, so a caller can mark it — a
/// preview that silently reads as the complete value has repeatedly cost
/// real diagnosis time: the log line looks complete, and nothing says
/// otherwise.
fn char_preview(s: &str, max_chars: usize) -> (String, bool) {
    let mut chars = s.chars();
    let preview: String = chars.by_ref().take(max_chars).collect();
    let truncated = chars.next().is_some();
    (preview, truncated)
}

pub fn canonical_args(args_json: &str) -> String {
    serde_json::from_str::<serde_json::Value>(args_json)
        .map(|mut v| {
            normalize_param_aliases(&mut v);
            // Repaired before the identity is taken so a malformed call and its
            // repaired retry share one identity. Otherwise the duplicate guards
            // would read them as two different calls and the loop this fix
            // exists to break would still run, just one round longer.
            //
            // Not redundant with the repair on the execution path: this one acts
            // on the raw emitted text for the per-turn duplicate detector, that
            // one mutates the arguments actually handed to the tool. Neither
            // covers the other's caller, so removing either leaves a live gap.
            //
            // Through the shared entry point rather than re-listing the repairs
            // here: the set was previously spelled out twice, so a repair added
            // for a newly observed malformation reached the execution path but
            // silently not the identity, and the two sides would disagree about
            // whether a call and its retry were the same call.
            //
            // Note what changing this set implies. This identity is PERSISTED by
            // the cross-turn write guard, so adding a repair means identities
            // recorded before the change no longer match the ones computed after
            // it, and a write guarded under the old spelling is not recognised
            // under the new one. That is acceptable here — the window is one
            // conversation and the failure mode is a duplicate-write prompt the
            // model can still resolve — but it is a real consequence, not a pure
            // refactor, and a future repair added to the set inherits it.
            repair_parsed_tool_arguments(&mut v);
            v.to_string()
        })
        .unwrap_or_else(|_| args_json.to_owned())
}

/// Rewrite aliased top-level parameter keys to the name the params struct uses.
///
/// Applied to every tool's arguments rather than per-tool. `node_id` is the only
/// alias in the tool registry and always means `id`, so a blanket rule is both
/// correct and self-maintaining — a future tool adopting the same alias needs no
/// matching fix here. A tool with no such parameter is unaffected in practice,
/// and even a stray `node_id` is renamed identically on both sides of the
/// comparison, so the identity stays consistent either way.
///
/// Only the top level: a nested `field_values` blob is the user's own data,
/// where a key named `node_id` means whatever the user's schema says and must
/// not be renamed. When a call carries *both* spellings the object is left
/// untouched —
/// serde's own precedence decides which wins, and picking one here could make
/// two genuinely different calls compare equal, which is the one failure this
/// guard must never have.
fn normalize_param_aliases(args: &mut serde_json::Value) {
    let Some(obj) = args.as_object_mut() else {
        return;
    };
    if obj.contains_key("node_id") && !obj.contains_key("id") {
        if let Some(v) = obj.remove("node_id") {
            obj.insert("id".to_string(), v);
        }
    }
}

/// Apply every argument repair to an already-parsed `Value`, in place.
///
/// The one place the set of malformations is listed. Both callers — the parse
/// boundary (via [`repair_tool_call_arguments`]) and the execution site's
/// backstop — go through here, so a repair added for a newly observed
/// malformation reaches both without having to be remembered twice.
fn repair_parsed_tool_arguments(args: &mut serde_json::Value) {
    repair_over_quoted_keys(args);
    repair_leaked_special_token_keys(args);
    repair_spliced_object_values(args);
    repair_scalar_in_operator_values(args);
}

/// Strip literal quote characters that the model wrapped around its own JSON
/// keys, e.g. the key `"\"name\""` (six characters, quotes included) where
/// `name` was meant.
///
/// This is repair, not interpretation. The malformation is purely mechanical —
/// the intended key is the same string with a leading and trailing `"` removed —
/// so the repair is unambiguous, and it is applied only when the trimmed name
/// does not itself contain a quote and the object has no genuine key by that
/// name to collide with. Anything outside that shape is left exactly as sent.
///
/// Applied to every tool's arguments, at every nesting depth, rather than to the
/// one tool where it was first observed. The cause is not tool-specific: a
/// rejected call stays in the conversation as an assistant turn, and the model
/// reproduces the malformed shape it reads there on every subsequent retry. That
/// was measured against `gemma-4-e4b-q4km` — a well-formed prior call yields a
/// well-formed retry in 8 of 8 trials, and a malformed one yields a malformed
/// retry in 8 of 8 — so the shape, not the tool, is what propagates. Any tool
/// taking an array of objects is reachable the same way.
///
/// Deliberately recurses where [`normalize_param_aliases`] deliberately does
/// not. That function stays top-level because a nested `field_values` blob is
/// the user's own data, where a key means whatever the user's schema says; this one
/// must reach exactly that data, because the malformation only ever appears
/// nested — inside `fields`, `add_fields`, or any other array of objects — and a
/// top-level-only rule would decline every case it exists for. The divergence is
/// accepted rather than overlooked: the two functions answer different
/// questions. An alias rewrite guesses at intent and could rename a key the user
/// meant, so it stays narrow; this one acts only on a key that is not
/// expressible as a deliberate choice, since a JSON key carrying its own quote
/// marks cannot be written by the schema path that produced it. The residual
/// risk is a renamed key, never a dropped value — the collision guard below
/// declines rather than overwrite.
///
/// Repairing here rather than in each tool's validator matters for the same
/// reason: the loop's duplicate guards and the tool executor both read the
/// arguments this produces, so a call repaired once is repaired for every
/// consumer, and the poisoned shape never re-enters the history to be copied
/// again.
fn repair_over_quoted_keys(args: &mut serde_json::Value) {
    match args {
        serde_json::Value::Object(obj) => {
            let over_quoted: Vec<String> = obj
                .keys()
                .filter(|k| {
                    let Some(inner) = k.strip_prefix('"').and_then(|rest| rest.strip_suffix('"'))
                    else {
                        return false;
                    };
                    // An empty or still-quoted inner name is not a shape this can
                    // claim to understand, and a collision with a real key would
                    // silently discard one of the two values.
                    !inner.is_empty() && !inner.contains('"') && !obj.contains_key(inner)
                })
                .cloned()
                .collect();
            for key in over_quoted {
                if let Some(value) = obj.remove(&key) {
                    let inner = key[1..key.len() - 1].to_string();
                    obj.insert(inner, value);
                }
            }
            for value in obj.values_mut() {
                repair_over_quoted_keys(value);
            }
        }
        serde_json::Value::Array(items) => {
            for item in items {
                repair_over_quoted_keys(item);
            }
        }
        _ => {}
    }
}

/// Strip Gemma 4's leaked quote-token text (`<|"|>`) from a tool call's JSON
/// keys, e.g. the key `<|"|>type<|"|>` where `type` was meant.
///
/// #1926: grammar-constrained decoding was observed emitting this literal
/// token text in place of nothing (the surrounding real `"` characters the
/// model also wrote are the actual delimiters) around a key inside
/// `create_schema`'s `fields` array — `{"<|"|>type<|"|>":"number",...}` parses
/// as valid JSON with this garbled key, so no parse-failure guard catches it;
/// the tool then rejects the call for a missing `"type"` field. Confirmed via
/// unit test that this specific token was reaching a fully-streamed tool
/// call's arguments unmodified even after a per-delta strip in the inference
/// engine — the token can straddle a delta boundary (one chunk ending `<|`,
/// the next starting `"|>`), where neither half alone contains the substring
/// a per-delta strip needs to match. Operating here, on the parsed `Value`
/// after the full argument string has been concatenated, is immune to where
/// that boundary happened to fall.
///
/// Deletion, not substitution: the literal quote characters bracketing the
/// leaked token in the model's own output are the real delimiters, so
/// removing the leaked text restores the intended key rather than doubling
/// quotes. Same recursion shape as [`repair_over_quoted_keys`] and applied
/// alongside it, for the same reason — the malformation is not
/// `create_schema`-specific, and a rejected call's malformed shape re-enters
/// the conversation history for the model to copy on retry.
fn repair_leaked_special_token_keys(args: &mut serde_json::Value) {
    const LEAKED_QUOTE_TOKEN: &str = "<|\"|>";
    match args {
        serde_json::Value::Object(obj) => {
            let corrupted: Vec<String> = obj
                .keys()
                .filter(|k| k.contains(LEAKED_QUOTE_TOKEN))
                .cloned()
                .collect();
            for key in corrupted {
                let repaired = key.replace(LEAKED_QUOTE_TOKEN, "");
                // An empty or already-present repaired name is not a shape
                // this can claim to understand — a collision would silently
                // discard one of the two values, same guard as
                // repair_over_quoted_keys.
                if repaired.is_empty() || obj.contains_key(&repaired) {
                    continue;
                }
                if let Some(value) = obj.remove(&key) {
                    obj.insert(repaired, value);
                }
            }
            for value in obj.values_mut() {
                repair_leaked_special_token_keys(value);
            }
        }
        serde_json::Value::Array(items) => {
            for item in items {
                repair_leaked_special_token_keys(item);
            }
        }
        _ => {}
    }
}

/// Apply every argument repair to a tool call's raw JSON string, in place.
///
/// The single entry point for repair, so the set of malformations handled
/// cannot drift between the record that enters history and the arguments handed
/// to the tool. Arguments that do not parse are left exactly as sent: they
/// cannot be repaired structurally, and the parse-failure path downstream
/// reports the malformed JSON itself, which is the honest error.
///
/// Rewrites the string only when a repair actually changed something, so a
/// well-formed call keeps its original byte-for-byte text rather than being
/// silently re-serialised into serde's key order.
/// Public so live-model tests that drive the inference engine directly — with
/// no `LocalAgentLoop` in the path — can apply the same repair production
/// applies, and therefore measure what a real turn would do rather than what an
/// unrepaired raw call does.
pub fn repair_tool_call_arguments(arguments_json: &mut String) {
    let Ok(mut value) = serde_json::from_str::<serde_json::Value>(arguments_json) else {
        return;
    };
    let before = value.clone();
    repair_parsed_tool_arguments(&mut value);
    if value != before {
        *arguments_json = value.to_string();
    }
}

/// Recover the intended value from a string that swallowed the JSON delimiters
/// following it, e.g. the value `task","value":` where `task` was meant.
///
/// #1943: observed inside `search_nodes`' `filters` array — the model emitted
/// `"type":"task\",\"value\":` , writing `"`, `,` and `:` as *content* where
/// they were meant as structure. The result is still valid JSON, so no parse
/// guard fires; the filter simply loses its `value` and is rejected for an
/// unknown filter type.
///
/// Only the swallowed key's *name* survives in the text — the model never
/// emitted its value — so this restores the truncated value (`task`) and drops
/// the fragment. It deliberately does **not** invent a value for the swallowed
/// key: the filter still fails validation, but it fails naming the field that
/// is actually missing rather than reporting a nonsense type, and the repaired
/// shape is what enters history for the retry to copy.
///
/// Narrow by construction: it fires only on a string whose entire tail is the
/// exact `","<name>":` delimiter run, with no further quotes in either part. A
/// value that merely contains a quote — user prose, embedded JSON — does not
/// match and is left untouched, the same bar the sibling key repairs hold to.
///
/// That shape test is necessary but not sufficient, because unlike the sibling
/// key repairs this one rewrites *values*, and some values are the user's own
/// authored text rather than the model's structural choices. A pasted CSV header
/// fragment (`name","email":`) matches the mechanical shape exactly and would be
/// silently truncated to `name`. So `content` and `field_values` subtrees are
/// skipped entirely at any depth: those carry user data verbatim, where a string
/// means whatever the user wrote and no repair is this function's to make. The
/// malformation this exists for appears in the model's *structural* slots —
/// `filters`, `fields` — which remain covered.
///
/// Note what that safety rests on: an enumerated list of user-data keys, i.e. a
/// denylist. It covers every parameter the current tools expose, but a tool
/// added later with a free-text parameter under some other name (`body`,
/// `note`, `summary`) would silently fall back inside the blast radius. Whoever
/// adds one must extend the list here. Inverting to an allowlist of structural
/// slots would fail safe instead, and is the better shape if this list ever
/// grows past a couple of entries.
fn repair_spliced_object_values(args: &mut serde_json::Value) {
    match args {
        serde_json::Value::Object(obj) => {
            for (key, value) in obj.iter_mut() {
                // User-authored text: not the model's structure to repair.
                if key == "content" || key == "field_values" {
                    continue;
                }
                if let serde_json::Value::String(s) = value {
                    if let Some(repaired) = strip_spliced_delimiter_tail(s) {
                        *value = serde_json::Value::String(repaired);
                        continue;
                    }
                }
                repair_spliced_object_values(value);
            }
        }
        serde_json::Value::Array(items) => {
            for item in items {
                repair_spliced_object_values(item);
            }
        }
        _ => {}
    }
}

/// Split `value","key":` into its intended value, or return `None` when the
/// string is not exactly that shape.
fn strip_spliced_delimiter_tail(s: &str) -> Option<String> {
    let rest = s.strip_suffix("\":")?;
    let (value, key) = rest.split_once("\",\"")?;
    // A quote in either half means this is not the mechanical splice above but
    // some richer string the model meant, so it is not this function's to touch.
    // An empty key name is likewise not a shape that can be claimed understood.
    if value.contains('"') || key.contains('"') || key.is_empty() {
        return None;
    }
    Some(value.to_string())
}

/// Give the `in` operator the array it requires, when the model sent a scalar
/// string instead — e.g. `{"operator":"in","property":"stage","value":"cut,soak"}`
/// where `["cut","soak"]` was meant.
///
/// #2182. Be precise about what this is and is not, because the issue that
/// prompted it overstated both the symptom and the cause, and the measurements
/// are the reason to keep this narrow rather than grow it.
///
/// **It is a backstop, not the mechanism.** What actually gets `in` an array is
/// `search_nodes`' own declaration of `value` — the type union naming `array`,
/// in `tools.rs`. Two ablation arms differing in only that declaration
/// (`goldens/ablation/in-operator-array-elicited` and `-string-declared`), 3
/// reps each and byte-identical within each arm, emit `["cut","soak"]` and
/// `"cut,soak"` respectively. Production already carries the union, so the
/// malformation the issue reported was the model complying with a *corpus*
/// case that asked for a string. This repair exists because a declaration is
/// not a guarantee, and it is measured converting real model output on the
/// arm that reproduces the failing shape — not because production was
/// standing broken.
///
/// **The symptom is loud, not silent.** A scalar `in` value is not a
/// zero-result indistinguishable from a genuinely empty search:
/// `QueryService::build_filter_condition` rejects it outright ("In requires
/// array value"), so the turn fails and the model must recover from an error it
/// caused. Worth removing, but it never produced a wrong answer the user could
/// not see, and this must not be credited with preventing one.
///
/// **Why a transform and not more prose.** `value`'s correct shape depends on
/// the value of a *sibling* field (`operator`), which nothing in JSON Schema
/// expresses here. Two channels already state the rule in words; a third
/// statement was measured to change nothing on `dev-unseen-schema` (3 of 3 reps
/// byte-identical) while making a sibling case worse. Restating it as a
/// transform on emitted arguments costs no prompt bytes and therefore cannot
/// regress a sibling case at all.
///
/// Mechanical and unambiguous, the same bar the sibling repairs hold to. `in`
/// means "matches any of these", a comma is the only separator the wire format
/// leaves available inside a JSON string, and the operator naming the array
/// shape is right there in the same object — so the intended value is the split.
/// A string with no comma is wrapped as a one-element array rather than left
/// alone, and the reason is semantic rather than schema conformance: `IN (x)`
/// and `= x` select the same rows. So even if the model reached for `in` when it
/// meant `equals`, the wrap produces a query with the same result set — nothing
/// is masked that would have produced a different answer — whereas leaving it a
/// bare string just fails the way this exists to prevent.
///
/// Deliberately narrower than its siblings in where it looks, and narrowed by an
/// ALLOWLIST rather than by the shape it matches.
///
/// The tempting version keys on `operator == "in"` alone and recurses
/// everywhere, on the reasoning that `operator` is a filter-item token so user
/// data can never carry it. That reasoning is wrong, and this comment used to
/// make the claim: `field_values` holds the fields of a *user-defined* schema,
/// and `create_schema` reserves no field names — so a type with fields named
/// `operator` and `value` produces `{"operator":"in","value":"Acme Corp, Ltd"}`
/// inside a WRITE tool's payload, and a shape-keyed repair silently rewrites the
/// user's own data to `["Acme Corp","Ltd"]`. Verified against the real function
/// before this was changed, not reasoned about.
///
/// So the entry point descends only into `filters`, the one parameter whose
/// contents are the model's structural choices rather than the user's data. That
/// is the inversion `repair_spliced_object_values`' own comment recommends for
/// when a denylist would otherwise have to grow: a tool added later with a
/// free-text parameter is out of range automatically, instead of being in range
/// until someone remembers to exclude it.
fn repair_scalar_in_operator_values(args: &mut serde_json::Value) {
    let Some(obj) = args.as_object_mut() else {
        return;
    };
    if let Some(filters) = obj.get_mut("filters") {
        repair_in_operator_values_within_filters(filters);
    }
}

/// Apply the `in`-value repair inside the `filters` subtree.
///
/// Recursive within that subtree only. The nesting it has to cross today is just
/// `filters` → array → item, but walking it generally costs nothing and keeps a
/// future grouped or nested filter shape covered without a matching fix here.
fn repair_in_operator_values_within_filters(filters: &mut serde_json::Value) {
    match filters {
        serde_json::Value::Object(obj) => {
            let is_in_filter = obj
                .get("operator")
                .and_then(|v| v.as_str())
                .is_some_and(|op| op == "in");
            if is_in_filter {
                if let Some(serde_json::Value::String(s)) = obj.get("value") {
                    if let Some(values) = split_in_operator_values(s) {
                        obj.insert("value".to_string(), serde_json::Value::Array(values));
                    }
                }
            }
            for value in obj.values_mut() {
                repair_in_operator_values_within_filters(value);
            }
        }
        serde_json::Value::Array(items) => {
            for item in items {
                repair_in_operator_values_within_filters(item);
            }
        }
        _ => {}
    }
}

/// Split an `in` operator's scalar value into the array members it stands for,
/// or return `None` when there is nothing this can honestly claim to recover.
///
/// Surrounding whitespace is trimmed per member, because `"cut, soak"` is the
/// same intent as `"cut,soak"` and the space is punctuation the model added, not
/// part of a stored value. Empty members are dropped for the same reason — a
/// trailing comma is a typo, not a filter on the empty string. A value that
/// yields no members at all (empty, or only separators) is left exactly as sent:
/// there is no intended list in it to recover, and an empty `IN ()` would match
/// nothing while looking like a working filter, which is the one outcome this
/// must not manufacture.
///
/// THE TRADEOFF, stated because it is real and cuts the other way from the bug
/// this fixes: a single stored value that legitimately contains a comma
/// (`"Acme, Inc."`) is split into two members that match nothing. That failure
/// is SILENT — well-formed SQL returning zero rows — where the malformation
/// being repaired is loud. It is accepted on the grounds that a comma is the
/// only separator a JSON string leaves available for a list, so the split is the
/// only reading available; that the caller-side allowlist keeps this inside
/// `filters`, where the values are enum members and identifiers rather than
/// prose; and that the alternative is failing every multi-value filter to
/// protect a comma-bearing one. Reconsider it if filters ever routinely compare
/// against free text — the honest fix then is a real array on the wire, not a
/// cleverer split.
fn split_in_operator_values(s: &str) -> Option<Vec<serde_json::Value>> {
    let values: Vec<serde_json::Value> = s
        .split(',')
        .map(str::trim)
        .filter(|part| !part.is_empty())
        .map(|part| serde_json::Value::String(part.to_string()))
        .collect();
    (!values.is_empty()).then_some(values)
}

/// The identity persisted for a call's canonical args.
///
/// Under [`CANONICAL_ARGS_MAX_CHARS`] this is the canonical JSON verbatim, so a
/// stored identity stays readable when diagnosing a refusal. Above it, a digest
/// of that same string — exact-match equality is all the guard needs, so hashing
/// preserves the guarantee at constant size instead of dropping the identity and
/// leaving the largest, most costly duplicates unguarded.
///
/// Both sides of the comparison must derive the identity through this one
/// function: the record side when persisting, the execution path when checking.
/// Deriving either independently is what would let the two drift apart.
pub fn canonical_args_identity(canonical: &str) -> String {
    if canonical.chars().count() <= CANONICAL_ARGS_MAX_CHARS {
        return canonical.to_owned();
    }
    format!(
        "{CANONICAL_ARGS_DIGEST_PREFIX}{:x}",
        Sha256::digest(canonical.as_bytes())
    )
}

/// Build the tool result returned in place of a refused duplicate write.
///
/// Deliberately informative rather than a bare failure. A user genuinely
/// re-asking for the same node is rare but real, so the model must be able to
/// read this, tell the user the thing already exists, and name it — or, if the
/// repeat is intended, proceed deliberately with a call that differs.
fn duplicate_write_result(prior: &crate::agent_types::PriorWrite) -> serde_json::Value {
    let mut what = prior.tool.clone();
    if let Some(ref s) = prior.summary {
        what.push_str(&format!(" \"{s}\""));
    }
    serde_json::json!({
        "skipped": "duplicate_write",
        "id": prior.node_id,
        "message": format!(
            "Not executed: an identical {what} call already completed earlier in this \
             conversation, so this would create a second copy. The result of that write \
             still stands{}. Tell the user it already exists rather than repeating the \
             write. If they explicitly want another, separate copy, say so and issue a \
             call that differs from the original.",
            match prior.node_id {
                Some(ref id) => format!(" ({id})"),
                None => String::new(),
            }
        ),
    })
}

/// Whether `all_tool_executions` (this turn's history) already contains a
/// successful `create_schema` call.
///
/// Structural backstop for `ONE_SCHEMA_PER_REQUEST`: nothing in the tool
/// surface stops the model from calling `create_schema` a second time within
/// the same skill invocation (Schema Creation's `max_iterations: 3` permits
/// it, and `stage2_tools` does not enforce call counts). Split out so the
/// guard site and its test can share the exact same notion of "already
/// created a schema this turn."
fn schema_already_created_this_turn(executions: &[ToolExecutionRecord]) -> bool {
    executions
        .iter()
        .any(|r| r.name == "create_schema" && !r.is_error)
}

/// Build the tool result returned in place of a refused second `create_schema`
/// call within one skill invocation.
///
/// Mirrors [`duplicate_write_result`]'s shape (informative, not a bare
/// failure) but is flagged as an error: unlike a duplicate write, this is a
/// genuine policy violation (`ONE_SCHEMA_PER_REQUEST`) that the model should
/// stop and report rather than retry differently.
fn second_schema_refused_result() -> serde_json::Value {
    serde_json::json!({
        "error": "second_schema_in_one_request",
        "message": "Not executed: a schema was already created earlier in this request. \
             Create exactly one type per request — do not also create a related type the \
             user didn't ask for. Stop here and report the schema that was already created. \
             If the user's request genuinely named two separate types, tell them the second \
             one needs its own follow-up request.",
    })
}

/// Maximum tokens any single inference round may generate.
///
/// Small local models (e.g. Gemma-4-E4B) occasionally open an empty
/// assistant turn with nothing to say and then run away to the model's
/// hard ceiling, producing a multi-minute hang that surfaces to the user
/// as "no reply". A tight cap bounds any such runaway to a couple of
/// seconds while still leaving ample room for a normal chat reply or a
/// tool-call argument blob.
///
/// Maximum tokens for the final text-only response (no tools). Keeps
/// user-facing replies concise. Tool-calling iterations use `max_tokens: None`
/// so argument JSON is never truncated mid-field.
const MAX_RESPONSE_TOKENS: u32 = 2_048;

/// Total token budget for the context window.
const TOTAL_TOKEN_BUDGET: u32 = 32_000;

/// Tokens reserved for the system prompt and tool definitions.
const SYSTEM_PROMPT_BUDGET: u32 = 4_000;

/// Stability-motivated prefill ceiling, independent of the context-window
/// budget above.
///
/// One measured run hit a Metal command-buffer OOM during prefill on a 16 GB
/// machine at an 8,518-token **total** prompt (system prompt + history,
/// 35.5 KB), with `n_ctx` granted at 32,768 — i.e. the context-window budget
/// (`total_budget` in `maybe_summarize_history`, effectively ~30K tokens on
/// that machine) was nowhere near full when the *backend* failed. A prompt
/// can be well within the model's context window and still be large enough
/// to risk exhausting Metal's transient compute-buffer allocation during a
/// single prefill. This gate therefore compares against the same
/// `history_tokens + system_tokens` total gate 1 uses (the full prompt
/// actually submitted to `llama_decode`), not history alone — the
/// tool-registered system prompt is itself ~6,600 tokens (see gate 1's doc
/// comment), so a history-only comparison would never reflect what Metal
/// actually decodes.
///
/// A later attempt to reproduce that OOM on matching 16 GB hardware to
/// measure the actual failure threshold (with `powermetrics` GPU telemetry)
/// could not: three full eval-matrix runs, including prompts up to
/// 9,469 tokens — larger than the original failure — decoded cleanly with no
/// backend error. The original OOM's exact trigger (allocator fragmentation
/// state, concurrent GPU clients, thermal state, or plain Metal
/// nondeterminism) remains unmeasured.
///
/// **This constant is therefore explicitly provisional**, not derived from a
/// validated safe/unsafe boundary: it sits below the observed 8,518-token
/// failure point with margin (and above the ~6,600-token system prompt
/// alone, so a turn with little history does not spuriously trigger
/// summarization every time), on the reasoning that giving up context-window
/// headroom that mostly goes unused (the shared-chat-node pattern that
/// triggered the original OOM rarely needs more than a few thousand tokens of
/// live history to function) is a cheap hedge against a failure mode whose
/// recovery is now loud but still costs a turn. Revise this once a real
/// repro pins the actual Metal failure threshold.
const PREFILL_STABILITY_CEILING: u32 = 8_000;

/// Last-resort user-facing message when a turn produces nothing usable — no
/// tool executions to summarize and no (non-empty, non-pseudo-code) text from
/// the model. Guarantees the chat UI always shows an honest failure notice
/// rather than an empty bubble.
const EMPTY_RESPONSE_FALLBACK: &str =
    "⚠️ I wasn't able to produce a response for that. Please try again.";

/// Shared confirmation request used when a guard suppresses a response the model
/// should not have produced — a fabricated action claim, or a tool call the
/// model narrated as text instead of invoking. Kept in one place so the two
/// guards stay in sync.
const CONFIRMATION_REQUEST: &str =
    "I'd like to help with that. Could you confirm what you'd like me to do? I want to make sure I take the right action.";

fn session_prompt_override(session: &AgentSession) -> Option<&str> {
    session.system_prompt_override.as_deref()
}

/// What Stage 1 and the retrieval step produced for Stage 2 to work with.
#[derive(Default)]
struct RoutingOutcome {
    /// Candidates for Stage 2 to judge. Empty means routing produced nothing —
    /// whether because retrieval was unavailable, matched nothing, or failed —
    /// and the turn runs on the full tool surface.
    candidates: Vec<crate::agent_types::SkillCandidate>,
    /// A composed clarification to return instead of running Stage 2. `Some`
    /// only when Stage 1 asked to clarify *and* the contract allowed it.
    ///
    /// This is the flattened text `format_clarification` produces — still the
    /// form persisted into `session.messages` for the LLM-facing history and
    /// `session_already_clarified`'s scan. [`Self::clarify_prompt`] carries the
    /// same question/options unflattened, for the frontend.
    clarification: Option<String>,
    /// The same clarification as structured data, alongside the flattened
    /// `clarification` string above. `Some` exactly when `clarification` is.
    clarify_prompt: Option<crate::agent_types::ClarifyPrompt>,
    /// Tokens spent on the Stage-1 turn, so the turn's reported usage covers
    /// the whole two-stage flow rather than under-reporting it.
    usage: InferenceUsage,
}

/// Build the message Stage 1 routes on: prior turns blended with the current one.
///
/// `run_turn` pushes the current user message into `session.messages` before
/// routing, so the trailing entry is excluded here and supplied separately —
/// blending it twice would double-weight it against the history it is meant to
/// be read in light of.
///
/// Only user/assistant turns are blended, matching `schema_retrieval_query`:
/// tool results are machine payloads whose vocabulary would dilute the query
/// rather than sharpen it.
///
/// Delegates to [`stage1_query_from_turns`], which does the actual framing —
/// kept as a free function (not inlined here) so `agent_loop.rs`'s own
/// golden-prompt test harness can build the exact same Stage-1 message from
/// `prior_turns: &[&str]` without constructing an `AgentSession`.
fn stage1_query(session: &AgentSession, user_message: &str) -> String {
    let prior = session
        .messages
        .split_last()
        .map(|(_, rest)| rest)
        .unwrap_or(&[]);
    let prior_turns: Vec<&str> = prior
        .iter()
        .filter(|m| matches!(m.role, Role::User | Role::Assistant))
        .map(|m| m.content.as_str())
        .collect();
    stage1_query_from_turns(&prior_turns, user_message)
}

/// Build the Stage-1 chat message from prior turns and the current message.
///
/// When prior turns exist, the blended text is wrapped with an explicit
/// PRIOR CONTEXT / CURRENT REQUEST boundary rather than handed to the model
/// as one undifferentiated blob. `build_retrieval_query`'s raw
/// `parts.join("\n")` output is tuned for embedding recall (measured:
/// extraction/reformatting measurably hurts it), but Stage 1 is not an
/// embedding call — it is a chat turn where the model decides `route_query`
/// vs `route_multi` vs `route_clarify`, and an unlabeled multi-line blob
/// reads to the model like several things to do, not one request in light of
/// history. Confirmed live: `route_multi` fired on genuinely
/// single-intent turns, fabricating a second "intent" out of blended prior
/// content. The wrapper only changes what the Stage-1 *chat* message looks
/// like; `build_retrieval_query`'s own output, used verbatim for schema
/// retrieval, is untouched.
pub fn stage1_query_from_turns(prior_turns: &[&str], user_message: &str) -> String {
    if prior_turns.is_empty() {
        return user_message.trim().to_string();
    }

    let blended =
        nodespace_core::ops::context_ops::build_retrieval_query(prior_turns, user_message);
    let current = user_message.trim();
    // `blended` is prior turns + current message joined by "\n", current
    // last. Split it back apart at that known boundary rather than
    // re-deriving the trim/cap logic here, so the two constructions cannot
    // drift apart in what text they contain — only in how it is framed.
    let prior_blended = blended
        .strip_suffix(current)
        .map(|s| s.trim_end_matches('\n'))
        .unwrap_or(&blended);

    if prior_blended.is_empty() {
        current.to_string()
    } else {
        format!("PRIOR CONTEXT (for background only — do not treat as part of the current request):\n{prior_blended}\n\nCURRENT REQUEST (route this):\n{current}")
    }
}

/// Whether this conversation already asked the user to clarify.
///
/// Enforces ADR-038's "at most one clarification per intent": if the user
/// answered a clarification and the request still cannot be routed, asking
/// again is the loop the contract exists to prevent — the turn falls through
/// to retrieval instead.
///
/// Scoped to the **current intent**, not the whole conversation. ADR-038 says
/// "at most one clarification per intent", and a conversation is many intents:
/// a clarification answered twenty turns ago must not stop a genuinely new
/// ambiguous request from being clarified today.
///
/// The intent boundary is an assistant turn that *answered* rather than asked:
/// once the agent has resolved something and replied normally, whatever the
/// user says next starts a fresh intent and the mechanism re-arms.
fn session_already_clarified(session: &AgentSession) -> bool {
    // Everything from the last resolved turn onward is the current intent.
    // A clarification is not a resolution, so it does not close an intent —
    // that is what lets the "already clarified" state survive the user's reply
    // to it, while still clearing once the turn actually completes.
    let intent_start = session
        .messages
        .iter()
        .rposition(|m| {
            matches!(m.role, Role::Assistant) && !m.content.starts_with(CLARIFICATION_OPENER)
        })
        .map_or(0, |idx| idx + 1);

    let current_intent = &session.messages[intent_start..];

    // Within this intent: did we ask, and has the user since replied? A
    // clarification with no user message after it is the one we just asked and
    // are still waiting on, not a second attempt.
    current_intent
        .iter()
        .position(|m| {
            matches!(m.role, Role::Assistant) && m.content.starts_with(CLARIFICATION_OPENER)
        })
        .is_some_and(|idx| {
            current_intent[idx + 1..]
                .iter()
                .any(|m| matches!(m.role, Role::User))
        })
}

/// Compose a clarification from Stage 1's question and options.
///
/// ADR-038 requires clarification be *specific*: it surfaces the concrete
/// candidates the model already produced, turning a dead end into a one-tap
/// disambiguation. A bare "what do you mean?" is the failure mode to avoid,
/// so the options are rendered as an explicit list when the model supplied
/// them.
fn format_clarification(question: &str, options: &[String]) -> String {
    let usable: Vec<&String> = options.iter().filter(|o| !o.trim().is_empty()).collect();
    if usable.is_empty() {
        return format!("{CLARIFICATION_OPENER}. {question}");
    }
    let listed = usable
        .iter()
        .map(|o| format!("- {o}"))
        .collect::<Vec<_>>()
        .join("\n");
    format!("{CLARIFICATION_OPENER}. {question}\n\n{listed}")
}

// ---------------------------------------------------------------------------
// Tool name humanization
// ---------------------------------------------------------------------------

/// Convert an internal tool identifier into user-facing prose.
///
/// Used by fallback responses that surface tool activity to the chat UI when
/// the model fails to produce its own text. The display label is derived from
/// the tool registry ([`crate::local_agent::tools::Tool`]); names not in the
/// registry fall back to a generic phrase so a stray tool name never reaches
/// the user.
fn humanize_tool_name(tool_name: &str) -> &'static str {
    crate::local_agent::tools::Tool::from_name(tool_name)
        .map(|t| t.humanized())
        .unwrap_or("the requested action")
}

/// Detect whether a response text contains claims of completed actions.
///
/// Used by the anti-fabrication guard to catch responses where the model
/// narrates actions it never took via tool calls. Checks for common patterns
/// like "I created", "I updated", "I found", etc. The check is deliberately
/// conservative — it only fires on strong first-person action phrases to avoid
/// false positives on legitimate conversational text.
fn contains_action_claim(text: &str) -> bool {
    let lower = text.to_ascii_lowercase();
    // First-person past/present action verbs that imply the model performed a
    // side-effecting operation. "I can" / "I would" / "I could" intentionally
    // excluded — those are capability expressions, not claims of past action.
    const ACTION_PHRASES: &[&str] = &[
        "i created",
        "i've created",
        "i have created",
        "i updated",
        "i've updated",
        "i have updated",
        "i found",
        "i've found",
        "i have found",
        "i added",
        "i've added",
        "i have added",
        "i deleted",
        "i've deleted",
        "i have deleted",
        "i removed",
        "i've removed",
        "i have removed",
        "i marked",
        "i've marked",
        "i have marked",
        "i set ",
        "i've set",
        "i have set",
        "i made ",
        "i've made",
        "i have made",
        "i completed",
        "i've completed",
        "i have completed",
        "successfully created",
        "successfully updated",
        "successfully added",
        "successfully deleted",
        "successfully removed",
        "has been created",
        "has been updated",
        "has been added",
        "has been deleted",
    ];
    if ACTION_PHRASES.iter().any(|p| lower.contains(p)) {
        return true;
    }

    // Bare passive past tense, no "has been". A live turn reported
    // "Fact: node nodespace://... was updated." with zero tool calls and passed
    // straight through, because every passive phrase above requires the "has
    // been" prefix. The claim is identical in force without it.
    //
    // NOTE FOR ANYONE EXTENDING THIS LIST: `exec_update_node`'s content-only
    // `note` (tools.rs) is deliberately worded to avoid every phrase here.
    // Models echo tool-result wording into their final text, and that note is
    // returned on exactly the turn the no-op guard watches, so a phrase added
    // here that the note happens to contain would make the tool feed the guard
    // its own trigger. `update_node_content_only_note_is_not_itself_an_action_claim`
    // pins that both ways.
    //
    // Matched only when the sentence is ABOUT a node, and is not a question or
    // a negation. The bare phrases are far too common on their own: "Nothing
    // was updated because the value already matched" (a negation — the exact
    // opposite of a claim), "Do you know when it was updated?" (a question),
    // and "The record shows it was marked returned" (reporting stored state,
    // not claiming an action) all contain them. Converting any of those to a
    // confirmation request would make the guard a UX regression, so the
    // qualifiers below are load-bearing, not defensive padding.
    const PASSIVE_CLAIMS: &[&str] = &[
        "was created",
        "were created",
        "was updated",
        "were updated",
        "was added",
        "were added",
        "was deleted",
        "were deleted",
        "was removed",
        "were removed",
        "was set to",
        "were set to",
        "was marked",
        "were marked",
    ];
    // Negators are matched with a leading space (or at sentence start) so they
    // cannot fire inside an unrelated word. A bare "not " matches inside
    // "cannot", and "n't " inside "isn't"/"didn't" — all of which routinely
    // co-occur with a TRUE claim ("The task was updated. I cannot see the old
    // value."), and each one silently waived the whole response.
    const NEGATORS: &[&str] = &[
        "nothing ",
        "no node",
        "not ",
        "never ",
        "would be",
        "will be",
        "could be",
        "should be",
    ];
    // "the record shows ...", "it appears ..." — reporting what storage holds,
    // not claiming to have changed it.
    const REPORTING: &[&str] = &[
        "shows ",
        "showed ",
        "indicates ",
        "appears ",
        "according to",
    ];

    // Qualifiers are evaluated PER SENTENCE, against the sentence carrying the
    // passive phrase — never against the whole response.
    //
    // Whole-text qualifiers cannot express a per-clause property, and on a
    // safety guard the failure defaults to "waive", which is the wrong
    // direction. Measured on the whole-text version, every one of these
    // genuine fabricated claims was silently waived:
    //   "The due date was set to 2026-08-06. I did not change anything else."
    //   "The task was updated. I cannot see the old value."
    //   "The node was updated. Anything else?"
    //   "The node was updated. That should be all."
    // "Anything else?" and "That should be all." are among the most common LLM
    // sign-offs, so in practice the guard held only for single-sentence text.
    // Split so each sentence KEEPS its terminator — otherwise a question mark
    // is consumed and "Do you know when it was updated?" reads as a statement.
    let mut sentences: Vec<&str> = Vec::new();
    let mut start = 0usize;
    for (i, c) in lower.char_indices() {
        if matches!(c, '.' | '!' | '?' | '\n' | ';') {
            sentences.push(&lower[start..i + c.len_utf8()]);
            start = i + c.len_utf8();
        }
    }
    if start < lower.len() {
        sentences.push(&lower[start..]);
    }

    sentences.iter().any(|sentence| {
        if !PASSIVE_CLAIMS.iter().any(|p| sentence.contains(p)) {
            return false;
        }
        let trimmed = sentence.trim();
        if trimmed.ends_with('?') {
            return false;
        }
        // Pad so a negator at sentence start still matches with its required
        // leading space.
        let padded = format!(" {trimmed} ");
        let negated = NEGATORS.iter().any(|n| padded.contains(&format!(" {n}")));
        let reporting = REPORTING.iter().any(|r| sentence.contains(r));
        !negated && !reporting
    })
}

/// Tools whose results report what they persisted, and are therefore the only
/// ones [`persisted_field_count`] may read a count from.
///
/// Keyed by tool NAME rather than by result shape because the shapes are not
/// unique to writes: `get_node` on a schema node returns that schema's `fields`
/// array, which is indistinguishable from `create_schema`'s report of the
/// fields it just wrote. Reading a schema that happens to define zero fields
/// would otherwise look exactly like a write that persisted nothing, and a
/// read-only turn would trip the no-op guard.
const FIELD_COUNT_REPORTING_WRITES: &[&str] = &["create_schema", "create_node", "update_node"];

/// Whether a tool execution demonstrably persisted something.
///
/// Reads the same signal the `Tool executed` log line reports as
/// `result_field_count`: `fields` (create_schema) or `property_count`
/// (create_node / update_node). Both answer "did this call record anything the
/// user could later resolve against". A tool that reports neither — or that is
/// not a write at all — returns `None`: absence of the signal is not evidence
/// of a no-op, so only an explicit zero from a known write counts as
/// "persisted nothing".
///
/// A zero is also NOT reported when the call carried no properties to persist
/// in the first place. Both writes flag that case on the result
/// (`updated_content_only`, `content_only`), because zero-of-zero is a complete
/// success — a plain text note has no schema fields, and a content edit changes
/// content by definition. Only zero-out-of-something-expected is evidence that
/// the user's particulars were dropped, and only that may suppress the model's
/// confirmation.
/// Matches a `nodespace://<id>` reference, stopping at whitespace, markdown
/// delimiters, JSON-string delimiters, or trailing sentence punctuation that
/// would otherwise be swept into the id.
///
/// Extends the terminator set `response_processing::replace_status_outside_special`
/// already treats as ending a bare URI (whitespace, `)`, `]`) with sentence
/// punctuation (`.`, `,`, `!`, `?`, `;`, `:`, backtick) and a double quote — a
/// guard comparing extracted ids against tool results by exact string match
/// must not let "...nodespace://abc." (end of sentence) or
/// `"id":"nodespace://abc"` (this regex also scans raw serialized tool-result
/// JSON from session history, see `grounded_node_uris_from_history`) fail to
/// match the grounded "nodespace://abc" a tool actually returned. Real ids
/// are alphanumeric plus `-`/`_`, so none of these characters are ever
/// legitimately part of one.
fn node_uri_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r#"nodespace://[^\s)\]`".,!?;:]+"#).unwrap())
}

/// Every `nodespace://<id>` reference in `text`, in first-seen order, deduped.
fn extract_node_uris(text: &str) -> Vec<&str> {
    let mut seen = HashSet::new();
    node_uri_re()
        .find_iter(text)
        .map(|m| m.as_str())
        .filter(|uri| seen.insert(*uri))
        .collect()
}

/// Recursively collect every `nodespace://<id>` reference appearing anywhere
/// inside a tool call's result JSON.
///
/// Ids surface in different shapes across tools — a bare top-level `id`
/// (`create_node`, `update_node`), an array of results (`search_nodes`), a
/// `resolved`/`id` pair (`resolve_query`) — and `node_uri()` (tools.rs)
/// normalizes all of them to the `nodespace://` form before they reach the
/// model. Walking the whole value rather than picking specific keys means
/// this stays correct as tools add new result shapes, at the cost of also
/// grounding ids that appear in unrelated string fields (e.g. a node's title
/// happening to contain the literal text) — an acceptable direction of error,
/// since it can only make the guard more permissive, never cause it to flag a
/// real id as fabricated.
fn collect_node_uris(value: &serde_json::Value, out: &mut HashSet<String>) {
    match value {
        serde_json::Value::String(s) => {
            for uri in extract_node_uris(s) {
                out.insert(uri.to_string());
            }
        }
        serde_json::Value::Array(items) => {
            for item in items {
                collect_node_uris(item, out);
            }
        }
        serde_json::Value::Object(map) => {
            for v in map.values() {
                collect_node_uris(v, out);
            }
        }
        _ => {}
    }
}

/// Every `nodespace://` id grounded by this turn's tool activity — the union
/// of everything appearing in any tool call's RESULT.
///
/// Deliberately `result`-only, not `args`: the issue's own invariant is "ids
/// come from a tool result... or they do not exist", and grounding on
/// arguments too would let a model launder a fabricated id by simply passing
/// it as an argument to some unrelated executing call (e.g.
/// `search_nodes({"query": "nodespace://invented-id"})`) — the call succeeds,
/// the argument is never validated by the tool, and the invented id would
/// enter the grounded set without any tool ever having confirmed it real.
fn grounded_node_uris(executions: &[ToolExecutionRecord]) -> HashSet<String> {
    let mut out = HashSet::new();
    for exec in executions {
        collect_node_uris(&exec.result, &mut out);
    }
    out
}

/// Every `nodespace://` id grounded by a PRIOR turn's tool activity, read back
/// from `session.messages`.
///
/// `all_tool_executions` covers only the current turn — it is rebuilt fresh
/// per `run_turn` — but a real id created several turns ago is legitimately
/// still referenceable: `format_tool_result` serializes each tool's result
/// JSON verbatim into a `Role::Tool` history entry (see
/// `local_agent::prompt_templates::format_tool_result`), and that history is
/// what feeds the model on every subsequent turn. Without this, a response
/// that correctly recalls an id from three turns ago — with no tool call in
/// the CURRENT turn to re-ground it — would read as fabricated. Scanning the
/// raw JSON text directly (rather than deserializing) is sufficient and
/// cheaper: `extract_node_uris` only needs substring matches, and a tool
/// result's serialized form always carries `nodespace://` ids as plain string
/// values, unescaped, since `node_uri()` never emits characters JSON escapes.
fn grounded_node_uris_from_history(session: &AgentSession) -> HashSet<String> {
    let mut out = HashSet::new();
    for msg in &session.messages {
        if matches!(msg.role, Role::Tool) {
            for uri in extract_node_uris(&msg.content) {
                out.insert(uri.to_string());
            }
        }
    }
    out
}

/// Ids the response text references that neither this turn's tool activity
/// nor prior-turn history grounds.
///
/// Returns them in the order they first appear in `text`, for a stable and
/// readable log line.
fn ungrounded_node_uris(
    text: &str,
    executions: &[ToolExecutionRecord],
    session: &AgentSession,
) -> Vec<String> {
    let mut grounded = grounded_node_uris(executions);
    grounded.extend(grounded_node_uris_from_history(session));
    extract_node_uris(text)
        .into_iter()
        .filter(|uri| !grounded.contains(*uri))
        .map(str::to_string)
        .collect()
}

fn persisted_field_count(tool: &str, result: &serde_json::Value) -> Option<usize> {
    if !FIELD_COUNT_REPORTING_WRITES.contains(&tool) {
        return None;
    }
    let content_only = |k: &str| result.get(k).and_then(|v| v.as_bool()).unwrap_or(false);
    if content_only("updated_content_only") || content_only("content_only") {
        return None;
    }
    result
        .get("fields")
        .and_then(|f| f.as_array())
        .map(|a| a.len())
        .or_else(|| {
            result
                .get("property_count")
                .and_then(|c| c.as_u64())
                .map(|c| c as usize)
        })
}

/// Synthesize a user-facing bullet summary from the tool executions of a turn.
///
/// Last-resort fallback for when the model produced no usable final text
/// (empty, or a leaked tool call). Repeated calls to the same tool collapse to
/// one bullet with a retry count so the diagnostic signal — the agent looped on
/// the same operation — survives.
///
/// Grouping by tool name is what makes the count possible, and it is also where
/// a failure can hide: two calls to the same tool can have opposite verdicts,
/// and a single verb for the pair has to pick one. The rule is the one the
/// tool-failure-surfacing guard already applies — **the LAST call of a name
/// decides**, because that is the call the turn ended on:
///
/// - every call errored → "failed"
/// - the last call succeeded → "completed". An earlier error was a retry the
///   model demonstrably recovered from, and reporting it would describe a
///   transient the user has no action to take on.
/// - the last call errored after an earlier one succeeded → BOTH counts are
///   reported. Collapsing this to "completed" is the exact defect this
///   summarizer exists to prevent, one layer down: `create_node` for record A
///   succeeding and `create_node` for record B failing would otherwise render
///   "• node creation completed (2×)" and the failed write would never be seen.
fn summarize_executions(executions: &[ToolExecutionRecord]) -> String {
    /// (label, total, errored, last call errored) preserving first-seen order.
    type Tally = (&'static str, usize, usize, bool);

    let mut counts: Vec<Tally> = Vec::new();
    for t in executions {
        let label = humanize_tool_name(&t.name);
        if let Some(entry) = counts.iter_mut().find(|(l, ..)| *l == label) {
            entry.1 += 1;
            if t.is_error {
                entry.2 += 1;
            }
            entry.3 = t.is_error;
        } else {
            counts.push((label, 1, usize::from(t.is_error), t.is_error));
        }
    }

    fn times(n: usize) -> String {
        if n > 1 {
            format!(" ({n}×)")
        } else {
            String::new()
        }
    }

    counts
        .into_iter()
        .map(|(label, count, errored, last_errored)| {
            if errored == count {
                format!("• {label} failed{}", times(count))
            } else if !last_errored {
                format!("• {label} completed{}", times(count))
            } else {
                // Mixed, ending on a failure. Both counts, always parenthesized
                // even at 1 — "completed, failed" without them reads as a
                // contradiction rather than as two calls with two outcomes.
                format!(
                    "• {label} completed ({}×), failed ({}×)",
                    count - errored,
                    errored
                )
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Detect a tool call that the model emitted as plain text instead of invoking.
///
/// Some locally-hosted models (e.g. `mistral:7b` via Ollama) don't reliably use
/// the structured `tool_calls` response field — they instead print the call as
/// prose, e.g. `search_nodes(node_type='task', ...)`. No tool ever executes,
/// yet the pseudo-code is persisted verbatim as the assistant's answer, so the
/// user sees a raw code snippet with no indication anything went wrong.
///
/// This is deliberately narrow: it matches a registered tool name in one of
/// three call-shaped positions, never a bare mention. Matching a general
/// `snake_case(` shape would false-positive on legitimate prose that references
/// functions. Tool names are taken from the registry ([`crate::local_agent::tools::Tool::ALL`])
/// so the detector stays in sync as tools are added or removed.
/// Whether `prefix` ends with `marker` standing as its own token.
///
/// A bare `ends_with` on a marker that is also a word ending re-admits the
/// false positives the markers exist to exclude: `"recall:"` ends with
/// `"call:"`, so "Recall: create_node {content} …" would read as a narrated
/// call and the model's correct answer would be discarded. Requiring the
/// character before the marker to be a non-word character (or nothing at all)
/// keeps the marker a delimiter rather than a suffix.
fn ends_with_marker(prefix: &str, marker: &str) -> bool {
    match prefix.strip_suffix(marker) {
        None => false,
        Some(head) => head
            .chars()
            .next_back()
            .is_none_or(|c| !c.is_alphanumeric() && c != '_'),
    }
}

fn looks_like_narrated_tool_call(text: &str) -> bool {
    let lower = text.to_ascii_lowercase();
    crate::local_agent::tools::Tool::ALL.iter().any(|tool| {
        let name = tool.name();
        lower.match_indices(name).any(|(idx, _)| {
            let rest = &lower[idx + name.len()..];
            let after = rest.trim_start();
            // `create_node(...)` — the tool named as a function.
            if after.starts_with('(') {
                return true;
            }
            let before = &lower[..idx];
            // `call:create_schema{fields:[...], name:"Ticket"}` — the tool named
            // with a brace-delimited argument object rather than parens, behind
            // a call-ish prefix. Observed live on the locked model, reaching the
            // user verbatim as the assistant's answer:
            //
            //     call:create_schema{fields:[{"friendlyName":"Status", …}]}
            //
            // A brace alone is NOT enough — "create_node {" appears in ordinary
            // prose about JSON — so this additionally requires a MARKER prefix:
            // a delimiter a narrated call carries and prose does not.
            //
            // Two earlier admissions were removed after they were shown to fire
            // on correct answers, which is the expensive error here: a positive
            // verdict DISCARDS the model's text and substitutes a confirmation
            // request, so a false positive destroys a good reply.
            //
            //   `ends_with("call")` matched "call" as an ordinary English verb —
            //   "To do this I would call create_node {…}" is a model correctly
            //   explaining itself, and "Recall create_node {a}" matched too.
            //
            //   `is_empty()` fired on any response merely BEGINNING with a tool
            //   name — "update_node {id, field_values} takes two arguments." is
            //   documentation, not a call.
            //
            // Only the marker forms survive, and each is anchored so it cannot
            // be reached by a longer word ending in it. `ends_with("call:")` on
            // its own matches "recall:", which puts the just-removed English
            // verb straight back one character narrower — "Recall: create_node
            // {content} is the one you want." is prose, not a call.
            //
            // No "own line" admission. An earlier version accepted a tool name
            // immediately followed by `{` at the start of a line, reasoning that
            // a narrated call writes its arguments flush against the name
            // (`create_node{"content":…}`) while documentation puts a space
            // there (`create_node {…}`). That distinction holds for
            // documentation, but not for a model's own honest report of a
            // completed write inside a fenced JSON block — ordinary, idiomatic
            // formatting omits the space too:
            //
            //     …the payload was:\n```json\ncreate_node{"content":"Buy milk"}\n```
            //
            // Found in a post-merge audit: every regression test for the
            // own-line case used the spaced form, so the unspaced one — a real
            // shape a model can produce at any time — went undetected as a false
            // positive. Missing a narrated call is the cheaper error (the user
            // sees pseudo-code, which is visible and recoverable); a false
            // positive here discards the model's correct answer outright. Only
            // an explicit marker (`call:`, `tool:`, `>>>`) is unambiguous enough
            // to accept.
            let prefix = before.trim_end();
            let marker_prefixed = ["call:", "tool:", ">>>"]
                .iter()
                .any(|marker| ends_with_marker(prefix, marker));
            if after.starts_with('{') && marker_prefixed {
                return true;
            }
            // `{"name": "create_node", "arguments": {...}}` — the tool named as
            // the value of a JSON `name` key. Observed in practice: a model
            // emits a well-formed tool call as *text*, so the loop sees no tool
            // call at all and the turn silently does nothing. The quote before
            // the name distinguishes this from prose that merely mentions the
            // tool.
            let quoted = prefix.ends_with('"') || prefix.ends_with('\'');
            quoted && after.starts_with(['"', '\''])
        })
    })
}

// ---------------------------------------------------------------------------
// LocalAgentLoop
// ---------------------------------------------------------------------------

/// Core ReAct loop implementation.
///
/// Stateless: operates on a provided session and delegates to the injected
/// inference engine and tool executor. The caller (`LocalAgentService`)
/// manages session state and persistence.
pub struct LocalAgentLoop<E: ChatInferenceEngine + ?Sized, T: AgentToolExecutor + ?Sized> {
    engine: Arc<E>,
    tool_executor: Arc<T>,
    prompt_assembler: Option<Arc<PromptAssembler>>,
}

impl<E: ChatInferenceEngine + ?Sized, T: AgentToolExecutor + ?Sized> LocalAgentLoop<E, T> {
    pub fn new(engine: Arc<E>, tool_executor: Arc<T>) -> Self {
        Self {
            engine,
            tool_executor,
            prompt_assembler: None,
        }
    }

    /// The inference engine backing this loop, so callers that need to report
    /// the loaded model's real geometry (id, granted context window) can ask it
    /// without threading a second handle through every construction site.
    pub fn engine(&self) -> &Arc<E> {
        &self.engine
    }

    pub fn with_prompt_assembler(mut self, assembler: Arc<PromptAssembler>) -> Self {
        self.prompt_assembler = Some(assembler);
        self
    }

    /// Execute one full agent turn: inference + tool loop.
    ///
    /// Appends the user message to the session, builds the prompt, runs
    /// inference (potentially multiple rounds of tool calls), and returns
    /// the final response. The session is mutated in place with all
    /// intermediate messages.
    ///
    /// `on_status` is called for each status transition.
    /// `on_chunk` forwards streaming tokens to the caller.
    /// `cancel` can be used to abort mid-generation.
    pub async fn run_turn(
        &self,
        session: &mut AgentSession,
        user_message: &str,
        on_status: impl Fn(LocalAgentStatus) + Send + Sync + 'static,
        on_chunk: impl Fn(StreamingChunk) + Send + Sync + 'static,
        cancel: CancellationToken,
    ) -> Result<AgentTurnResult, InferenceError> {
        // Wrap on_chunk in Arc so it can be cloned into each iteration's callback
        let on_chunk = Arc::new(on_chunk);

        // Root OTLP span for this full agent turn. No-op when tracing is disabled.
        let tracer = opentelemetry::global::tracer(TRACER_NAME);
        let mut turn_span = tracer.start("agent_turn");
        turn_span.set_attribute(KeyValue::new("session_id", session.id.clone()));
        turn_span.set_attribute(KeyValue::new(
            "model_id",
            session.model_id.clone().unwrap_or_default(),
        ));
        turn_span.set_attribute(KeyValue::new("user_message", user_message.to_string()));
        let turn_cx = opentelemetry::Context::current_with_span(turn_span);

        // Append user message
        session
            .messages
            .push(ChatMessage::text(Role::User, user_message.to_string()));

        // The model-facing tool surface. `search_skills` is deliberately absent:
        // ADR-038 makes retrieval a deterministic system step (see `route`
        // below) rather than a model tool call, because a model-issued
        // retrieval lets the model set K and bypasses the trust filter.
        let all_tools = self
            .tool_executor
            .available_tools()
            .await
            .unwrap_or_default();

        // Stage 1 + deterministic retrieval. Yields the candidates Stage 2 will
        // judge, or a clarification to put to the user. Routing is best-effort:
        // every failure path inside returns "no candidates", which leaves the
        // turn running on the full tool surface rather than costing the user
        // their request.
        // Stage 1 runs a generation, so the turn is already working from the
        // caller's point of view — announce it before the routing turn rather
        // than leaving the UI idle through it.
        on_status(LocalAgentStatus::Thinking);
        let routed = self.route(session, user_message, &turn_cx, &cancel).await;

        if let Some(clarification) = routed.clarification {
            // Stage 1 chose to clarify and the contract permits it. Answer with
            // the question directly — no retrieval, no tool surface, no second
            // model turn to paraphrase what the model already composed.
            session
                .messages
                .push(ChatMessage::text(Role::Assistant, clarification.clone()));
            on_status(LocalAgentStatus::Idle);
            return Ok(AgentTurnResult {
                response: clarification,
                reasoning: None,
                tool_calls_made: Vec::new(),
                usage: routed.usage,
                clarify: routed.clarify_prompt,
            });
        }

        // Stage 2's surface: scoped to what the eligible candidates permit, so
        // the model judges among what retrieval surfaced and can only fire what
        // the matched skills allow. Falls back to the full list when nothing
        // was retrieved. Tool-surface scoping is independent of prompt
        // injection below and stays in effect even when injection is
        // disabled — the routing-reliability matrix measured the *prompt
        // block* suppressing tool-calling, not the scoped tool list, so only
        // the mechanism actually implicated is turned off.
        let mut tools = routing::stage2_tools(&routed.candidates, &all_tools);

        // A destructive tool withheld because its skill placed but did not win
        // retrieval. Logged because the *absence* of a tool is otherwise
        // indistinguishable from a model that declined to call it: nothing in
        // the log or the turn output flagged the surface as wrong, so a
        // retrieval defect read as a model defect for as long as it went
        // unnoticed. `info` rather than `warn` — withholding here is the
        // system working as designed, and the interesting case is being able
        // to correlate it with a turn that then behaved oddly.
        // A destructive skill retrieval surfaced but the destructive bar
        // rejected outright. Logged separately from the withheld case below
        // because it is the failure mode in the other direction: not "a
        // destructive tool leaked onto the surface" but "a real deletion
        // request may have been dropped". `DESTRUCTIVE_SKILL_SCORE_BAR` is an
        // unmeasured placeholder and the skill's description was narrowed in
        // the same change, so this is the line that would show the bar is set
        // too high — with the scores attached, so the distribution can be read
        // from ordinary logs instead of a dedicated eval run.
        let below_bar = routing::destructive_candidates_below_bar(&routed.candidates);
        if !below_bar.is_empty() {
            tracing::info!(
                session_id = %session.id,
                rejected = %below_bar
                    .iter()
                    .map(|(n, s)| format!("{n}={s:.3}"))
                    .collect::<Vec<_>>()
                    .join(", "),
                bar = routing::DESTRUCTIVE_SKILL_SCORE_BAR,
                "Stage-2 rejected a destructive skill below its score bar. If the user was in \
                 fact asking to delete something, the bar is set too high for how that request \
                 embeds."
            );
        }

        let withheld = routing::destructive_tools_withheld(&routed.candidates);
        if !withheld.is_empty() {
            tracing::info!(
                session_id = %session.id,
                withheld_tools = %withheld.join(", "),
                routed_skills = %routing::routed_skill_names(&routed.candidates),
                "Stage-2 scoping withheld a destructive tool from a skill that did not win \
                 retrieval. If this turn looks like the model failed to act, check whether it \
                 needed a tool no eligible skill offered."
            );
        }

        // Declares write-tool `field_values` sub-properties from the same
        // retrieved-schema data `candidate_block` below renders into the
        // prompt — content, not tool-list scoping, so it is gated behind the
        // same `routing_disabled` probe rather than folded into
        // `stage2_tools` unconditionally. See `declare_write_tool_fields`'s
        // own doc comment for why: the routing-reliability matrix's finding
        // was about retrieved-schema content reaching the model at all, not
        // specifically the prompt-text channel it was measured through.
        if !session.routing_disabled {
            tools = routing::declare_write_tool_fields(&routed.candidates, tools);
        }

        // `session.routing_disabled` is set once, by the caller, from a cached
        // routing-probe verdict for this session's model (see
        // `local_agent::routing_probe`). The matrix in
        // `tests/live_openai_compat_routing.rs` found injecting this block
        // suppresses tool-calling outright on some served models, independent
        // of the block's content — so a model probed unsafe skips injection
        // entirely rather than receiving a "safer" smaller block; there is no
        // known-safe non-empty block to fall back to.
        let candidate_block = if session.routing_disabled {
            if routing::render_candidates_for_prompt(&routed.candidates).is_some() {
                tracing::warn!(
                    session_id = %session.id,
                    model = %session.model_id.as_deref().unwrap_or("unknown"),
                    "Stage-2 candidate injection skipped: this model's routing probe found \
                     candidate injection suppresses tool-calling. Routing falls back to the \
                     full tool surface for this turn."
                );
            }
            None
        } else {
            routing::render_candidates_for_prompt(&routed.candidates)
        };

        // `stage2_tools`'s own guidance-dependency exclusion only fires on its
        // *fail-open* branches (no candidate cleared the score gate). Two
        // narrower cases it cannot see from inside `stage2_tools` alone:
        // `routing_disabled` forcing `candidate_block` to `None` regardless of
        // whether a candidate cleared the gate, and a candidate that clears
        // the gate but whose own `schema_metadata` renders no entity-types
        // sub-block (so the *header* of `candidate_block` is `Some`, but the
        // specific guidance a tool like `resolve_query` needs never appears
        // in it). `tools_with_available_guidance` checks per-tool, per-
        // candidate availability directly rather than inferring it from
        // whether `candidate_block` as a whole is present, so both cases are
        // covered by the same call precisely instead of an approximation.
        //
        // `dynamic_ctx` is read first and passed in because the entity-types
        // block reaches the prompt from two independent sites: the Stage-2
        // candidate block above, and the resident workspace context built
        // before this session was created. A tool whose required parameter
        // points at that heading is satisfiable from either one.
        let dynamic_ctx = session.dynamic_context.as_deref().unwrap_or("");
        let available_guidance = routing::tools_with_available_guidance(
            &routed.candidates,
            session.routing_disabled,
            dynamic_ctx,
        );
        tools.retain(|t| {
            !requires_routed_guidance_tool(&t.name) || available_guidance.contains(t.name.as_str())
        });

        let model_name = session.model_id.as_deref().unwrap_or("unknown");
        let template_ctx = TemplateContext {
            current_date: chrono::Utc::now().format("%Y-%m-%d").to_string(),
            model_name: model_name.to_string(),
            workspace_context: dynamic_ctx.to_string(),
        };

        // Build the system prompt: test override > graph assembler > emergency.
        // `session_prompt_override` returns `None` in production builds (see
        // the `testing` feature on the agent crate). Production inference always
        // wires a `PromptAssembler`; the emergency arm only fires for the
        // daemon's no-op/idle service, which never reaches live inference.
        let base_system_content = if let Some(override_prompt) = session_prompt_override(session) {
            override_prompt.to_string()
        } else if let Some(ref assembler) = self.prompt_assembler {
            assembler
                .assemble(&template_ctx, tools.clone())
                .await
                .system_prompt
        } else {
            EMERGENCY_FALLBACK_PROMPT.to_string()
        };

        // Stage 2 receives its candidates **in the prompt**, not as a tool
        // result. ADR-064 rule 4 reserves tool results for resolved facts
        // rather than procedures, and the measurement supporting skill
        // instructions was taken on prompt-rendered text; the same payload
        // returned as a tool result was observed to suppress tool-calling.
        //
        // This is the injection point where the KV-cache prefix diverges from
        // Stage 1 — the cost ADR-038 accepts and requires be measured.
        let system_content = match &candidate_block {
            Some(block) => format!("{base_system_content}\n\n{block}"),
            None => base_system_content,
        };

        // prompt_assembly child span: records full assembled system prompt and tools offered.
        {
            let mut span = tracer.start_with_context("prompt_assembly", &turn_cx);
            span.set_attribute(KeyValue::new("system_prompt", system_content.clone()));
            span.set_attribute(KeyValue::new("workspace_context", dynamic_ctx.to_string()));
            let tools_json: Vec<serde_json::Value> = tools
                .iter()
                .map(|t| serde_json::json!({"name": t.name, "description": t.description}))
                .collect();
            span.set_attribute(KeyValue::new(
                "tools_offered",
                serde_json::to_string(&tools_json).unwrap_or_default(),
            ));
        }

        let effective_max_iterations = MAX_TOOL_ITERATIONS;

        tracing::info!(
            tools_count = tools.len(),
            tool_names = %tools.iter().map(|t| t.name.as_str()).collect::<Vec<_>>().join(", "),
            system_prompt_len = system_content.len(),
            // Whether Stage 2 actually received a candidate block in its prompt.
            // A turn that silently skipped injection (no eligible candidates, or
            // routing produced none) falls through to the same general tool
            // surface as one that never routed at all — the two are otherwise
            // indistinguishable from the final pass/fail, so this line is what
            // lets an eval tell "routed but nothing matched" apart from "never
            // routed."
            stage2_candidates_injected = candidate_block.is_some(),
            // A third, distinct state from the two above: injection was
            // available (retrieval matched something) but withheld because
            // this session's model failed the routing probe. Without this
            // field, a disabled turn is indistinguishable in the log from
            // "routing produced nothing" even though the causes — and the
            // right fix — are different.
            stage2_routing_disabled = session.routing_disabled,
            "Agent turn: system prompt and tools prepared"
        );

        let mut all_tool_executions: Vec<ToolExecutionRecord> = Vec::new();
        // Reasoning (chain-of-thought) accumulated across every ReAct iteration of
        // this turn, joined into a single section on the final assistant message.
        let mut accumulated_reasoning = String::new();
        // Tracks whether any iteration in this turn has produced at least one
        // tool call. Used by the anti-fabrication guard: once the model has done
        // real work, a fabricated final summary is a different (harder) problem
        // than a pure hallucination with zero tool calls.
        let mut any_real_tool_calls = false;
        // Duplicate tool-call detector: (tool_name, canonical_args_json) pairs
        // seen so far this turn. When the model issues an identical call again
        // (same tool + identical args after round-tripping through serde to
        // normalise key order), we break the loop immediately rather than burning
        // an iteration executing the same query and getting the same result.
        let mut seen_calls: std::collections::HashSet<(String, String)> =
            std::collections::HashSet::new();
        // Consecutive tool calls whose arguments would not parse as JSON. The
        // duplicate detector above cannot catch this class: each attempt is
        // malformed *differently*, so no two canonical arg strings match and the
        // model can burn every iteration without executing a single tool. Reset
        // on any successful parse, so only an unbroken run trips it.
        let mut consecutive_parse_failures = 0usize;
        // Seeded with the Stage-1 routing turn's usage: it is part of what this
        // turn cost, and reporting only the Stage-2 tokens would hide the price
        // of the extra turn from everything that reads this figure.
        let mut total_usage = routed.usage;

        // ReAct loop: iterate up to effective_max_iterations (skill-specific or global fallback)
        for iteration in 0..effective_max_iterations {
            if cancel.is_cancelled() {
                return Err(InferenceError::Engine("cancelled".into()));
            }

            // Maybe summarize history if over budget
            self.maybe_summarize_history(session, &system_content)
                .await?;

            // Build message list: system + history
            let mut messages = vec![ChatMessage::text(Role::System, system_content.clone())];
            messages.extend(session.messages.clone());

            // react_iteration_N child span — records full messages sent, raw response, tool calls.
            let iter_span_name = format!("react_iteration_{iteration}");
            let mut iter_span = tracer.start_with_context(iter_span_name, &turn_cx);
            iter_span.set_attribute(KeyValue::new("iteration", iteration as i64));
            // Serialize messages sent (truncated per message to avoid huge spans).
            let messages_json: Vec<serde_json::Value> = messages
                .iter()
                .map(|m| {
                    let role = format!("{:?}", m.role).to_lowercase();
                    let (preview, truncated) = char_preview(&m.content, 2000);
                    serde_json::json!({"role": role, "content": preview, "content_truncated": truncated})
                })
                .collect();
            iter_span.set_attribute(KeyValue::new(
                "messages_sent",
                serde_json::to_string(&messages_json).unwrap_or_default(),
            ));

            // Status: Thinking
            on_status(LocalAgentStatus::Thinking);
            session.status = LocalAgentStatus::Thinking;

            // Collect chunks to parse tool calls from the response.
            // Uses std::sync::Mutex (not tokio) because the callback runs on
            // a blocking thread inside spawn_blocking.
            let collected_chunks: Arc<std::sync::Mutex<Vec<StreamingChunk>>> =
                Arc::new(std::sync::Mutex::new(Vec::new()));
            let collected_for_cb = Arc::clone(&collected_chunks);
            let on_chunk_clone = Arc::clone(&on_chunk);

            // Wrap on_chunk so we can also collect
            let chunk_callback: Box<dyn Fn(StreamingChunk) + Send> =
                Box::new(move |chunk: StreamingChunk| {
                    // Forward to caller
                    on_chunk_clone(chunk.clone());
                    // Collect for parsing
                    if let Ok(mut guard) = collected_for_cb.lock() {
                        guard.push(chunk);
                    }
                });

            let request = InferenceRequest {
                messages,
                tools: Some(tools.clone()),
                temperature: Some(0.1),
                max_tokens: None, // No cap on tool-calling iterations — truncated args produce invalid JSON
            };

            // Run inference
            let usage = self.engine.generate(request, chunk_callback).await?;

            total_usage.prompt_tokens += usage.prompt_tokens;
            total_usage.completion_tokens += usage.completion_tokens;

            // Parse collected chunks into text + tool calls.
            // Poison recovery is safe here: chunks are append-only, so partial
            // data after a panic is acceptable (we just get fewer chunks).
            let chunks: Vec<StreamingChunk> = {
                let guard = collected_chunks.lock().unwrap_or_else(|p| p.into_inner());
                guard.clone()
            };
            let (response_text, iteration_reasoning, mut tool_calls, chunk_error) =
                Self::parse_chunks(&chunks);

            // A mid-generation error (most commonly the context window
            // filling up before the model finished) still lets the engine
            // return Ok, with only a truncated response_text/tool_calls to
            // show for it. Failing the whole turn here, before either branch
            // below acts on that truncated output, mirrors the "empty
            // response" precedent a few lines down (ADR-062: refuse loudly,
            // don't clamp silently) rather than silently persisting a
            // partial answer as a normal, complete success.
            if let Some(err) = chunk_error {
                tracing::error!(
                    session_id = %session.id,
                    iteration = iteration,
                    error = %err,
                    "Agent loop: inference reported an error mid-generation"
                );
                return Err(InferenceError::Engine(format!(
                    "inference failed mid-generation: {err}"
                )));
            }

            // Repair the model's malformed argument encodings here, at the parse
            // boundary, so every downstream consumer sees one repaired form.
            //
            // Repairing at the execution site instead (where this used to happen,
            // and still happens as a backstop) fixes the copy handed to the tool
            // but leaves `tc.arguments_json` malformed — and that string is what
            // the assistant turn pushed below carries into history, and what the
            // chat template replays verbatim into the next prompt. The model then
            // copies the shape it reads there: measured at 8 of 8 malformed
            // retries from a malformed prior call, against 8 of 8 clean retries
            // from a clean one (`nlp-engine/tests/toolcall_json_shape.rs`). So a
            // single malformation was self-sustaining — repaired for the tool on
            // every attempt, yet re-taught to the model on every attempt.
            // Repairing the record itself is what breaks that loop.
            for tc in &mut tool_calls {
                repair_tool_call_arguments(&mut tc.arguments_json);
            }

            iter_span.set_attribute(KeyValue::new("raw_response", response_text.clone()));
            let tool_calls_json: Vec<serde_json::Value> = tool_calls
                .iter()
                .map(|tc| {
                    serde_json::json!({
                        "id": tc.id,
                        "name": tc.function_name,
                        "arguments": tc.arguments_json,
                    })
                })
                .collect();
            iter_span.set_attribute(KeyValue::new(
                "tool_calls_parsed",
                serde_json::to_string(&tool_calls_json).unwrap_or_default(),
            ));

            iter_span.set_attribute(KeyValue::new("prompt_tokens", usage.prompt_tokens as i64));
            iter_span.set_attribute(KeyValue::new(
                "completion_tokens",
                usage.completion_tokens as i64,
            ));

            // Accumulate this iteration's reasoning into the turn-wide section,
            // separating iterations with a blank line.
            if !iteration_reasoning.trim().is_empty() {
                if !accumulated_reasoning.is_empty() {
                    accumulated_reasoning.push_str("\n\n");
                }
                accumulated_reasoning.push_str(iteration_reasoning.trim());
            }

            let (response_preview, response_preview_truncated) = char_preview(&response_text, 200);
            tracing::info!(
                iteration,
                tool_calls = tool_calls.len(),
                response_len = response_text.len(),
                response_preview = %response_preview,
                response_preview_truncated,
                "Agent loop: inference round completed"
            );
            // Full, untruncated generation — deliberately `debug`, not `info`, so
            // production's default log level is unaffected and this exists only
            // when explicitly requested (`RUST_LOG=debug`). Every false eval
            // result on record was diagnosable from one raw generation and
            // invisible in any aggregate score; the 200-char preview above is not
            // enough to reconstruct what the model actually said when a scenario
            // needs investigating.
            //
            // JSON-encoded rather than `%response_text` (Display) — Display
            // writes the text VERBATIM, embedded newlines included, which breaks
            // the one-line-per-record shape every log scraper here relies on. A
            // JSON string escapes newlines and quotes, so this line is always
            // exactly one line no matter what the model generated, including
            // text that happens to contain another log line's marker substring.
            tracing::debug!(
                iteration,
                raw_response = %serde_json::to_string(&response_text).unwrap_or_default(),
                "Agent loop: raw generation"
            );

            if tool_calls.is_empty() {
                // No tool calls — final response
                on_status(LocalAgentStatus::Streaming);
                session.status = LocalAgentStatus::Streaming;

                // response_processing child span: records raw input, normalized output,
                // and which strippers fired. Uses normalize_response_traced so we get
                // the stripper list without running normalization twice.
                let raw_for_span = response_text.clone();
                let (normalized, strippers_fired) = normalize_response_traced(&response_text);
                {
                    let mut span = tracer.start_with_context("response_processing", &turn_cx);
                    span.set_attribute(KeyValue::new("raw_input", raw_for_span));
                    span.set_attribute(KeyValue::new("normalized_output", normalized.clone()));
                    span.set_attribute(KeyValue::new(
                        "strippers_fired",
                        strippers_fired.join(", "),
                    ));
                }

                // Anti-fabrication guard: if the model claims an action it never
                // executed (no tool calls in any iteration this turn), suppress the
                // fabricated claim and ask the user to confirm instead.
                // This catches models (e.g. 12B@8K) that narrate fictional successes
                // like "I created invoice 104" without calling any tool.
                // Uses any_real_tool_calls rather than all_tool_executions.is_empty()
                // so that the guard correctly fires in the final iteration even when
                // zero-tool-call turns precede it in the same ReAct loop.
                let normalized = if normalized.is_empty() {
                    normalized
                } else if !any_real_tool_calls && contains_action_claim(&normalized) {
                    let (preview, preview_truncated) = char_preview(&normalized, 120);
                    tracing::warn!(
                        session_id = %session.id,
                        model = %session.model_id.as_deref().unwrap_or("unknown"),
                        iteration = iteration,
                        response_preview = %preview,
                        response_preview_truncated = preview_truncated,
                        "Anti-fabrication: model claimed action with zero tool calls — converting to confirmation request"
                    );
                    CONFIRMATION_REQUEST.to_string()
                } else {
                    normalized
                };

                // Fabricated-id guard: a `nodespace://<id>` in the model's text
                // that no tool result (this turn or an earlier one in this
                // session) ever produced is invented — ids are never something
                // the model should originate, they come from a tool result or
                // they do not exist. This catches the shape #2257's
                // zero-tool-call guard above cannot: the write genuinely
                // succeeded and the model DID call a tool, but then narrated a
                // different id than the one the tool actually returned. A
                // fabricated id in `nodespace://` form is worse than a vague
                // hallucination — it reads as a durable, pastable reference and
                // resolves to nothing.
                let normalized = if !normalized.is_empty() && normalized.contains("nodespace://") {
                    let bad_ids = ungrounded_node_uris(&normalized, &all_tool_executions, session);
                    if bad_ids.is_empty() {
                        normalized
                    } else {
                        let (preview, preview_truncated) = char_preview(&normalized, 120);
                        tracing::warn!(
                            session_id = %session.id,
                            model = %session.model_id.as_deref().unwrap_or("unknown"),
                            iteration = iteration,
                            fabricated_ids = %bad_ids.join(", "),
                            response_preview = %preview,
                            response_preview_truncated = preview_truncated,
                            "Fabricated id: model referenced a nodespace:// id no tool call this turn produced — converting to confirmation request"
                        );
                        CONFIRMATION_REQUEST.to_string()
                    }
                } else {
                    normalized
                };

                // Narrated-tool-call guard: some local models emit a tool call as
                // plain text (e.g. `search_nodes(...)`) instead of using the
                // structured tool_calls field, so nothing executes and the raw
                // pseudo-code would be persisted as the answer. Detect that shape
                // and replace it with a confirmation request rather than leaking
                // internal call syntax to the user. Fires independently of
                // any_real_tool_calls: even after a real call earlier in the turn,
                // a leaked pseudo-call in the final text is still not a valid answer.
                let normalized = if !normalized.is_empty()
                    && looks_like_narrated_tool_call(&normalized)
                {
                    let (preview, preview_truncated) = char_preview(&normalized, 120);
                    tracing::warn!(
                        session_id = %session.id,
                        model = %session.model_id.as_deref().unwrap_or("unknown"),
                        iteration = iteration,
                        response_preview = %preview,
                        response_preview_truncated = preview_truncated,
                        "Narrated tool call: model printed a tool call as text instead of invoking it — converting to confirmation request"
                    );
                    CONFIRMATION_REQUEST.to_string()
                } else {
                    normalized
                };

                // No-op-success guard: the anti-fabrication guard above keys on
                // *zero* tool calls, so an action claim backed by a call that
                // succeeded while persisting nothing passes straight through —
                // the claim looks substantiated because a tool did run and did
                // return success. That is the more dangerous shape of the two:
                // an error prompts a retry, a false success does not.
                //
                // Fires only when EVERY write this turn persisted zero fields.
                // A turn with one real write and one empty one is grounded in
                // the real write, mirroring the `any_real_tool_calls` and
                // later-retry narrowing the neighbouring guards already apply.
                // Tools that report no field-count signal are not writes and are
                // ignored entirely, so read-only turns are unaffected.
                let write_field_counts: Vec<usize> = all_tool_executions
                    .iter()
                    .filter(|r| !r.is_error)
                    .filter_map(|r| persisted_field_count(&r.name, &r.result))
                    .collect();
                let all_writes_were_empty =
                    !write_field_counts.is_empty() && write_field_counts.iter().all(|&c| c == 0);
                let normalized = if !normalized.is_empty()
                    && all_writes_were_empty
                    && contains_action_claim(&normalized)
                {
                    let (preview, preview_truncated) = char_preview(&normalized, 120);
                    tracing::warn!(
                        session_id = %session.id,
                        model = %session.model_id.as_deref().unwrap_or("unknown"),
                        iteration = iteration,
                        write_calls = write_field_counts.len(),
                        response_preview = %preview,
                        response_preview_truncated = preview_truncated,
                        "No-op success: model claimed an action backed only by writes that persisted nothing — converting to confirmation request"
                    );
                    CONFIRMATION_REQUEST.to_string()
                } else {
                    normalized
                };

                // Tool-failure surfacing: if any tool failed and the model's response
                // doesn't acknowledge the error, replace the response with an honest
                // error message. Appending to a success claim would produce contradictory
                // output ("The node was updated. ⚠️ Note: ... encountered an error.").
                //
                // A failure is excluded when a LATER execution of the same tool name
                // (by chronological position in `all_tool_executions`, which is pushed
                // in iteration order) succeeded — the model demonstrably reacted to the
                // failure by retrying, and the final response is grounded in that
                // successful retry rather than the earlier error. Mirrors the
                // `any_real_tool_calls` narrowing already applied to the neighboring
                // anti-fabrication guard above: scope to what actually grounds the
                // final answer, not everything that happened anywhere in the turn.
                let failed_tools: Vec<&str> = all_tool_executions
                    .iter()
                    .enumerate()
                    .filter(|(i, r)| {
                        r.is_error
                            && !all_tool_executions[i + 1..]
                                .iter()
                                .any(|later| later.name == r.name && !later.is_error)
                    })
                    .map(|(_, r)| r.name.as_str())
                    .collect();
                let normalized = if !failed_tools.is_empty() && !normalized.is_empty() {
                    let lower = normalized.to_ascii_lowercase();
                    let mentions_error = lower.contains("error")
                        || lower.contains("fail")
                        || lower.contains("couldn't")
                        || lower.contains("could not")
                        || lower.contains("unable");
                    if !mentions_error {
                        let unique_labels: Vec<String> = {
                            let mut seen = std::collections::HashSet::new();
                            failed_tools
                                .iter()
                                .map(|n| humanize_tool_name(n).to_string())
                                .filter(|l| seen.insert(l.clone()))
                                .collect()
                        };
                        tracing::warn!(
                            session_id = %session.id,
                            failed_tools = %failed_tools.join(", "),
                            "Tool failures not surfaced in model response — replacing with error message"
                        );
                        format!(
                            "⚠️ {} encountered an error. Please try again or check the details and retry.",
                            unique_labels.join(", ")
                        )
                    } else {
                        normalized
                    }
                } else {
                    normalized
                };

                // If the model produced no text after tool calls, synthesize a
                // summary so the UI always shows something meaningful.
                //
                // Uses `summarize_executions` — the same error-aware summarizer
                // the other two empty-text exits already use — rather than
                // formatting the last execution's NAME into a success string.
                // The three guards above (anti-fabrication, no-op success,
                // tool-failure surfacing) are each gated on `!normalized
                // .is_empty()`, so a turn where the model emits no text at all
                // reaches here with none of them having run. Naming the last
                // tool and asserting it "completed successfully" then reports a
                // write that failed as a write that happened:
                //
                //     [tool] create_node [ERROR]   (Unknown node_type)
                //     assistant> Done — node creation completed successfully.
                //
                // observed live on a seeded chain where `create_schema` had
                // failed, so the type `create_node` named did not exist. A
                // silently-failed write is the failure mode ADR-056 records as
                // worth taking seriously, and an empty generation is precisely
                // when the model is least able to report it itself.
                let final_response = if normalized.is_empty() && !all_tool_executions.is_empty() {
                    summarize_executions(&all_tool_executions)
                } else if normalized.is_empty() {
                    // Model returned nothing at all — no tools, no text. This is
                    // an inference bug, not a routing decision: the model should
                    // either call a tool or produce text. Surface as an error so
                    // it shows up in logs/metrics rather than being masked by a
                    // canned UX string. Structured fields below let the
                    // production dashboards group these by model and surface
                    // session/iteration for replay.
                    let (user_message_preview, user_message_preview_truncated) = session
                        .messages
                        .iter()
                        .rev()
                        .find(|m| matches!(m.role, Role::User))
                        .map(|m| char_preview(&m.content, 80))
                        .unwrap_or_default();
                    tracing::error!(
                        session_id = %session.id,
                        model = %session.model_id.as_deref().unwrap_or("unknown"),
                        iteration = iteration,
                        prompt_tokens = total_usage.prompt_tokens,
                        completion_tokens = total_usage.completion_tokens,
                        user_message_preview = %user_message_preview,
                        user_message_preview_truncated,
                        "Agent returned empty response with no tool calls"
                    );
                    return Err(InferenceError::Engine(
                        "model produced empty response with no tool calls".into(),
                    ));
                } else {
                    normalized
                };

                // Collapse the accumulated reasoning into the final option.
                let reasoning = (!accumulated_reasoning.trim().is_empty())
                    .then(|| accumulated_reasoning.trim().to_string());

                // Append assistant response to history, carrying the reasoning so
                // it persists and round-trips on reload.
                let mut assistant_msg = ChatMessage::text(Role::Assistant, final_response.clone());
                assistant_msg.reasoning = reasoning.clone();
                session.messages.push(assistant_msg);

                on_status(LocalAgentStatus::Idle);
                session.status = LocalAgentStatus::Idle;

                // `clarify: None` here is a scope decision, not an oversight: a
                // zero-tool-call reply that merely *phrases itself* as a
                // question (the observed failure on agent-matrix scenarios
                // 6/8c, tracked as a model-capability gap in #1922/#1927) is
                // deliberately NOT treated as a clarification. Detecting "this
                // prose reads like a question" would mean parsing free text
                // for intent — exactly the unstructured, unreliable channel
                // ADR-038 built `route_clarify` to avoid. Widening this is a
                // model-behavior question for #1922/#1927 to own, not a
                // rendering gap for this turn's response to paper over.
                return Ok(AgentTurnResult {
                    response: final_response,
                    reasoning,
                    tool_calls_made: all_tool_executions,
                    usage: total_usage,
                    clarify: None,
                });
            }

            // Append the assistant message that issued these tool calls, carrying
            // the structured tool_calls so the next re-prompt produces a
            // well-formed turn (assistant tool_calls → matching tool results).
            //
            // Drop any prose the model emitted alongside the tool call. Small
            // models (Gemma 4) tend to narrate ("Let me search…") in the same
            // turn as a tool call; when re-prompted, the chat template collapses
            // that prose + the tool_call + the following tool result into a
            // single malformed assistant turn, then opens an empty turn the
            // model fills by running away to the token cap. Persisting only the
            // tool_calls keeps the turn structurally clean: the assistant turn
            // is purely the call, and the answer is produced in a later turn
            // once the tool results are in hand.
            session
                .messages
                .push(ChatMessage::assistant_with_tool_calls(
                    String::new(),
                    tool_calls.clone(),
                ));

            // Record that at least one real tool call has been made this turn.
            any_real_tool_calls = true;

            // Duplicate-call guard: if every tool call in this iteration is
            // identical to one already executed this turn, the model is stuck in
            // a loop. Break out now so the final-inference path can produce a
            // response from the results already in session history, rather than
            // burning iterations re-executing the same query.
            //
            // Args are round-tripped through serde to normalise JSON key order
            // so {"b":1,"a":2} and {"a":2,"b":1} are treated as the same call.
            let all_duplicate = tool_calls.iter().all(|tc| {
                seen_calls.contains(&(tc.function_name.clone(), canonical_args(&tc.arguments_json)))
            });
            if all_duplicate {
                tracing::warn!(
                    session_id = %session.id,
                    iteration = iteration,
                    tool_names = %tool_calls.iter().map(|tc| tc.function_name.as_str()).collect::<Vec<_>>().join(", "),
                    "Duplicate tool-call loop detected — breaking to force final response"
                );
                // Remove the assistant tool-call message we just pushed (it has no
                // matching tool results and would produce a malformed turn).
                session.messages.pop();
                break;
            }
            // Register each call in the seen set so later iterations can detect repeats.
            for tc in &tool_calls {
                seen_calls.insert((tc.function_name.clone(), canonical_args(&tc.arguments_json)));
            }

            // Execute each tool call
            let mut tool_results_for_span: Vec<serde_json::Value> = Vec::new();
            // Pre-scanned BEFORE any call in this iteration executes, not
            // discovered mid-loop: nothing here can prevent a write this
            // iteration's OWN loop already ran for real, so "does this batch
            // contain a clarify" has to be known up front. A well-formed
            // Stage-2 route_clarify supersedes every other call the model
            // made in the SAME iteration — asking "which one?" while also
            // taking an action in the same breath is a contradiction
            // `route_clarify`'s own description only discourages in prose
            // ("do not call another tool in the same turn"), not something
            // structurally prevented. Without this, a batched write
            // (`update_node` alongside `route_clarify`) would execute for
            // real while only the clarifying question reaches the user —
            // exactly the kind of silent side effect this turn is trying to
            // avoid by asking in the first place.
            let stage2_clarify: Option<(String, Vec<crate::local_agent::tools::ClarifyOption>)> =
                tool_calls.iter().find_map(|tc| {
                    if tc.function_name != routing::ROUTE_CLARIFY_TOOL {
                        return None;
                    }
                    let mut args_value: serde_json::Value = if tc.arguments_json.trim().is_empty() {
                        serde_json::json!({})
                    } else {
                        serde_json::from_str(&tc.arguments_json).ok()?
                    };
                    // Mirrors the repair pass the per-call loop below applies
                    // before its own parse, so this scan cannot disagree with
                    // what that loop decides for the same call — a mismatch
                    // here would mean a well-formed clarify goes undetected,
                    // which is exactly the silent-write gap this scan exists
                    // to close.
                    repair_parsed_tool_arguments(&mut args_value);
                    crate::local_agent::tools::parse_route_clarify_args(&args_value)
                });
            for tc in &tool_calls {
                if cancel.is_cancelled() {
                    return Err(InferenceError::Engine("cancelled".into()));
                }

                on_status(LocalAgentStatus::ToolExecution {
                    tool_name: tc.function_name.clone(),
                });
                session.status = LocalAgentStatus::ToolExecution {
                    tool_name: tc.function_name.clone(),
                };

                let start = Instant::now();

                // Unparseable arguments must be reported as such. Substituting an
                // empty object here (the previous behaviour) sends the tool a
                // payload the model never wrote, so the failure surfaces as a
                // missing required field — describing the substitute rather than
                // the malformed JSON that actually caused it, and pointing the
                // model's repair attempt at the wrong problem.
                let parsed_args = if tc.arguments_json.trim().is_empty() {
                    // No arguments emitted at all: an empty object is the faithful
                    // reading, and the tool's own required-field error is correct.
                    Ok(serde_json::json!({}))
                } else {
                    serde_json::from_str::<serde_json::Value>(&tc.arguments_json)
                };

                let (args, tool_result) = match parsed_args {
                    Ok(mut args) => {
                        consecutive_parse_failures = 0;
                        // Repair the model's malformed encodings before the tool
                        // sees them. The arguments parse as valid JSON — the keys
                        // or values are simply wrong — so no parse-failure guard
                        // fires and nothing upstream would catch them.
                        //
                        // `arguments_json` was already repaired at the parse
                        // boundary, so in practice this is a no-op. Retained as a
                        // backstop because this is the only repair the tool itself
                        // and the cross-turn write guard observe directly: it must
                        // not depend on the earlier pass having run. Both sites go
                        // through the same helper, so the set of malformations
                        // handled cannot drift apart as new ones are found.
                        repair_parsed_tool_arguments(&mut args);

                        if let Some((question, options)) = &stage2_clarify {
                            // A well-formed route_clarify call is present
                            // SOMEWHERE in this iteration's calls (see the
                            // pre-scan above) — supersedes every OTHER call in
                            // the same batch, including the guards below,
                            // which would otherwise apply: refusing a call for
                            // being a duplicate write is moot when nothing in
                            // this batch is going to execute for real anyway.
                            if tc.function_name == routing::ROUTE_CLARIFY_TOOL {
                                (
                                    args,
                                    Some(Ok(crate::agent_types::ToolResult {
                                        tool_call_id: tc.id.clone(),
                                        name: tc.function_name.clone(),
                                        result: serde_json::json!({
                                            "acknowledged": true,
                                            "question": question,
                                            "options": options,
                                        }),
                                        is_error: false,
                                    })),
                                )
                            } else {
                                tracing::warn!(
                                    session_id = %session.id,
                                    tool = %tc.function_name,
                                    iteration = iteration,
                                    "Tool call skipped — superseded by a route_clarify call in \
                                     the same iteration"
                                );
                                (
                                    args,
                                    Some(Ok(crate::agent_types::ToolResult {
                                        tool_call_id: tc.id.clone(),
                                        name: tc.function_name.clone(),
                                        result: serde_json::json!({
                                            "skipped": "superseded_by_clarification",
                                            "message": "Not executed: this response also asked \
                                                the user a clarifying question, so nothing else \
                                                in it was carried out. Wait for the user's \
                                                answer, then act on it in a follow-up call.",
                                        }),
                                        is_error: false,
                                    })),
                                )
                            }
                        } else {
                            // Cross-turn duplicate guard. The per-turn `seen_calls`
                            // set above cannot see this: the session is rebuilt from
                            // persisted messages every turn, so a repeat of a write
                            // that landed in an *earlier* turn arrives here looking
                            // brand new. Comparison is on the same canonical form
                            // `seen_calls` uses, against writes replayed from the
                            // conversation record.
                            let already_written = if is_cross_turn_guarded_tool(&tc.function_name) {
                                // Canonicalised from the *parsed* arguments, not the
                                // raw text, so this derives from exactly what the
                                // record side stores (`ToolExecutionRecord.args`).
                                // Canonicalising the raw string instead would differ
                                // wherever the two are not textually identical — most
                                // concretely when empty arguments are read as `{}`
                                // above, which yields "" here against a stored "{}"
                                // and a write that could never match itself.
                                //
                                // Reduced to an identity by the same function the
                                // record side uses, so an oversized call compares
                                // digest-against-digest. Comparing raw canonical args
                                // here against a stored digest would never match, and
                                // would silently unguard exactly the largest writes.
                                let incoming =
                                    canonical_args_identity(&canonical_args(&args.to_string()));
                                session.prior_writes.iter().find(|w| {
                                    w.tool == tc.function_name && w.canonical_args == incoming
                                })
                            } else {
                                None
                            };
                            if let Some(prior) = already_written {
                                tracing::warn!(
                                    session_id = %session.id,
                                    tool = %tc.function_name,
                                    iteration = iteration,
                                    "Cross-turn duplicate write refused — identical call already completed in an earlier turn"
                                );
                                // An informative result, not a silent block. A user
                                // genuinely re-asking for the same node is rare but
                                // real, so the model needs to be able to tell them it
                                // already exists — or, if the repeat is deliberate,
                                // proceed by varying the call.
                                (
                                    args,
                                    Some(Ok(crate::agent_types::ToolResult {
                                        tool_call_id: tc.id.clone(),
                                        name: tc.function_name.clone(),
                                        result: duplicate_write_result(prior),
                                        // Not an error: nothing went wrong, and the
                                        // requested state already holds. Flagging it
                                        // as a failure would invite a repair retry —
                                        // the exact loop this guard exists to stop.
                                        is_error: false,
                                    })),
                                )
                            } else if tc.function_name == "create_schema"
                                && schema_already_created_this_turn(&all_tool_executions)
                            {
                                // Structural backstop for ONE_SCHEMA_PER_REQUEST
                                // (see #1905): the prose rule already says "create
                                // exactly the type asked for", but nothing stops a
                                // second create_schema call within the same skill
                                // invocation from actually executing — Schema
                                // Creation's max_iterations permits up to three
                                // create_schema calls per turn. Refuse the second
                                // one outright rather than letting the model split
                                // a request across two types and silently leave
                                // one half-populated.
                                tracing::warn!(
                                    session_id = %session.id,
                                    iteration = iteration,
                                    "Second create_schema call in one skill invocation refused"
                                );
                                (
                                    args,
                                    Some(Ok(crate::agent_types::ToolResult {
                                        tool_call_id: tc.id.clone(),
                                        name: tc.function_name.clone(),
                                        result: second_schema_refused_result(),
                                        is_error: true,
                                    })),
                                )
                            } else {
                                // Note on `tc.function_name == routing::ROUTE_CLARIFY_TOOL`
                                // reaching here rather than a dedicated branch: it
                                // can only mean this call's own arguments did NOT
                                // parse via `tools::parse_route_clarify_args` (the
                                // pre-scan above uses the same repair-then-parse
                                // logic over every call in this iteration, so if
                                // this call HAD parsed, `stage2_clarify` would be
                                // `Some` and we would not be in this `else`).
                                // Falling through to the normal executor reports
                                // the same invalid-arguments error
                                // `exec_route_clarify` gives on direct dispatch,
                                // rather than a second, differently-worded
                                // validation path here.
                                let result = self
                                    .tool_executor
                                    .execute(&tc.function_name, args.clone())
                                    .await;
                                (args, Some(result))
                            }
                        }
                    }
                    Err(parse_err) => {
                        consecutive_parse_failures += 1;
                        let (args_preview, args_preview_truncated) =
                            char_preview(&tc.arguments_json, 300);
                        tracing::warn!(
                            session_id = %session.id,
                            tool = %tc.function_name,
                            iteration = iteration,
                            error = %parse_err,
                            consecutive_parse_failures,
                            args_preview = %args_preview,
                            args_preview_truncated,
                            "Model emitted unparseable tool arguments — reporting to model instead of substituting an empty object"
                        );
                        (serde_json::json!({}), None)
                    }
                };

                let duration_ms = start.elapsed().as_millis() as u64;

                let (result_value, is_error) = match tool_result {
                    Some(Ok(tr)) => (tr.result, tr.is_error),
                    Some(Err(e)) => (serde_json::json!({"error": e.to_string()}), true),
                    None => (
                        serde_json::json!({
                            "error": format!(
                                "The arguments for {} were not valid JSON, so the call could not \
                                 be made. Re-send the call with the same intent and syntactically \
                                 valid JSON arguments.",
                                tc.function_name
                            )
                        }),
                        true,
                    ),
                };

                // Field count from the tool RESULT, not its arguments: the result is the
                // executor's report of what it persisted, while args are only the model's
                // report of what it asked for. Logged as a bare integer because both
                // previews truncate at 300 chars — a realistic create_schema payload
                // exceeds that, so a parser reading them would fail on exactly the
                // well-formed calls it is meant to pass.
                //
                // Two shapes report it: `fields` (create_schema — the array it
                // persisted) and `property_count` (create_node and update_node
                // — a bare count, since their results carry only the node's
                // id). All answer the same question, "did this call persist
                // anything the user can actually record against", so they feed
                // one field rather than several the scraper would have to
                // reconcile. On update_node the count is of what the CALL
                // supplied, so an id-only call reports 0 rather than the
                // node's existing property count.
                // Shares `persisted_field_count` with the no-op-success guard
                // below, so what is logged and what is gated on can never drift
                // apart — the signal was previously computed here and only
                // logged, never acted on.
                let result_field_count = persisted_field_count(&tc.function_name, &result_value);

                let (args_preview, args_preview_truncated) = char_preview(&args.to_string(), 300);
                let (result_preview, result_preview_truncated) =
                    char_preview(&result_value.to_string(), 300);
                tracing::info!(
                    tool = %tc.function_name,
                    is_error,
                    duration_ms,
                    result_field_count,
                    args_preview = %args_preview,
                    args_preview_truncated,
                    result_preview = %result_preview,
                    result_preview_truncated,
                    "Tool executed"
                );

                let (result_for_span, result_for_span_truncated) =
                    char_preview(&result_value.to_string(), 2000);
                tool_results_for_span.push(serde_json::json!({
                    "tool": tc.function_name,
                    "is_error": is_error,
                    "duration_ms": duration_ms,
                    "result": result_for_span,
                    "result_truncated": result_for_span_truncated,
                }));

                let record = ToolExecutionRecord {
                    tool_call_id: tc.id.clone(),
                    name: tc.function_name.clone(),
                    args,
                    result: result_value.clone(),
                    is_error,
                    duration_ms,
                };

                session.tool_executions.push(record.clone());
                all_tool_executions.push(record);

                // Append tool result to history
                let tool_msg = prompt_templates::format_tool_result(
                    &tc.function_name,
                    &result_value,
                    is_error,
                );
                session.messages.push(ChatMessage::tool_result(
                    tool_msg,
                    tc.id.clone(),
                    tc.function_name.clone(),
                ));
            }

            // Stage-2 route_clarify: end the turn now, the same way Stage 1's
            // clarify does — surface the question and stop, without running
            // another inference round or any further tool call. Checked here
            // (after every call this iteration got its matching tool result,
            // keeping the conversation well-formed) rather than returning
            // from inside the loop above.
            if let Some((question, options)) = stage2_clarify {
                // `ClarifyPrompt.options`/`format_clarification` are Stage-1's
                // existing shape (`Vec<String>`) and stay that way — the
                // richer `{id, label}` pair only needs to reach the MODEL
                // (so it can name an exact candidate precisely); the id has
                // no role in what the USER is shown, so flattening to labels
                // here reuses both unchanged rather than widening a
                // frontend-facing contract Stage 1 already established.
                let labels: Vec<String> = options.iter().map(|o| o.label.clone()).collect();
                let clarification = format_clarification(&question, &labels);
                session
                    .messages
                    .push(ChatMessage::text(Role::Assistant, clarification.clone()));
                on_status(LocalAgentStatus::Idle);
                session.status = LocalAgentStatus::Idle;
                let reasoning = (!accumulated_reasoning.trim().is_empty())
                    .then(|| accumulated_reasoning.trim().to_string());
                return Ok(AgentTurnResult {
                    response: clarification,
                    reasoning,
                    tool_calls_made: all_tool_executions,
                    usage: total_usage,
                    clarify: Some(crate::agent_types::ClarifyPrompt {
                        question,
                        options: labels,
                    }),
                });
            }

            // A model that cannot emit valid JSON is not making progress, and each
            // attempt is malformed differently so the duplicate guard never fires.
            // Break after a short unbroken run — the error results are already in
            // history, so the final-inference path can still answer from them.
            if consecutive_parse_failures >= MAX_CONSECUTIVE_PARSE_FAILURES {
                tracing::warn!(
                    session_id = %session.id,
                    iteration = iteration,
                    consecutive_parse_failures,
                    "Model repeatedly emitted unparseable tool arguments — breaking to force final response"
                );
                break;
            }

            // Set tool results on the iteration span and close it before the next iteration.
            iter_span.set_attribute(KeyValue::new(
                "tool_results",
                serde_json::to_string(&tool_results_for_span).unwrap_or_default(),
            ));

            // If this was the last allowed iteration, do one final inference
            // WITHOUT tools so the model must produce a text response.
            if iteration == effective_max_iterations - 1 {
                tracing::info!(
                    "Agent loop: max iterations reached, running final inference without tools"
                );
                on_status(LocalAgentStatus::Thinking);

                let mut messages = vec![ChatMessage::text(Role::System, system_content.clone())];
                messages.extend(session.messages.clone());

                let final_chunks: Arc<std::sync::Mutex<Vec<StreamingChunk>>> =
                    Arc::new(std::sync::Mutex::new(Vec::new()));
                let final_for_cb = Arc::clone(&final_chunks);
                let on_chunk_final = Arc::clone(&on_chunk);

                let final_callback: Box<dyn Fn(StreamingChunk) + Send> =
                    Box::new(move |chunk: StreamingChunk| {
                        on_chunk_final(chunk.clone());
                        if let Ok(mut guard) = final_for_cb.lock() {
                            guard.push(chunk);
                        }
                    });

                let final_request = InferenceRequest {
                    messages,
                    tools: None, // No tools — force text response
                    temperature: Some(0.1),
                    max_tokens: Some(MAX_RESPONSE_TOKENS),
                };

                if let Ok(usage) = self.engine.generate(final_request, final_callback).await {
                    total_usage.prompt_tokens += usage.prompt_tokens;
                    total_usage.completion_tokens += usage.completion_tokens;

                    // Poison recovery safe: append-only chunk collection (see above).
                    let chunks: Vec<StreamingChunk> = {
                        let guard = final_chunks.lock().unwrap_or_else(|p| p.into_inner());
                        guard.clone()
                    };
                    let (final_text, final_reasoning, _, final_chunk_error) =
                        Self::parse_chunks(&chunks);
                    if !final_reasoning.trim().is_empty() {
                        if !accumulated_reasoning.is_empty() {
                            accumulated_reasoning.push_str("\n\n");
                        }
                        accumulated_reasoning.push_str(final_reasoning.trim());
                    }
                    if let Some(err) = &final_chunk_error {
                        tracing::error!(
                            session_id = %session.id,
                            error = %err,
                            "Agent loop: final inference reported an error mid-generation"
                        );
                    }
                    // Accept the final text only if it's real, complete content
                    // — not empty, not a tool call the model printed as text
                    // instead of invoking, and not truncated by a mid-generation
                    // error (e.g. context-window overflow). Any of those falls
                    // through to the tool-result synthesis below rather than
                    // persisting a truncated or otherwise untrustworthy reply.
                    let normalized = normalize_response(&final_text);
                    if final_chunk_error.is_none()
                        && !normalized.is_empty()
                        && !looks_like_narrated_tool_call(&normalized)
                    {
                        let reasoning = (!accumulated_reasoning.trim().is_empty())
                            .then(|| accumulated_reasoning.trim().to_string());
                        let mut assistant_msg =
                            ChatMessage::text(Role::Assistant, normalized.clone());
                        assistant_msg.reasoning = reasoning.clone();
                        session.messages.push(assistant_msg);

                        on_status(LocalAgentStatus::Idle);
                        session.status = LocalAgentStatus::Idle;

                        return Ok(AgentTurnResult {
                            response: normalized,
                            reasoning,
                            tool_calls_made: all_tool_executions,
                            usage: total_usage,
                            clarify: None,
                        });
                    }
                }

                on_status(LocalAgentStatus::Idle);
                session.status = LocalAgentStatus::Idle;

                // Both final inference and last iteration returned empty —
                // synthesize a summary from tool results so the UI always gets a
                // response.
                let fallback = if !all_tool_executions.is_empty() {
                    summarize_executions(&all_tool_executions)
                } else {
                    // No tool executions to summarize. Try the last iteration's
                    // text; if it's empty, pure internal plumbing, or a leaked
                    // tool call printed as text, fall back to an honest failure
                    // notice so the UI is never blank and never shows pseudo-code.
                    let normalized = normalize_response(&response_text);
                    if normalized.is_empty() || looks_like_narrated_tool_call(&normalized) {
                        EMPTY_RESPONSE_FALLBACK.to_string()
                    } else {
                        normalized
                    }
                };

                return Ok(AgentTurnResult {
                    response: fallback,
                    reasoning: (!accumulated_reasoning.trim().is_empty())
                        .then(|| accumulated_reasoning.trim().to_string()),
                    tool_calls_made: all_tool_executions,
                    usage: total_usage,
                    clarify: None,
                });
            }

            // Otherwise loop back for another inference round
        }

        // Reached via either `break` above: the duplicate-call guard, or the
        // consecutive-parse-failure guard. The max-iteration path
        // (iteration == effective_max_iterations - 1) always returns early and
        // never falls through here. Run one final text-only inference so the
        // session always produces a response from the tool results already in
        // history.
        on_status(LocalAgentStatus::Thinking);

        let mut messages = vec![ChatMessage::text(Role::System, system_content.clone())];
        messages.extend(session.messages.clone());

        let final_chunks: Arc<std::sync::Mutex<Vec<StreamingChunk>>> =
            Arc::new(std::sync::Mutex::new(Vec::new()));
        let final_for_cb = Arc::clone(&final_chunks);
        let on_chunk_tail = Arc::clone(&on_chunk);
        let tail_callback: Box<dyn Fn(StreamingChunk) + Send> =
            Box::new(move |chunk: StreamingChunk| {
                on_chunk_tail(chunk.clone());
                if let Ok(mut guard) = final_for_cb.lock() {
                    guard.push(chunk);
                }
            });

        let tail_request = InferenceRequest {
            messages,
            tools: None,
            temperature: Some(0.1),
            max_tokens: Some(MAX_RESPONSE_TOKENS),
        };

        let final_response =
            if let Ok(usage) = self.engine.generate(tail_request, tail_callback).await {
                total_usage.prompt_tokens += usage.prompt_tokens;
                total_usage.completion_tokens += usage.completion_tokens;
                let chunks: Vec<StreamingChunk> = {
                    let guard = final_chunks.lock().unwrap_or_else(|p| p.into_inner());
                    guard.clone()
                };
                let (tail_text, tail_reasoning, _, tail_chunk_error) = Self::parse_chunks(&chunks);
                if !tail_reasoning.trim().is_empty() {
                    if !accumulated_reasoning.is_empty() {
                        accumulated_reasoning.push_str("\n\n");
                    }
                    accumulated_reasoning.push_str(tail_reasoning.trim());
                }
                if let Some(err) = &tail_chunk_error {
                    tracing::error!(
                        session_id = %session.id,
                        error = %err,
                        "Agent loop: tail inference reported an error mid-generation"
                    );
                }
                let normalized_tail = normalize_response(&tail_text);
                if tail_chunk_error.is_some()
                    || normalized_tail.is_empty()
                    || looks_like_narrated_tool_call(&normalized_tail)
                {
                    // Model returned nothing, leaked internal plumbing (e.g. a
                    // <tool_call> block) that stripped down to nothing, printed
                    // a tool call as text instead of invoking it, or the
                    // response was truncated by a mid-generation error (e.g.
                    // context-window overflow) -- synthesize a summary from the
                    // tool results instead of persisting a blank, truncated, or
                    // untrustworthy reply.
                    summarize_executions(&all_tool_executions)
                } else {
                    normalized_tail
                }
            } else {
                // Inference failed — synthesize from executions.
                //
                // Same error-aware summarizer as the sibling branch above, for
                // the same reason: formatting the last execution's NAME into
                // "completed successfully" reports a failed write as a
                // successful one. This branch is the worse of the two to get
                // wrong — inference itself failed here, so the claim is not
                // even the model's; it is the loop asserting success on its
                // behalf about a call that may have errored.
                if !all_tool_executions.is_empty() {
                    summarize_executions(&all_tool_executions)
                } else {
                    String::new()
                }
            };

        // Final safety net: every branch above can, in principle, produce an
        // empty string (no tool executions to summarize and no usable model
        // text). Never persist a blank assistant bubble — surface an honest
        // failure notice instead.
        let final_response = if final_response.trim().is_empty() {
            EMPTY_RESPONSE_FALLBACK.to_string()
        } else {
            final_response
        };

        let reasoning = (!accumulated_reasoning.trim().is_empty())
            .then(|| accumulated_reasoning.trim().to_string());

        let mut assistant_msg = ChatMessage::text(Role::Assistant, final_response.clone());
        assistant_msg.reasoning = reasoning.clone();
        session.messages.push(assistant_msg);

        on_status(LocalAgentStatus::Idle);
        session.status = LocalAgentStatus::Idle;

        Ok(AgentTurnResult {
            response: final_response,
            reasoning,
            tool_calls_made: all_tool_executions,
            usage: total_usage,
            clarify: None,
        })
    }

    /// Parse collected streaming chunks into response text and tool calls.
    /// Parse a streamed chunk sequence into `(answer_text, reasoning_text, tool_calls)`.
    ///
    /// Reasoning chunks (the model's chain-of-thought, already separated from the
    /// answer at the nlp-engine parse layer) are accumulated independently of the
    /// answer text so the answer bubble stays clean.
    /// Run Stage 1 and the deterministic retrieval step (ADR-038).
    ///
    /// Stage 1 asks the model for a *structural* choice — form a search query,
    /// or ask to clarify — expressed as which of two typed tools it calls. A
    /// tool schema is the strongest measured channel for structured output,
    /// and using the tool-call path means there is no free text to parse and
    /// so no parser failure to confuse with a model failure. ADR-038 rejects
    /// gating on a self-reported confidence number, which a small model is not
    /// calibrated to produce.
    ///
    /// Retrieval then runs here, in the system, rather than as a model tool
    /// call — that is where K is bounded and the trust filter applies.
    ///
    /// Every failure path yields no candidates rather than an error: routing
    /// is an optimisation over the general tool surface, and losing it must
    /// not cost the user their turn.
    async fn route(
        &self,
        session: &AgentSession,
        user_message: &str,
        turn_cx: &opentelemetry::Context,
        cancel: &CancellationToken,
    ) -> RoutingOutcome {
        let mut outcome = RoutingOutcome::default();

        if cancel.is_cancelled() {
            return outcome;
        }

        let tracer = opentelemetry::global::tracer(TRACER_NAME);

        // Routing costs a model turn, so it only runs where it can pay for
        // itself: when retrieval is actually wired up. Without it there are no
        // candidates to judge, and Stage 1 would spend a full generation to
        // produce a query nothing consumes. This also keeps every test double
        // that does not opt into routing on the single-turn path.
        if !self.tool_executor.routing_available().await {
            // Say so in the trace. Routing availability is a live per-turn
            // property — the embedding service loads in the background — so two
            // identical messages in one session can take structurally different
            // paths, and without this nothing distinguishes "routing off, model
            // still warming" from "this executor never routes". An eval run
            // started before the embedding service is ready would otherwise
            // measure the unrouted path and blame routing quality for it.
            let mut span = tracer.start_with_context("stage1_routing_skipped", turn_cx);
            span.set_attribute(KeyValue::new("routing.available", false));
            // `routing_decision` distinguishes this from a genuine "none" decision
            // (Stage 1 ran and chose not to route) — an eval scraping the text log
            // for "none" must not conflate "routing never ran" with "routing ran
            // and declined," which would otherwise look identical downstream.
            tracing::info!(
                routing_decision = "unavailable",
                "routing unavailable for this turn; running unrouted"
            );
            return outcome;
        }

        let mut span = tracer.start_with_context("stage1_routing", turn_cx);
        let started = Instant::now();

        let stage1_tools = routing::stage1_tool_definitions();
        // Blend the preceding turns into Stage 1's view, the same way schema
        // retrieval does. A follow-up naming its subject only by pronoun or
        // ellipsis ("mark it paid", "add another one") gives the model nothing
        // to describe on its own, so it emits a weak query or asks to clarify
        // a request the conversation had already disambiguated — and both fail
        // silently, as worse routing rather than an error.
        //
        // This bites hardest right after a clarification: the answer to one is
        // a short reply, i.e. the message with the least standalone routing
        // signal, arriving on the turn that must succeed without clarifying
        // again. Shares `build_retrieval_query` with schema retrieval so the
        // two context constructions cannot drift apart.
        let routing_query = stage1_query(session, user_message);
        let messages = vec![
            ChatMessage::text(Role::System, STAGE1_SYSTEM_PROMPT.to_string()),
            ChatMessage::text(Role::User, routing_query),
        ];
        let request = InferenceRequest {
            messages,
            tools: Some(stage1_tools),
            temperature: Some(0.1),
            max_tokens: Some(STAGE1_MAX_TOKENS),
        };

        let chunks: Arc<std::sync::Mutex<Vec<StreamingChunk>>> =
            Arc::new(std::sync::Mutex::new(Vec::new()));
        let sink = chunks.clone();
        // Stage 1 is internal routing, not user-facing content: its chunks are
        // collected here and deliberately not forwarded to `on_chunk`, so the
        // user never sees the routing turn stream past.
        let usage = match self
            .engine
            .generate(
                request,
                Box::new(move |c| {
                    if let Ok(mut g) = sink.lock() {
                        g.push(c);
                    }
                }),
            )
            .await
        {
            Ok(u) => u,
            Err(e) => {
                tracing::warn!(
                    routing_decision = "failed",
                    error = %e,
                    "stage-1 routing failed; continuing unrouted"
                );
                span.set_attribute(KeyValue::new("routing.failed", true));
                return outcome;
            }
        };
        outcome.usage = usage;

        let collected = chunks.lock().map(|g| g.clone()).unwrap_or_default();
        let (_, _, tool_calls, _) = Self::parse_chunks(&collected);
        let decision = tool_calls
            .iter()
            .find_map(|tc| routing::parse_route_decision(&tc.function_name, &tc.arguments_json));

        let mut routing_decision_tag = "none";
        // A single query for Query/None/clarify-suppressed; two or more for
        // Multi. Retrieval below re-enters once per element — Stage 2's
        // per-candidate trust boundary and score gating are unchanged, only
        // how many times retrieval runs for this turn.
        let queries: Vec<String> = match decision {
            Some(RouteDecision::Query(q)) => {
                routing_decision_tag = "query";
                span.set_attribute(KeyValue::new("routing.decision", "query"));
                span.set_attribute(KeyValue::new("routing.query", q.clone()));
                vec![q]
            }
            Some(RouteDecision::Multi(qs)) => {
                routing_decision_tag = "multi";
                span.set_attribute(KeyValue::new("routing.decision", "multi"));
                span.set_attribute(KeyValue::new("routing.multi_count", qs.len() as i64));
                qs
            }
            Some(RouteDecision::Clarify { question, options }) => {
                // The clarification contract: at most one per intent. If the
                // conversation already contains a clarification, asking again
                // is the annoying-loop failure ADR-038 specifies against — fall
                // through to retrieval on the raw message instead, so the turn
                // resolves with whatever the general skill can offer.
                if session_already_clarified(session) {
                    routing_decision_tag = "clarify_suppressed";
                    span.set_attribute(KeyValue::new("routing.decision", "clarify_suppressed"));
                    tracing::debug!(
                        "stage-1 asked to clarify twice in one intent; falling through to retrieval"
                    );
                    vec![user_message.to_string()]
                } else {
                    span.set_attribute(KeyValue::new("routing.decision", "clarify"));
                    outcome.clarification = Some(format_clarification(&question, &options));
                    outcome.clarify_prompt = Some(crate::agent_types::ClarifyPrompt {
                        question: question.clone(),
                        options: options.clone(),
                    });
                    let elapsed_ms = started.elapsed().as_millis() as i64;
                    span.set_attribute(KeyValue::new("routing.latency_ms", elapsed_ms));
                    // Mirrors the "two-stage routing overhead" line below, which this
                    // path returns before reaching. Both this and that line carry
                    // `routing_decision` as a plain text field (not only an OTel span
                    // attribute) so an eval scraping the daemon's text log — which has
                    // no OTel exporter attached — can observe which of Stage 1's five
                    // outcomes (query/multi/clarify/clarify_suppressed/none) fired,
                    // rather than inferring it from reply text or downstream tool effects.
                    tracing::info!(
                        routing_decision = "clarify",
                        routing_latency_ms = elapsed_ms,
                        "stage-1 routing decision"
                    );
                    return outcome;
                }
            }
            None => {
                // The model called neither routing tool, or emitted arguments
                // that would not parse. Retrieve on the raw message rather than
                // abandoning routing: a weak query still beats none.
                span.set_attribute(KeyValue::new("routing.decision", "none"));
                vec![user_message.to_string()]
            }
        };

        if cancel.is_cancelled() {
            return outcome;
        }

        // Merge candidates across every query, deduped by skill id (a skill
        // matching more than one intent's query counts once) and capped at
        // the same RETRIEVAL_TOP_K the single-query path already respects —
        // a compound request must not silently widen Stage 2's candidate
        // bound past the system-owned limit ADR-038 requires.
        let mut merged: Vec<crate::agent_types::SkillCandidate> = Vec::new();
        let mut seen_ids: std::collections::HashSet<String> = std::collections::HashSet::new();
        let mut top_score: f32 = 0.0;
        for query in &queries {
            match self
                .tool_executor
                .retrieve_skills(query, routing::RETRIEVAL_TOP_K)
                .await
            {
                Ok(r) => {
                    if let Some(s) = r.candidates.first().map(|c| c.score) {
                        top_score = top_score.max(s);
                    }
                    for c in r.candidates {
                        if seen_ids.insert(c.id.clone()) {
                            merged.push(c);
                        }
                    }
                }
                Err(e) => {
                    tracing::warn!(error = %e, query, "skill retrieval failed for one query; continuing with the rest");
                }
            }
        }
        merged.sort_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        merged.truncate(routing::RETRIEVAL_TOP_K);

        span.set_attribute(KeyValue::new("routing.candidates", merged.len() as i64));
        span.set_attribute(KeyValue::new("routing.top_score", top_score as f64));
        outcome.candidates = merged;

        // The latency ADR-038 requires be measured rather than assumed. Covers
        // the Stage-1 generation plus the retrieval step — i.e. everything the
        // two-stage flow adds ahead of the turn that previously ran alone.
        let elapsed_ms = started.elapsed().as_millis() as i64;
        span.set_attribute(KeyValue::new("routing.latency_ms", elapsed_ms));
        // `routing_decision` here covers the three outcomes that reach this line
        // (query/clarify_suppressed/none); the plain `clarify` outcome returns
        // earlier and logs its own "stage-1 routing decision" line above. Both
        // carry the same field name so a log scraper (an eval, a dashboard) can
        // grep one key regardless of which path a turn took.
        // `routed_skills` names the candidates that clear the score gate — the
        // ones `render_candidates_for_prompt` actually writes into Stage 2's
        // prompt and `stage2_tools` scopes the tool surface from. The count
        // alone cannot answer "which skill was routed to", so a routing claim
        // about any eval scenario required reading source rather than results;
        // an empty value here means retrieval returned nothing above the bar,
        // which is exactly the "routed but matched nothing" case
        // `stage2_candidates_injected` distinguishes on the assembly line.
        // `all_scores` carries every retrieved candidate's score, not only the
        // winner's — `routing.top_score` above cannot tell a tiebreak (a
        // near-tied runner-up) apart from a skill scoring badly on its own
        // core use case with no close competitor, and those two situations
        // call for different fixes. See `all_candidate_scores`'s doc comment.
        tracing::info!(
            routing_decision = routing_decision_tag,
            routing_latency_ms = elapsed_ms,
            candidates = outcome.candidates.len(),
            routed_skills = %routing::routed_skill_names(&outcome.candidates),
            all_scores = %routing::all_candidate_scores(&outcome.candidates),
            "two-stage routing overhead"
        );
        outcome
    }

    /// Parses collected chunks into (text, reasoning, tool calls, error).
    ///
    /// The error slot is `Some(message)` when a `StreamingChunk::Error` chunk
    /// was seen -- e.g. a mid-generation context-window overflow, which lets
    /// the underlying engine finish and report success even though the
    /// output is truncated. Callers that silently ignored this (falling
    /// through to treat the truncated text as a normal, complete response)
    /// are exactly the "turn succeeds with truncated output and zero visible
    /// signal" failure mode; see `run_turn`'s use of this field for the
    /// established "fail loudly" handling other bad-generation outcomes
    /// (e.g. an empty response) already get in this same loop.
    fn parse_chunks(
        chunks: &[StreamingChunk],
    ) -> (String, String, Vec<ToolCallRaw>, Option<String>) {
        let mut text = String::new();
        let mut reasoning = String::new();
        let mut tool_calls: Vec<ToolCallRaw> = Vec::new();
        // Accumulate tool call args by id
        // Use Vec to preserve tool call ordering (important for causal dependencies)
        let mut pending_calls: Vec<(String, String, String, Option<serde_json::Value>)> =
            Vec::new(); // (id, name, args_json, provider_extra)
        let mut error: Option<String> = None;

        for chunk in chunks {
            match chunk {
                StreamingChunk::Token { text: t } => {
                    text.push_str(t);
                }
                StreamingChunk::Reasoning { text: r } => {
                    reasoning.push_str(r);
                }
                StreamingChunk::ToolCallStart {
                    id,
                    name,
                    provider_extra,
                } => {
                    pending_calls.push((
                        id.clone(),
                        name.clone(),
                        String::new(),
                        provider_extra.clone(),
                    ));
                }
                StreamingChunk::ToolCallArgs { id, args_json } => {
                    if let Some(call) = pending_calls
                        .iter_mut()
                        .rev()
                        .find(|(cid, _, _, _)| cid == id)
                    {
                        call.2.push_str(args_json);
                    }
                }
                StreamingChunk::Done { .. } => {}
                StreamingChunk::Error { message } => {
                    // Keep the first error: it names the condition that
                    // actually caused generation to go wrong, rather than
                    // whatever knock-on chunk arrived last.
                    if error.is_none() {
                        error = Some(message.clone());
                    }
                }
            }
        }

        // Convert accumulated tool calls into ToolCallRaw (order preserved)
        for (id, name, args_json, provider_extra) in pending_calls {
            tool_calls.push(ToolCallRaw {
                id,
                function_name: name,
                arguments_json: args_json,
                provider_extra,
            });
        }

        (text, reasoning, tool_calls, error)
    }

    /// Summarize older history turns if the conversation exceeds the token budget.
    ///
    /// Estimates token count for the full history (including tool-call argument
    /// JSON) plus the real system prompt. If the two together exceed the
    /// model's effective context window, summarizes older messages (keeping
    /// the most recent 2-3 turns) and replaces them with a single summary
    /// message.
    async fn maybe_summarize_history(
        &self,
        session: &mut AgentSession,
        system_content: &str,
    ) -> Result<(), InferenceError> {
        if session.messages.len() <= 4 {
            // Too few messages to need summarization
            return Ok(());
        }

        // Estimate token count of the full conversation. A tool-call assistant
        // turn carries its signal in `tool_calls[].arguments_json`, not
        // `content` (content is empty by construction) — up to MAX_RESPONSE_TOKENS
        // worth of JSON per call that the naive content-only scan would miss
        // entirely, which is exactly the corridor where the real prompt
        // overflows the window while the counted history still looks small.
        let mut history_text = String::new();
        for msg in &session.messages {
            history_text.push_str(&msg.content);
            history_text.push(' ');
            for call in &msg.tool_calls {
                history_text.push_str(&call.function_name);
                history_text.push(' ');
                history_text.push_str(&call.arguments_json);
                history_text.push(' ');
            }
        }

        let history_tokens = self.engine.token_count(&history_text).await?;
        let system_tokens = self.engine.token_count(system_content).await?;

        // Budget against the model's *effective* context window, which the
        // native path sizes to available memory at load time (a large model on
        // a constrained machine may get far less than 32K). Reserve room for the
        // reply so summarization triggers before the prompt fills the window and
        // the engine rejects it with ContextOverflow. Fall back to the static
        // budget when the window is unknown (model not loaded / remote backend).
        let total_budget = match self.engine.model_info().await {
            Ok(Some(spec)) if spec.context_window > 0 => spec
                .context_window
                .saturating_sub(MAX_RESPONSE_TOKENS)
                .max(SYSTEM_PROMPT_BUDGET + 1),
            _ => TOTAL_TOKEN_BUDGET,
        };

        // Gate 1: measured history plus the *actual* system prompt against
        // the window. A second check against a static
        // `total_budget - SYSTEM_PROMPT_BUDGET` budget used to sit here, but
        // SYSTEM_PROMPT_BUDGET (4,000) is well under the real tool-registered
        // system prompt (~6,600 measured in nlp-engine). That let the second
        // check pass — and skip summarization — in exactly the corridor where
        // this first check had already established the prompt overflows the
        // window, because it compared against a looser, wrong-headroom budget.
        //
        // Gate 2 (`PREFILL_STABILITY_CEILING`): independent of the window
        // budget — see its doc comment. A large model on a spacious machine
        // can pass gate 1 by a wide margin while still submitting a prefill
        // large enough to risk the backend failure the measured OOM found. Compares
        // against the same total gate 1 does (the whole prompt actually
        // submitted for decode), not history alone.
        let total_prompt_tokens = history_tokens + system_tokens;
        if total_prompt_tokens <= total_budget && total_prompt_tokens <= PREFILL_STABILITY_CEILING {
            return Ok(());
        }

        // Need to summarize. Keep the last 3 messages verbatim, summarize the rest.
        let keep_count = 3.min(session.messages.len());
        let mut split_point = session.messages.len() - keep_count;

        // Never split an assistant tool-call turn from its tool results. A naive
        // cut can leave the kept window starting with an orphan `Tool` message
        // (its preceding assistant `tool_calls` turn drained into the summary),
        // which is exactly the malformed sequence that makes Gemma's template
        // collapse turns and run away. Walk the cut back until the kept window
        // begins on a non-tool message, so every tool result stays paired with
        // the assistant turn that issued it.
        while split_point > 0 && matches!(session.messages[split_point].role, Role::Tool) {
            split_point -= 1;
        }

        // If the back-off consumed the whole prefix there is nothing older to
        // summarize — skip the summarization inference entirely rather than
        // spending a model call on empty input. (Rare: only when the entire
        // over-budget history is one unbroken tool-call chain.)
        if split_point == 0 {
            return Ok(());
        }

        let older_messages: Vec<ChatMessage> = session.messages.drain(..split_point).collect();

        // Build summarization text from older messages. A tool-call assistant
        // turn carries its signal in `tool_calls`, not `content` (content is
        // empty by construction), so render a synthetic line for it — otherwise
        // it collapses to a bare "Assistant:" and the summary loses what the
        // agent actually did.
        let mut summary_input = String::new();
        for msg in &older_messages {
            let role_str = match msg.role {
                Role::System => "System",
                Role::User => "User",
                Role::Assistant => "Assistant",
                Role::Tool => "Tool",
            };
            let rendered = if msg.content.is_empty() && !msg.tool_calls.is_empty() {
                // Cap each argument blob: a single tool call can emit up to the
                // generation token cap of JSON, and the summary only needs the
                // gist of what was called, not the full payload.
                let calls = msg
                    .tool_calls
                    .iter()
                    .map(|tc| {
                        let args: String = tc.arguments_json.chars().take(120).collect();
                        format!("{}({})", tc.function_name, args)
                    })
                    .collect::<Vec<_>>()
                    .join(", ");
                format!("called tools: {calls}")
            } else {
                msg.content.clone()
            };
            summary_input.push_str(&format!("{role_str}: {rendered}\n"));
        }

        let summary_prompt = prompt_templates::summarization_prompt(&summary_input);

        // Run a single-shot summarization inference (no tools).
        //
        // This cap is intentionally larger than `MAX_RESPONSE_TOKENS` (the chat
        // turn cap): a summary condenses many drained turns and is not part of
        // the runaway-prone ReAct loop (no tools, single shot, then discarded
        // into one message), so it can afford more room without risking a
        // multi-minute hang. Do NOT unify the two constants.
        let summary_request = InferenceRequest {
            messages: vec![ChatMessage::text(Role::User, summary_prompt)],
            tools: None,
            temperature: Some(0.1),
            max_tokens: Some(4096),
        };

        let summary_chunks: Arc<std::sync::Mutex<Vec<StreamingChunk>>> =
            Arc::new(std::sync::Mutex::new(Vec::new()));
        let summary_for_cb = Arc::clone(&summary_chunks);
        let cb: Box<dyn Fn(StreamingChunk) + Send> = Box::new(move |chunk: StreamingChunk| {
            if let Ok(mut guard) = summary_for_cb.lock() {
                guard.push(chunk);
            }
        });

        let _ = self.engine.generate(summary_request, cb).await?;

        let chunks: Vec<StreamingChunk> = {
            let guard = summary_chunks.lock().unwrap_or_else(|p| p.into_inner());
            guard.clone()
        };
        let (summary_text, _, _, _) = Self::parse_chunks(&chunks);

        let summary_content = if summary_text.is_empty() {
            // Fallback: just note that history was truncated
            "Previous conversation context was summarized due to token limits.".to_string()
        } else {
            format!("[Conversation summary]: {}", summary_text)
        };

        // Prepend summary as a system-like message at the start of remaining history
        session
            .messages
            .insert(0, ChatMessage::text(Role::System, summary_content));

        Ok(())
    }
}

// ---------------------------------------------------------------------------
// LocalAgentService
// ---------------------------------------------------------------------------

/// Session management facade for the local agent.
///
/// Manages active sessions and provides a high-level API for creating,
/// resuming, and ending conversations. Delegates the actual ReAct loop
/// to [`LocalAgentLoop`].
pub struct LocalAgentService<E: ChatInferenceEngine + ?Sized, T: AgentToolExecutor + ?Sized> {
    sessions: RwLock<HashMap<String, AgentSession>>,
    agent_loop: LocalAgentLoop<E, T>,
    /// Per-session cancellation tokens.
    cancel_tokens: RwLock<HashMap<String, CancellationToken>>,
}

impl<E: ChatInferenceEngine + ?Sized + 'static, T: AgentToolExecutor + ?Sized + 'static>
    LocalAgentService<E, T>
{
    pub fn new(engine: Arc<E>, tool_executor: Arc<T>) -> Self {
        Self::new_with_assembler(engine, tool_executor, None)
    }

    pub fn new_with_assembler(
        engine: Arc<E>,
        tool_executor: Arc<T>,
        prompt_assembler: Option<Arc<PromptAssembler>>,
    ) -> Self {
        let mut agent_loop = LocalAgentLoop::new(engine, tool_executor);
        if let Some(assembler) = prompt_assembler {
            agent_loop = agent_loop.with_prompt_assembler(assembler);
        }
        Self {
            sessions: RwLock::new(HashMap::new()),
            agent_loop,
            cancel_tokens: RwLock::new(HashMap::new()),
        }
    }

    /// The loaded model's spec (id and the context window actually granted at
    /// load time, not the configured ceiling). `None` when no model is loaded.
    ///
    /// Exposed so the daemon can report the real window over the wire: an eval
    /// or client that assumes a window larger than the one granted produces
    /// turns that die on context overflow before inference runs.
    pub async fn model_spec(&self) -> Result<Option<ChatModelSpec>, InferenceError> {
        self.agent_loop.engine().model_info().await
    }

    /// Create a new conversation session.
    ///
    /// Returns the session ID. If a model_id is provided, it is recorded
    /// in the session metadata.
    pub async fn create_session(
        &self,
        model_id: Option<String>,
        history: Vec<ChatMessage>,
    ) -> String {
        let session_id = uuid::Uuid::new_v4().to_string();
        let session = AgentSession {
            id: session_id.clone(),
            model_id,
            messages: history,
            status: LocalAgentStatus::Idle,
            created_at: chrono::Utc::now(),
            tool_executions: Vec::new(),
            dynamic_context: None,
            system_prompt_override: None,
            prior_writes: Vec::new(),
            routing_disabled: false,
        };

        let cancel = CancellationToken::new();
        self.sessions
            .write()
            .await
            .insert(session_id.clone(), session);
        self.cancel_tokens
            .write()
            .await
            .insert(session_id.clone(), cancel);

        session_id
    }

    /// Set the dynamic workspace context for a session.
    ///
    /// Called after session creation once NodeService is available to
    /// populate schemas, collections, and playbooks for the system prompt.
    pub async fn set_session_context(&self, session_id: &str, context: String) {
        let mut sessions = self.sessions.write().await;
        if let Some(session) = sessions.get_mut(session_id) {
            session.dynamic_context = Some(context);
        }
    }

    /// Seed the writes completed by earlier turns of this conversation.
    ///
    /// The tool-execution path uses these to refuse a repeat of a write that
    /// already landed in a prior turn. Callers that do not persist conversation
    /// history simply never call this.
    pub async fn set_session_prior_writes(
        &self,
        session_id: &str,
        prior_writes: Vec<crate::agent_types::PriorWrite>,
    ) {
        let mut sessions = self.sessions.write().await;
        if let Some(session) = sessions.get_mut(session_id) {
            session.prior_writes = prior_writes;
        }
    }

    /// Disable Stage-2 candidate-block injection for this session.
    ///
    /// Called once, right after session creation, with the caller's cached
    /// routing-probe verdict for the model this session will run on (see
    /// `local_agent::routing_probe`). The loop itself never probes or knows
    /// why injection is disabled — this is a one-way switch a caller with
    /// that context sets before the first turn.
    pub async fn set_session_routing_disabled(&self, session_id: &str, disabled: bool) {
        let mut sessions = self.sessions.write().await;
        if let Some(session) = sessions.get_mut(session_id) {
            session.routing_disabled = disabled;
        }
    }

    /// Override the full system prompt for a session.
    ///
    /// When set, this bypasses both `PromptAssembler` and the emergency fallback.
    /// Intended for integration tests that want to inject a pre-built prompt
    /// (constructed via `PromptAssembler::assemble_static`) without a live database.
    ///
    /// Gated by the `testing` Cargo feature so it does not leak into the
    /// production API surface.
    #[cfg(any(test, feature = "testing"))]
    pub async fn set_system_prompt(&self, session_id: &str, prompt: String) {
        let mut sessions = self.sessions.write().await;
        if let Some(session) = sessions.get_mut(session_id) {
            session.system_prompt_override = Some(prompt);
        }
    }

    /// Send a user message and run the agent turn.
    ///
    /// Returns the agent's response after potentially multiple rounds
    /// of tool execution. Streams chunks and status updates via callbacks.
    pub async fn send_message(
        &self,
        session_id: &str,
        message: &str,
        on_status: impl Fn(LocalAgentStatus) + Send + Sync + 'static,
        on_chunk: impl Fn(StreamingChunk) + Send + Sync + 'static,
    ) -> Result<AgentTurnResult, InferenceError> {
        let cancel = {
            let tokens = self.cancel_tokens.read().await;
            tokens
                .get(session_id)
                .cloned()
                .ok_or_else(|| InferenceError::Engine(format!("session not found: {session_id}")))?
        };

        // Take session out for mutation, put it back after
        let mut session = {
            let mut sessions = self.sessions.write().await;
            sessions
                .remove(session_id)
                .ok_or_else(|| InferenceError::Engine(format!("session not found: {session_id}")))?
        };

        let result = self
            .agent_loop
            .run_turn(&mut session, message, on_status, on_chunk, cancel)
            .await;

        // Put session back
        self.sessions
            .write()
            .await
            .insert(session_id.to_string(), session);

        result
    }

    /// Cancel an in-progress generation for the given session.
    pub async fn cancel(&self, session_id: &str) {
        let mut tokens = self.cancel_tokens.write().await;
        if let Some(token) = tokens.get(session_id) {
            token.cancel();
        }
        // Replace with a fresh token for future use
        tokens.insert(session_id.to_string(), CancellationToken::new());
    }

    /// End and remove a session, freeing all resources.
    pub async fn end_session(&self, session_id: &str) {
        self.sessions.write().await.remove(session_id);
        if let Some(token) = self.cancel_tokens.write().await.remove(session_id) {
            token.cancel();
        }
    }

    /// List all active sessions (id + status).
    pub async fn get_sessions(&self) -> Vec<(String, LocalAgentStatus)> {
        self.sessions
            .read()
            .await
            .iter()
            .map(|(id, s)| (id.clone(), s.status.clone()))
            .collect()
    }

    /// Get a snapshot of a session's current state.
    pub async fn get_session(&self, session_id: &str) -> Option<AgentSession> {
        self.sessions.read().await.get(session_id).cloned()
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent_types::{
        ChatModelSpec, ModelFamily, PriorWrite, ToolDefinition, ToolError, ToolResult,
    };
    use async_trait::async_trait;
    use serde_json::json;
    use std::sync::atomic::{AtomicUsize, Ordering};

    // Compile-time coupling check: `multi_skill_turn_invokes_skill_tools_between_searches`
    // feeds exactly 5 inference rounds (search_nodes → search_semantic →
    // search_nodes → create_node → final text) and expects the loop to make
    // it through all of them without hitting the iteration-cap fallback.
    // If `MAX_TOOL_ITERATIONS` is ever reduced below 5, that test would
    // silently start asserting on the fallback path instead of the multi-skill
    // chain — fail loudly here at compile time rather than mysteriously at
    // test time.
    const _: () = assert!(
        MAX_TOOL_ITERATIONS >= 5,
        "multi_skill_turn_invokes_skill_tools_between_searches requires MAX_TOOL_ITERATIONS >= 5",
    );

    // -- char_preview -----------------------------------------------------

    #[test]
    fn char_preview_reports_no_truncation_when_the_string_fits() {
        let (preview, truncated) = char_preview("short", 200);
        assert_eq!(preview, "short");
        assert!(!truncated);
    }

    #[test]
    fn char_preview_reports_truncation_and_the_right_prefix_when_it_does_not_fit() {
        let (preview, truncated) = char_preview("abcdefghij", 5);
        assert_eq!(preview, "abcde");
        assert!(truncated);
    }

    #[test]
    fn char_preview_is_char_boundary_safe_on_multi_byte_content() {
        // Each of these is a multi-byte UTF-8 character. A byte-slice `[..3]`
        // on a string built from them would panic (splits a character); a
        // char-based take must not.
        let s = "日本語のテキストです";
        let (preview, truncated) = char_preview(s, 3);
        assert_eq!(preview, "日本語");
        assert!(truncated);
    }

    #[test]
    fn char_preview_treats_exact_length_as_not_truncated() {
        let (preview, truncated) = char_preview("exact", 5);
        assert_eq!(preview, "exact");
        assert!(
            !truncated,
            "a string exactly at the limit was not cut, so is not truncated"
        );
    }

    // -- Mock inference engine -------------------------------------------

    /// Mock engine that returns pre-configured responses.
    struct MockEngine {
        /// Responses to return for sequential calls to `generate`.
        /// Each entry is a list of chunks to emit.
        responses: tokio::sync::Mutex<Vec<Vec<StreamingChunk>>>,
        generate_count: AtomicUsize,
        /// Effective context window reported by `model_info` — the summarization
        /// gate budgets against this, so tests can exercise a reduced window.
        context_window: u32,
    }

    impl MockEngine {
        fn new(responses: Vec<Vec<StreamingChunk>>) -> Self {
            Self {
                responses: tokio::sync::Mutex::new(responses),
                generate_count: AtomicUsize::new(0),
                context_window: 8192,
            }
        }

        /// Same as `new` but with an explicit effective context window.
        fn with_context_window(responses: Vec<Vec<StreamingChunk>>, context_window: u32) -> Self {
            Self {
                responses: tokio::sync::Mutex::new(responses),
                generate_count: AtomicUsize::new(0),
                context_window,
            }
        }

        /// Create a mock that returns a single text response (no tools).
        fn single_text(text: &str) -> Self {
            Self::new(vec![vec![
                StreamingChunk::Token {
                    text: text.to_string(),
                },
                StreamingChunk::Done {
                    usage: InferenceUsage {
                        prompt_tokens: 10,
                        completion_tokens: 5,
                    },
                },
            ]])
        }

        /// Create a mock that first returns a tool call, then a text response.
        fn tool_then_text(tool_name: &str, tool_args: &str, final_text: &str) -> Self {
            Self::new(vec![
                // First call: tool call
                vec![
                    StreamingChunk::ToolCallStart {
                        id: "tc_1".to_string(),
                        name: tool_name.to_string(),
                        provider_extra: None,
                    },
                    StreamingChunk::ToolCallArgs {
                        id: "tc_1".to_string(),
                        args_json: tool_args.to_string(),
                    },
                    StreamingChunk::Done {
                        usage: InferenceUsage {
                            prompt_tokens: 20,
                            completion_tokens: 10,
                        },
                    },
                ],
                // Second call: final text
                vec![
                    StreamingChunk::Token {
                        text: final_text.to_string(),
                    },
                    StreamingChunk::Done {
                        usage: InferenceUsage {
                            prompt_tokens: 30,
                            completion_tokens: 15,
                        },
                    },
                ],
            ])
        }
    }

    #[async_trait]
    impl ChatInferenceEngine for MockEngine {
        async fn generate(
            &self,
            _request: InferenceRequest,
            on_chunk: Box<dyn Fn(StreamingChunk) + Send>,
        ) -> Result<InferenceUsage, InferenceError> {
            let idx = self.generate_count.fetch_add(1, Ordering::SeqCst);
            let responses = self.responses.lock().await;

            if idx >= responses.len() {
                // Return empty response if we run out of pre-configured ones
                on_chunk(StreamingChunk::Done {
                    usage: InferenceUsage {
                        prompt_tokens: 0,
                        completion_tokens: 0,
                    },
                });
                return Ok(InferenceUsage {
                    prompt_tokens: 0,
                    completion_tokens: 0,
                });
            }

            let chunks = &responses[idx];
            let mut usage = InferenceUsage {
                prompt_tokens: 0,
                completion_tokens: 0,
            };
            for chunk in chunks {
                if let StreamingChunk::Done { usage: u } = chunk {
                    usage = *u;
                }
                on_chunk(chunk.clone());
            }
            Ok(usage)
        }

        async fn model_info(&self) -> Result<Option<ChatModelSpec>, InferenceError> {
            Ok(Some(ChatModelSpec {
                model_id: "test-model".into(),
                family: ModelFamily::Ministral,
                context_window: self.context_window,
                default_temperature: 0.1,
                type_k: None,
                type_v: None,
            }))
        }

        async fn token_count(&self, text: &str) -> Result<u32, InferenceError> {
            // Rough estimate: ~4 chars per token
            Ok((text.len() as f32 / 4.0).ceil() as u32)
        }
    }

    // -- Mock tool executor ----------------------------------------------

    struct MockToolExecutor {
        tools: Vec<ToolDefinition>,
        /// Canned results keyed by tool name.
        results: HashMap<String, serde_json::Value>,
    }

    impl MockToolExecutor {
        fn new() -> Self {
            Self {
                tools: Vec::new(),
                results: HashMap::new(),
            }
            .with_tool(
                "search_nodes",
                json!({"type": "object", "properties": {"query": {"type": "string"}}, "required": ["query"]}),
                json!({"count": 2, "nodes": [
                    {"id": "abc123", "title": "Billing Architecture", "type": "text"},
                    {"id": "def456", "title": "Payment Processing", "type": "text"},
                ]}),
            )
            .with_tool(
                "get_node",
                json!({"type": "object", "properties": {"id": {"type": "string"}}, "required": ["id"]}),
                json!({"id": "abc123", "title": "Billing Architecture", "body": "Content here"}),
            )
        }

        /// Register an additional tool with its JSON schema and canned result.
        ///
        /// Lets tests express "I expect the agent to call X with shape Y, and
        /// when it does, return Z" in one call instead of poking at the
        /// internal `tools` / `results` fields directly.
        fn with_tool(
            mut self,
            name: &str,
            parameters_schema: serde_json::Value,
            result: serde_json::Value,
        ) -> Self {
            self.tools.push(ToolDefinition {
                name: name.into(),
                description: format!("Mock tool: {name}"),
                parameters_schema,
            });
            self.results.insert(name.to_string(), result);
            self
        }
    }

    #[async_trait]
    impl AgentToolExecutor for MockToolExecutor {
        async fn available_tools(&self) -> Result<Vec<ToolDefinition>, ToolError> {
            Ok(self.tools.clone())
        }

        async fn execute(
            &self,
            name: &str,
            _args: serde_json::Value,
        ) -> Result<ToolResult, ToolError> {
            let result = self
                .results
                .get(name)
                .cloned()
                .unwrap_or(json!({"error": "unknown tool"}));
            let is_error = !self.results.contains_key(name);
            Ok(ToolResult {
                tool_call_id: format!("call_{name}"),
                name: name.to_string(),
                result,
                is_error,
            })
        }
    }

    /// Executor that records every call it actually performs.
    ///
    /// The duplicate guard's whole point is that a call never reaches the
    /// executor, so asserting on the *result* alone is not enough — a guard that
    /// executed the write and then relabelled the result would pass such a
    /// check. This records ground truth.
    struct RecordingToolExecutor {
        inner: MockToolExecutor,
        calls: Arc<std::sync::Mutex<Vec<String>>>,
    }

    impl RecordingToolExecutor {
        fn new(inner: MockToolExecutor) -> Self {
            Self {
                inner,
                calls: Arc::new(std::sync::Mutex::new(Vec::new())),
            }
        }

        fn calls_handle(&self) -> Arc<std::sync::Mutex<Vec<String>>> {
            Arc::clone(&self.calls)
        }
    }

    #[async_trait]
    impl AgentToolExecutor for RecordingToolExecutor {
        async fn available_tools(&self) -> Result<Vec<ToolDefinition>, ToolError> {
            self.inner.available_tools().await
        }

        async fn execute(
            &self,
            name: &str,
            args: serde_json::Value,
        ) -> Result<ToolResult, ToolError> {
            self.calls.lock().unwrap().push(name.to_string());
            self.inner.execute(name, args).await
        }
    }

    // -- Helper to create a fresh session --------------------------------

    fn new_session() -> AgentSession {
        AgentSession {
            id: "test-session".to_string(),
            model_id: Some("test-model".to_string()),
            messages: Vec::new(),
            status: LocalAgentStatus::Idle,
            created_at: chrono::Utc::now(),
            tool_executions: Vec::new(),
            dynamic_context: None,
            system_prompt_override: None,
            prior_writes: Vec::new(),
            routing_disabled: false,
        }
    }

    // -- Tests -----------------------------------------------------------

    #[tokio::test]
    async fn single_turn_no_tools() {
        let engine = Arc::new(MockEngine::single_text("Hello! How can I help?"));
        let executor = Arc::new(MockToolExecutor::new());
        let agent_loop = LocalAgentLoop::new(engine, executor);

        let mut session = new_session();
        let statuses: Arc<std::sync::Mutex<Vec<LocalAgentStatus>>> =
            Arc::new(std::sync::Mutex::new(Vec::new()));
        let statuses_cb = Arc::clone(&statuses);
        let chunks: Arc<std::sync::Mutex<Vec<StreamingChunk>>> =
            Arc::new(std::sync::Mutex::new(Vec::new()));
        let chunks_cb = Arc::clone(&chunks);

        let result = agent_loop
            .run_turn(
                &mut session,
                "Summarize the GitHub release notes for v1.2 in plain English",
                move |s| {
                    statuses_cb.lock().unwrap().push(s);
                },
                move |c| {
                    chunks_cb.lock().unwrap().push(c);
                },
                CancellationToken::new(),
            )
            .await
            .unwrap();

        assert_eq!(result.response, "Hello! How can I help?");
        assert!(result.tool_calls_made.is_empty());
        assert!(result.usage.prompt_tokens > 0);
        // #1930: an ordinary reply — even one phrased conversationally — is
        // not a `route_clarify` turn and must carry no structured prompt.
        assert!(result.clarify.is_none());

        // Session should have 2 messages: user + assistant
        assert_eq!(session.messages.len(), 2);
        assert_eq!(session.messages[0].role, Role::User);
        assert_eq!(session.messages[1].role, Role::Assistant);
        assert_eq!(session.status, LocalAgentStatus::Idle);
    }

    #[tokio::test]
    async fn tool_call_then_final_response() {
        let engine = Arc::new(MockEngine::tool_then_text(
            "search_nodes",
            r#"{"query":"billing"}"#,
            "Found 2 nodes about billing.",
        ));
        let executor = Arc::new(MockToolExecutor::new());
        let agent_loop = LocalAgentLoop::new(engine, executor);

        let mut session = new_session();
        let result = agent_loop
            .run_turn(
                &mut session,
                "Search GitHub for open release-blocker issues then summarize them",
                |_| {},
                |_| {},
                CancellationToken::new(),
            )
            .await
            .unwrap();

        assert_eq!(result.response, "Found 2 nodes about billing.");
        assert_eq!(result.tool_calls_made.len(), 1);
        assert_eq!(result.tool_calls_made[0].name, "search_nodes");

        // Session should have: user, assistant (tool call), tool result, assistant (final)
        assert_eq!(session.messages.len(), 4);
        assert_eq!(session.messages[0].role, Role::User);
        assert_eq!(session.messages[1].role, Role::Assistant);
        assert_eq!(session.messages[2].role, Role::Tool);
        assert_eq!(session.messages[3].role, Role::Assistant);
    }

    #[tokio::test]
    async fn multi_step_tool_chain() {
        // First: search_nodes, Second: get_node, Third: final text
        let engine = Arc::new(MockEngine::new(vec![
            // Round 1: search_nodes
            vec![
                StreamingChunk::ToolCallStart {
                    id: "tc_1".to_string(),
                    name: "search_nodes".to_string(),
                    provider_extra: None,
                },
                StreamingChunk::ToolCallArgs {
                    id: "tc_1".to_string(),
                    args_json: r#"{"query":"architecture"}"#.to_string(),
                },
                StreamingChunk::Done {
                    usage: InferenceUsage {
                        prompt_tokens: 20,
                        completion_tokens: 10,
                    },
                },
            ],
            // Round 2: get_node
            vec![
                StreamingChunk::ToolCallStart {
                    id: "tc_2".to_string(),
                    name: "get_node".to_string(),
                    provider_extra: None,
                },
                StreamingChunk::ToolCallArgs {
                    id: "tc_2".to_string(),
                    args_json: r#"{"id":"abc123"}"#.to_string(),
                },
                StreamingChunk::Done {
                    usage: InferenceUsage {
                        prompt_tokens: 40,
                        completion_tokens: 10,
                    },
                },
            ],
            // Round 3: final response
            vec![
                StreamingChunk::Token {
                    text: "The Billing Architecture node describes...".to_string(),
                },
                StreamingChunk::Done {
                    usage: InferenceUsage {
                        prompt_tokens: 60,
                        completion_tokens: 20,
                    },
                },
            ],
        ]));
        let executor = Arc::new(MockToolExecutor::new());
        let agent_loop = LocalAgentLoop::new(engine, executor);

        let mut session = new_session();
        let result = agent_loop
            .run_turn(
                &mut session,
                "Look up the Billing Architecture node then fetch its referenced details",
                |_| {},
                |_| {},
                CancellationToken::new(),
            )
            .await
            .unwrap();

        assert!(result
            .response
            .contains("Billing Architecture node describes"));
        assert_eq!(result.tool_calls_made.len(), 2);
        assert_eq!(result.tool_calls_made[0].name, "search_nodes");
        assert_eq!(result.tool_calls_made[1].name, "get_node");

        // Total usage should sum all rounds
        assert_eq!(result.usage.prompt_tokens, 120); // 20+40+60
        assert_eq!(result.usage.completion_tokens, 40); // 10+10+20
    }

    #[tokio::test]
    async fn max_iteration_limit() {
        // Each round returns a DISTINCT query so the duplicate-call guard does
        // NOT fire — this test exercises the pure iteration-cap path.
        let tool_round = |i: usize| {
            vec![
                StreamingChunk::ToolCallStart {
                    id: format!("tc_{i}"),
                    name: "search_nodes".to_string(),
                    provider_extra: None,
                },
                StreamingChunk::ToolCallArgs {
                    id: format!("tc_{i}"),
                    args_json: format!(r#"{{"query":"test-{i}"}}"#),
                },
                StreamingChunk::Done {
                    usage: InferenceUsage {
                        prompt_tokens: 10,
                        completion_tokens: 5,
                    },
                },
            ]
        };

        // Provide more rounds than the limit; the loop must stop at MAX_TOOL_ITERATIONS.
        // +1 extra for the final tool-less inference call.
        let rounds: Vec<_> = (0..MAX_TOOL_ITERATIONS + 2).map(tool_round).collect();
        let engine = Arc::new(MockEngine::new(rounds));
        let executor = Arc::new(MockToolExecutor::new());
        let agent_loop = LocalAgentLoop::new(engine, executor);

        let mut session = new_session();
        let result = agent_loop
            .run_turn(
                &mut session,
                "Keep running search_nodes forever — verify the iteration cap stops it",
                |_| {},
                |_| {},
                CancellationToken::new(),
            )
            .await
            .unwrap();

        // Should have executed exactly MAX_TOOL_ITERATIONS tool calls (the limit)
        assert_eq!(result.tool_calls_made.len(), MAX_TOOL_ITERATIONS);
        // All should be search_nodes
        for tc in &result.tool_calls_made {
            assert_eq!(tc.name, "search_nodes");
        }

        // The fallback response must encode the invariant:
        // no raw tool identifier reaches the UI.
        assert!(
            !result.response.contains("search_nodes"),
            "fallback response leaked raw tool name: {:?}",
            result.response
        );
    }

    /// Duplicate-call guard: when the model issues the same tool+args pair
    /// it already executed in this turn, the loop breaks immediately so the
    /// final-inference step can synthesise a response from what's in history.
    ///
    /// Engine call sequence:
    ///   Round 0: first call → executed normally (added to seen_calls)
    ///   Round 1: identical call → guard fires, loop breaks (round NOT executed)
    ///   Round 2: tail text-only inference → returns the real response
    #[tokio::test]
    async fn duplicate_tool_call_breaks_loop() {
        let dup_call = || {
            vec![
                StreamingChunk::ToolCallStart {
                    id: "tc_dup".to_string(),
                    name: "search_nodes".to_string(),
                    provider_extra: None,
                },
                StreamingChunk::ToolCallArgs {
                    id: "tc_dup".to_string(),
                    args_json: r#"{"node_type":"task","query":"Test Task"}"#.to_string(),
                },
                StreamingChunk::Done {
                    usage: InferenceUsage {
                        prompt_tokens: 10,
                        completion_tokens: 5,
                    },
                },
            ]
        };

        let rounds = vec![
            // Round 0: first call — executes normally
            dup_call(),
            // Round 1: identical call — guard detects duplicate, breaks loop
            dup_call(),
            // Round 2: tail tool-less inference — returns text
            vec![
                StreamingChunk::Token {
                    text: "I found the task.".to_string(),
                },
                StreamingChunk::Done {
                    usage: InferenceUsage {
                        prompt_tokens: 20,
                        completion_tokens: 5,
                    },
                },
            ],
        ];

        let engine = Arc::new(MockEngine::new(rounds));
        let executor = Arc::new(MockToolExecutor::new());
        let agent_loop = LocalAgentLoop::new(engine, executor);

        let mut session = new_session();
        let result = agent_loop
            .run_turn(
                &mut session,
                "Find the task named Test Task",
                |_| {},
                |_| {},
                CancellationToken::new(),
            )
            .await
            .unwrap();

        // Guard fires before executing round 1 — exactly 1 tool call recorded.
        assert_eq!(
            result.tool_calls_made.len(),
            1,
            "duplicate guard must break after first execution, got: {:?}",
            result
                .tool_calls_made
                .iter()
                .map(|t| &t.name)
                .collect::<Vec<_>>()
        );
        assert_eq!(result.tool_calls_made[0].name, "search_nodes");
        // Tail inference text is returned (not a synthesised fallback).
        assert!(
            result.response.contains("found the task"),
            "expected tail-inference response, got: {:?}",
            result.response
        );
    }

    // -- humanize_tool_name ------------------------------------------------

    #[test]
    fn humanize_tool_name_known_tools() {
        assert_eq!(humanize_tool_name("create_schema"), "schema creation");
        assert_eq!(humanize_tool_name("update_node"), "node update");
        assert_eq!(humanize_tool_name("search_semantic"), "semantic search");
        assert_eq!(humanize_tool_name("delete_node"), "node deletion");
    }

    #[test]
    fn humanize_tool_name_unknown_falls_back_to_generic() {
        // Unknown identifiers must NOT leak through verbatim — they map to a
        // generic phrase so the chat UI never displays an internal name.
        assert_eq!(
            humanize_tool_name("some_future_tool"),
            "the requested action"
        );
        assert_eq!(humanize_tool_name(""), "the requested action");
    }

    // The former `humanize_tool_name_covers_all_registered_tools` drift detector
    // is gone: `humanize_tool_name` now derives from `Tool`, whose `humanized()`
    // arm is exhaustive over the registry, so coverage holds by construction.

    #[tokio::test]
    async fn cancellation_stops_generation() {
        let engine = Arc::new(MockEngine::single_text("Should not complete"));
        let executor = Arc::new(MockToolExecutor::new());
        let agent_loop = LocalAgentLoop::new(engine, executor);

        let mut session = new_session();
        let cancel = CancellationToken::new();
        cancel.cancel(); // Cancel immediately

        let result = agent_loop
            .run_turn(
                &mut session,
                "Begin generating a long answer about the GitHub release process",
                |_| {},
                |_| {},
                cancel,
            )
            .await;

        assert!(result.is_err());
        match result.unwrap_err() {
            InferenceError::Engine(msg) => assert_eq!(msg, "cancelled"),
            other => panic!("Expected Engine error, got {:?}", other),
        }
    }

    #[tokio::test]
    async fn parse_chunks_text_only() {
        let chunks = vec![
            StreamingChunk::Token {
                text: "Hello ".to_string(),
            },
            StreamingChunk::Token {
                text: "world".to_string(),
            },
            StreamingChunk::Done {
                usage: InferenceUsage {
                    prompt_tokens: 5,
                    completion_tokens: 2,
                },
            },
        ];
        let (text, reasoning, tool_calls, error) =
            LocalAgentLoop::<MockEngine, MockToolExecutor>::parse_chunks(&chunks);
        assert_eq!(text, "Hello world");
        assert!(reasoning.is_empty());
        assert!(tool_calls.is_empty());
        assert!(error.is_none());
    }

    #[tokio::test]
    async fn parse_chunks_separates_reasoning_from_answer() {
        let chunks = vec![
            StreamingChunk::Reasoning {
                text: "The user said hi; ".to_string(),
            },
            StreamingChunk::Token {
                text: "Hello!".to_string(),
            },
            StreamingChunk::Reasoning {
                text: "I should greet back.".to_string(),
            },
            StreamingChunk::Done {
                usage: InferenceUsage {
                    prompt_tokens: 3,
                    completion_tokens: 2,
                },
            },
        ];
        let (text, reasoning, tool_calls, error) =
            LocalAgentLoop::<MockEngine, MockToolExecutor>::parse_chunks(&chunks);
        assert_eq!(text, "Hello!");
        assert_eq!(reasoning, "The user said hi; I should greet back.");
        assert!(tool_calls.is_empty());
        assert!(error.is_none());
    }

    #[tokio::test]
    async fn parse_chunks_with_tool_calls() {
        let chunks = vec![
            StreamingChunk::Token {
                text: "Let me search".to_string(),
            },
            StreamingChunk::ToolCallStart {
                id: "tc_1".to_string(),
                name: "search_nodes".to_string(),
                provider_extra: None,
            },
            StreamingChunk::ToolCallArgs {
                id: "tc_1".to_string(),
                args_json: r#"{"query":""#.to_string(),
            },
            StreamingChunk::ToolCallArgs {
                id: "tc_1".to_string(),
                args_json: r#"test"}"#.to_string(),
            },
            StreamingChunk::Done {
                usage: InferenceUsage {
                    prompt_tokens: 10,
                    completion_tokens: 5,
                },
            },
        ];
        let (text, _reasoning, tool_calls, error) =
            LocalAgentLoop::<MockEngine, MockToolExecutor>::parse_chunks(&chunks);
        assert_eq!(text, "Let me search");
        assert_eq!(tool_calls.len(), 1);
        assert_eq!(tool_calls[0].function_name, "search_nodes");
        assert_eq!(tool_calls[0].arguments_json, r#"{"query":"test"}"#);
        assert!(error.is_none());
    }

    #[tokio::test]
    async fn parse_chunks_captures_the_first_error() {
        let chunks = vec![
            StreamingChunk::Token {
                text: "partial answer".to_string(),
            },
            StreamingChunk::Error {
                message: "Context window full".to_string(),
            },
            // A second error chunk, if one ever arrived, must not overwrite
            // the first -- the first is what actually caused the failure.
            StreamingChunk::Error {
                message: "a later, unrelated error".to_string(),
            },
        ];
        let (text, _reasoning, _tool_calls, error) =
            LocalAgentLoop::<MockEngine, MockToolExecutor>::parse_chunks(&chunks);
        assert_eq!(text, "partial answer");
        assert_eq!(error, Some("Context window full".to_string()));
    }

    // -- LocalAgentService tests -----------------------------------------

    #[tokio::test]
    async fn service_create_and_list_sessions() {
        let engine = Arc::new(MockEngine::single_text("Hello"));
        let executor = Arc::new(MockToolExecutor::new());
        let service = LocalAgentService::new(engine, executor);

        let id1 = service.create_session(Some("model-a".into()), vec![]).await;
        let id2 = service.create_session(None, vec![]).await;

        let sessions = service.get_sessions().await;
        assert_eq!(sessions.len(), 2);

        let session1 = service.get_session(&id1).await.unwrap();
        assert_eq!(session1.model_id, Some("model-a".to_string()));

        let session2 = service.get_session(&id2).await.unwrap();
        assert_eq!(session2.model_id, None);
    }

    #[tokio::test]
    async fn service_end_session() {
        let engine = Arc::new(MockEngine::single_text("Hello"));
        let executor = Arc::new(MockToolExecutor::new());
        let service = LocalAgentService::new(engine, executor);

        let id = service.create_session(None, vec![]).await;
        assert!(service.get_session(&id).await.is_some());

        service.end_session(&id).await;
        assert!(service.get_session(&id).await.is_none());
        assert!(service.get_sessions().await.is_empty());
    }

    #[tokio::test]
    async fn service_send_message() {
        let engine = Arc::new(MockEngine::single_text("I can help with that!"));
        let executor = Arc::new(MockToolExecutor::new());
        let service = LocalAgentService::new(engine, executor);

        let id = service.create_session(None, vec![]).await;
        let result = service
            .send_message(
                &id,
                "Send this message to the agent and confirm a GitHub release reply comes back",
                |_| {},
                |_| {},
            )
            .await
            .unwrap();

        assert_eq!(result.response, "I can help with that!");

        // Session should still exist with messages
        let session = service.get_session(&id).await.unwrap();
        assert_eq!(session.messages.len(), 2);
    }

    #[tokio::test]
    async fn service_send_message_unknown_session() {
        let engine = Arc::new(MockEngine::single_text("Hello"));
        let executor = Arc::new(MockToolExecutor::new());
        let service = LocalAgentService::new(engine, executor);

        let result = service
            .send_message("nonexistent", "Hello", |_| {}, |_| {})
            .await;

        assert!(result.is_err());
    }

    #[tokio::test]
    async fn service_cancel_session() {
        let engine = Arc::new(MockEngine::single_text("Hello"));
        let executor = Arc::new(MockToolExecutor::new());
        let service = LocalAgentService::new(engine, executor);

        let id = service.create_session(None, vec![]).await;

        // Cancel should not panic even if nothing is in progress
        service.cancel(&id).await;

        // Session should still be usable after cancel
        let session = service.get_session(&id).await;
        assert!(session.is_some());
    }

    #[tokio::test]
    async fn status_transitions_single_turn() {
        let engine = Arc::new(MockEngine::single_text("Response"));
        let executor = Arc::new(MockToolExecutor::new());
        let agent_loop = LocalAgentLoop::new(engine, executor);

        let mut session = new_session();
        let statuses: Arc<std::sync::Mutex<Vec<String>>> =
            Arc::new(std::sync::Mutex::new(Vec::new()));
        let statuses_cb = Arc::clone(&statuses);

        agent_loop
            .run_turn(
                &mut session,
                "Walk through each status transition while answering about GitHub releases",
                move |s| {
                    let label = match &s {
                        LocalAgentStatus::Idle => "Idle",
                        LocalAgentStatus::Thinking => "Thinking",
                        LocalAgentStatus::ToolExecution { .. } => "ToolExecution",
                        LocalAgentStatus::Streaming => "Streaming",
                        LocalAgentStatus::Error { .. } => "Error",
                    };
                    statuses_cb.lock().unwrap().push(label.to_string());
                },
                |_| {},
                CancellationToken::new(),
            )
            .await
            .unwrap();

        let statuses = statuses.lock().unwrap();
        // Should be: Thinking, Streaming, Idle
        assert!(statuses.contains(&"Thinking".to_string()));
        assert!(statuses.contains(&"Streaming".to_string()));
        assert!(statuses.contains(&"Idle".to_string()));
    }

    #[tokio::test]
    async fn status_transitions_with_tool() {
        let engine = Arc::new(MockEngine::tool_then_text(
            "search_nodes",
            r#"{"query":"test"}"#,
            "Done",
        ));
        let executor = Arc::new(MockToolExecutor::new());
        let agent_loop = LocalAgentLoop::new(engine, executor);

        let mut session = new_session();
        let statuses: Arc<std::sync::Mutex<Vec<String>>> =
            Arc::new(std::sync::Mutex::new(Vec::new()));
        let statuses_cb = Arc::clone(&statuses);

        agent_loop
            .run_turn(
                &mut session,
                "Search GitHub release notes and report each status transition along the way",
                move |s| {
                    let label = match &s {
                        LocalAgentStatus::Idle => "Idle",
                        LocalAgentStatus::Thinking => "Thinking",
                        LocalAgentStatus::ToolExecution { .. } => "ToolExecution",
                        LocalAgentStatus::Streaming => "Streaming",
                        LocalAgentStatus::Error { .. } => "Error",
                    };
                    statuses_cb.lock().unwrap().push(label.to_string());
                },
                |_| {},
                CancellationToken::new(),
            )
            .await
            .unwrap();

        let statuses = statuses.lock().unwrap();
        // Should include: Thinking (round 1), ToolExecution, Thinking (round 2), Streaming, Idle
        assert!(statuses.contains(&"Thinking".to_string()));
        assert!(statuses.contains(&"ToolExecution".to_string()));
        assert!(statuses.contains(&"Idle".to_string()));
    }

    #[tokio::test]
    async fn history_summarization_trigger() {
        // Create an engine that always returns text (no tools) but we
        // pre-populate the session with enough history to trigger summarization.
        let engine = Arc::new(MockEngine::new(vec![
            // Summarization call
            vec![
                StreamingChunk::Token {
                    text: "Summary: user discussed billing and payments.".to_string(),
                },
                StreamingChunk::Done {
                    usage: InferenceUsage {
                        prompt_tokens: 50,
                        completion_tokens: 20,
                    },
                },
            ],
            // Actual response
            vec![
                StreamingChunk::Token {
                    text: "Here is your answer.".to_string(),
                },
                StreamingChunk::Done {
                    usage: InferenceUsage {
                        prompt_tokens: 30,
                        completion_tokens: 10,
                    },
                },
            ],
        ]));
        let executor = Arc::new(MockToolExecutor::new());
        let agent_loop = LocalAgentLoop::new(engine, executor);

        let mut session = new_session();

        // Add enough history to exceed TOTAL_TOKEN_BUDGET (32000 tokens).
        // With ~4 chars/token estimate, we need > 32000*4 = 128000 chars.
        // 20 messages * 7000 chars = 140000 chars = ~35000 tokens > 32000 budget.
        for i in 0..20 {
            let role = if i % 2 == 0 {
                Role::User
            } else {
                Role::Assistant
            };
            session.messages.push(ChatMessage::text(
                role,
                format!("Message {} with extensive content: {}", i, "x".repeat(7000)),
            ));
        }

        let messages_before = session.messages.len();

        let result = agent_loop
            .run_turn(
                &mut session,
                "Recap the prior Billing conversation after triggering history summarization",
                |_| {},
                |_| {},
                CancellationToken::new(),
            )
            .await
            .unwrap();

        // After summarization, older messages should be replaced with a summary.
        // Without summarization, count would be: 20 (pre-existing) + 1 (user) + 1 (assistant) = 22.
        // With summarization, it should be: 1 (summary) + 3 (kept) + 1 (user) + 1 (assistant) = 6 or similar.
        assert!(
            session.messages.len() < messages_before,
            "Expected summarization to reduce message count. Before: {}, After: {}",
            messages_before,
            session.messages.len()
        );

        // The first message should be the summary
        assert!(
            session.messages[0]
                .content
                .contains("[Conversation summary]"),
            "First message should be the summary, got: {}",
            session.messages[0].content
        );

        assert_eq!(result.response, "Here is your answer.");
    }

    /// A model whose effective context window was reduced to fit memory must
    /// summarize when history exceeds *that* window — not the old
    /// hardcoded 32K budget. Here history is ~10K tokens: comfortably under the
    /// former 32K constant, but well over a 4096-token effective window. Before
    /// budgeting against the effective window, this history would sail past the
    /// gate and then be rejected by the engine as ContextOverflow.
    #[tokio::test]
    async fn summarization_triggers_on_reduced_effective_window() {
        let engine = Arc::new(MockEngine::with_context_window(
            vec![
                // Summarization call
                vec![
                    StreamingChunk::Token {
                        text: "Summary: earlier discussion condensed.".to_string(),
                    },
                    StreamingChunk::Done {
                        usage: InferenceUsage {
                            prompt_tokens: 40,
                            completion_tokens: 10,
                        },
                    },
                ],
                // Final answer
                vec![
                    StreamingChunk::Token {
                        text: "Done.".to_string(),
                    },
                    StreamingChunk::Done {
                        usage: InferenceUsage {
                            prompt_tokens: 30,
                            completion_tokens: 10,
                        },
                    },
                ],
            ],
            4096, // effective window reduced to fit memory
        ));
        let executor = Arc::new(MockToolExecutor::new());
        let agent_loop = LocalAgentLoop::new(engine, executor);

        let mut session = new_session();

        // ~10K tokens (@ ~4 chars/token): under the old 32K constant, over 4096.
        // 20 messages * 2000 chars = 40000 chars = ~10000 tokens.
        for i in 0..20 {
            let role = if i % 2 == 0 {
                Role::User
            } else {
                Role::Assistant
            };
            session.messages.push(ChatMessage::text(
                role,
                format!("Msg {}: {}", i, "y".repeat(2000)),
            ));
        }
        let messages_before = session.messages.len();

        agent_loop
            .run_turn(
                &mut session,
                "Continue the conversation",
                |_| {},
                |_| {},
                CancellationToken::new(),
            )
            .await
            .unwrap();

        assert!(
            session.messages.len() < messages_before,
            "Reduced effective window must trigger summarization. Before: {}, After: {}",
            messages_before,
            session.messages.len()
        );
        assert!(
            session.messages[0]
                .content
                .contains("[Conversation summary]"),
            "First message should be the summary, got: {}",
            session.messages[0].content
        );
    }

    /// Measured: a Metal backend OOM during prefill on a 16 GB machine with
    /// `n_ctx` granted at 32,768 — the context-window budget was nowhere
    /// near full (~8.5K tokens against a ~30K-token budget) when the
    /// *backend* failed. `PREFILL_STABILITY_CEILING` is a second, narrower
    /// gate independent of the context window, so a spacious `n_ctx` does
    /// not let the total prompt (history + system) grow past it just
    /// because gate 1 (the window budget) has room to spare.
    #[tokio::test]
    async fn summarization_triggers_on_stability_ceiling_despite_spacious_window() {
        let engine = Arc::new(MockEngine::with_context_window(
            vec![
                // Summarization call
                vec![
                    StreamingChunk::Token {
                        text: "Summary: earlier discussion condensed.".to_string(),
                    },
                    StreamingChunk::Done {
                        usage: InferenceUsage {
                            prompt_tokens: 40,
                            completion_tokens: 10,
                        },
                    },
                ],
                // Final answer
                vec![
                    StreamingChunk::Token {
                        text: "Done.".to_string(),
                    },
                    StreamingChunk::Done {
                        usage: InferenceUsage {
                            prompt_tokens: 30,
                            completion_tokens: 10,
                        },
                    },
                ],
            ],
            // Matches #2172's granted n_ctx: the window-budget gate (gate 1)
            // has ample room (~30K tokens) at this history size.
            32_768,
        ));
        let executor = Arc::new(MockToolExecutor::new());
        let agent_loop = LocalAgentLoop::new(engine, executor);

        let mut session = new_session();

        // ~8.5K history tokens (@ ~4 chars/token: 20 * 1700 = 34,000 chars).
        // The test harness has no PromptAssembler wired in, so
        // `system_content` falls back to `EMERGENCY_FALLBACK_PROMPT` — a
        // short, fixed string (tens of tokens), unlike production's ~6,600-
        // token tool-registered prompt. Total (history + this small system
        // prompt) still clears PREFILL_STABILITY_CEILING (8,000) while
        // staying far under a 32,768 window budget (~30K after
        // MAX_RESPONSE_TOKENS) — gate 1 alone would not trigger
        // summarization here; only gate 2 does.
        for i in 0..20 {
            let role = if i % 2 == 0 {
                Role::User
            } else {
                Role::Assistant
            };
            session.messages.push(ChatMessage::text(
                role,
                format!("Msg {}: {}", i, "z".repeat(1700)),
            ));
        }
        let messages_before = session.messages.len();

        agent_loop
            .run_turn(
                &mut session,
                "Continue the conversation",
                |_| {},
                |_| {},
                CancellationToken::new(),
            )
            .await
            .unwrap();

        assert!(
            session.messages.len() < messages_before,
            "Stability ceiling must trigger summarization even with a spacious \
             context window. Before: {}, After: {}",
            messages_before,
            session.messages.len()
        );
        assert!(
            session.messages[0]
                .content
                .contains("[Conversation summary]"),
            "First message should be the summary, got: {}",
            session.messages[0].content
        );
    }

    #[tokio::test]
    async fn summarization_never_orphans_a_tool_result() {
        // Build a history that exceeds the token budget and is arranged so the
        // naive "keep last 3" window would START on a Tool message — i.e. its
        // preceding assistant tool-call turn would be drained into the summary,
        // leaving an orphan tool result. The split must back off so the kept
        // window begins on the assistant tool-call turn instead.
        let engine = Arc::new(MockEngine::new(vec![vec![
            StreamingChunk::Token {
                text: "Summary of earlier turns.".to_string(),
            },
            StreamingChunk::Done {
                usage: InferenceUsage {
                    prompt_tokens: 10,
                    completion_tokens: 5,
                },
            },
        ]]));
        let executor = Arc::new(MockToolExecutor::new());
        let agent_loop = LocalAgentLoop::new(engine, executor);

        let mut session = new_session();
        // Bulk filler to blow the budget (≈4 chars/token in the mock).
        for i in 0..18 {
            let role = if i % 2 == 0 {
                Role::User
            } else {
                Role::Assistant
            };
            session
                .messages
                .push(ChatMessage::text(role, "x".repeat(8000)));
        }
        // CRITICAL fixture detail: the tail is
        //   [assistant tool-call, tool result, user, user]
        // so the naive "keep last 3" window is [tool result, user, user] — it
        // STARTS on the orphan tool result, with its assistant tool-call turn
        // sitting just before the cut. This is what actually exercises the
        // back-off; without it (e.g. a 3-element tail) the split lands on the
        // assistant turn and the loop never runs, making the test a tautology.
        session
            .messages
            .push(ChatMessage::assistant_with_tool_calls(
                String::new(),
                vec![ToolCallRaw {
                    id: "tc_1".into(),
                    function_name: "search_nodes".into(),
                    arguments_json: r#"{"query":"q"}"#.into(),
                    provider_extra: None,
                }],
            ));
        session.messages.push(ChatMessage::tool_result(
            "result".to_string(),
            "tc_1".to_string(),
            "search_nodes".to_string(),
        ));
        session
            .messages
            .push(ChatMessage::text(Role::User, "first follow-up"));
        session
            .messages
            .push(ChatMessage::text(Role::User, "second follow-up"));

        agent_loop
            .maybe_summarize_history(&mut session, "system")
            .await
            .unwrap();

        // (1) No orphan: every retained Tool message is immediately preceded by
        // an assistant turn carrying tool_calls.
        for (i, m) in session.messages.iter().enumerate() {
            if matches!(m.role, Role::Tool) {
                assert!(
                    i > 0,
                    "tool result at index 0 has no preceding assistant turn"
                );
                let prev = &session.messages[i - 1];
                assert!(
                    matches!(prev.role, Role::Assistant) && !prev.tool_calls.is_empty(),
                    "tool result at index {i} is orphaned (preceding message is not an assistant tool-call turn)"
                );
            }
        }

        // (2) Back-off actually fired: the assistant tool-call turn (which a naive
        // cut would have drained into the summary) must be RETAINED in the kept
        // window, paired with its tool result. Without the back-off this assertion
        // fails — that is what makes this test exercise the fix rather than pass
        // vacuously.
        let retained_tool_call = session
            .messages
            .iter()
            .any(|m| matches!(m.role, Role::Assistant) && !m.tool_calls.is_empty());
        assert!(
            retained_tool_call,
            "assistant tool-call turn was drained, orphaning its tool result — back-off did not fire"
        );
    }

    /// A tool-call assistant turn carries its signal in `tool_calls[].arguments_json`,
    /// not `content` (content is empty by construction). A history built entirely
    /// from such turns must still trigger summarization once the argument JSON
    /// alone pushes it over the effective window — the content-only scan this
    /// guards against would see nothing but empty strings and never trigger.
    #[tokio::test]
    async fn history_summarization_counts_tool_call_arguments() {
        let engine = Arc::new(MockEngine::with_context_window(
            vec![vec![
                StreamingChunk::Token {
                    text: "Summary of earlier tool activity.".to_string(),
                },
                StreamingChunk::Done {
                    usage: InferenceUsage {
                        prompt_tokens: 10,
                        completion_tokens: 5,
                    },
                },
            ]],
            4096, // effective window small enough that arg JSON alone overflows it
        ));
        let executor = Arc::new(MockToolExecutor::new());
        let agent_loop = LocalAgentLoop::new(engine, executor);

        let mut session = new_session();

        // Every assistant turn has EMPTY content — all signal lives in
        // arguments_json — plus a paired empty-content tool result. ~4 chars/token
        // in the mock: 12 calls * 2000 chars of args = 24000 chars ≈ 6000 tokens,
        // which alone exceeds the 4096-token effective window even though every
        // `content` field in the history is empty.
        for i in 0..12 {
            session
                .messages
                .push(ChatMessage::assistant_with_tool_calls(
                    String::new(),
                    vec![ToolCallRaw {
                        id: format!("tc_{i}"),
                        function_name: "create_nodes_from_markdown".into(),
                        arguments_json: format!(r#"{{"markdown":"{}"}}"#, "x".repeat(2000)),
                        provider_extra: None,
                    }],
                ));
            session.messages.push(ChatMessage::tool_result(
                String::new(),
                format!("tc_{i}"),
                "create_nodes_from_markdown".to_string(),
            ));
        }
        let messages_before = session.messages.len();

        agent_loop
            .maybe_summarize_history(&mut session, "system")
            .await
            .unwrap();

        assert!(
            session.messages.len() < messages_before,
            "tool-call argument JSON must count toward the history budget. Before: {}, After: {}",
            messages_before,
            session.messages.len()
        );
        assert!(
            session.messages[0]
                .content
                .contains("[Conversation summary]"),
            "First message should be the summary, got: {}",
            session.messages[0].content
        );
    }

    /// PREFILL_STABILITY_CEILING must account for the system prompt, not
    /// just history — this is the exact regression a history-only
    /// comparison would miss: history alone sits under the ceiling, but
    /// history + a realistic-size system prompt (production's tool-
    /// registered prompt runs ~6,600 tokens) pushes the total over it.
    #[tokio::test]
    async fn stability_ceiling_counts_the_system_prompt_not_just_history() {
        let engine = Arc::new(MockEngine::with_context_window(
            vec![vec![
                StreamingChunk::Token {
                    text: "Summary.".to_string(),
                },
                StreamingChunk::Done {
                    usage: InferenceUsage {
                        prompt_tokens: 10,
                        completion_tokens: 5,
                    },
                },
            ]],
            // Spacious window: gate 1 alone would not trigger at this size.
            32_768,
        ));
        let executor = Arc::new(MockToolExecutor::new());
        let agent_loop = LocalAgentLoop::new(engine, executor);

        let mut session = new_session();

        // ~5.5K history tokens (@ ~4 chars/token: 20 * 1100 = 22,000 chars) —
        // under PREFILL_STABILITY_CEILING (8,000) on its own.
        for i in 0..20 {
            let role = if i % 2 == 0 {
                Role::User
            } else {
                Role::Assistant
            };
            session.messages.push(ChatMessage::text(
                role,
                format!("Msg {}: {}", i, "w".repeat(1100)),
            ));
        }
        let messages_before = session.messages.len();

        // ~6,600-token system prompt (26,400 chars), matching production's
        // measured tool-registered prompt size. history (~5.5K) alone is
        // under the ceiling; history + this system prompt (~12K) is not.
        let realistic_system_prompt = "s".repeat(26_400);

        agent_loop
            .maybe_summarize_history(&mut session, &realistic_system_prompt)
            .await
            .unwrap();

        assert!(
            session.messages.len() < messages_before,
            "history + system prompt over PREFILL_STABILITY_CEILING must trigger \
             summarization even though history alone is under it. Before: {}, After: {}",
            messages_before,
            session.messages.len()
        );
    }

    // -- Additional coverage tests ------------------------------------------

    /// Mock engine that always fails on generate.
    struct FailingEngine;

    #[async_trait]
    impl ChatInferenceEngine for FailingEngine {
        async fn generate(
            &self,
            _request: InferenceRequest,
            _on_chunk: Box<dyn Fn(StreamingChunk) + Send>,
        ) -> Result<InferenceUsage, InferenceError> {
            Err(InferenceError::Engine("model crashed".into()))
        }

        async fn model_info(&self) -> Result<Option<ChatModelSpec>, InferenceError> {
            Ok(None)
        }

        async fn token_count(&self, text: &str) -> Result<u32, InferenceError> {
            Ok((text.len() as f32 / 4.0).ceil() as u32)
        }
    }

    /// When `run_turn` returns an error the session must still be in the
    /// sessions map (the "take-mutate-return" pattern reinserts on error).
    #[tokio::test]
    async fn session_persistence_after_inference_error() {
        let engine = Arc::new(FailingEngine);
        let executor = Arc::new(MockToolExecutor::new());
        let service = LocalAgentService::new(engine, executor);

        let id = service
            .create_session(Some("test-model".into()), vec![])
            .await;

        // send_message should fail because FailingEngine errors
        let user_msg = "Trigger an inference error and confirm the GitHub session survives intact";
        let result = service.send_message(&id, user_msg, |_| {}, |_| {}).await;

        assert!(result.is_err(), "Expected inference error");

        // Session must still exist in the map despite the error
        let session = service.get_session(&id).await;
        assert!(
            session.is_some(),
            "Session should persist after inference error"
        );

        // The user message should have been appended before the error
        let session = session.unwrap();
        assert!(
            !session.messages.is_empty(),
            "User message should be in session history"
        );
        assert_eq!(session.messages[0].role, Role::User);
        assert_eq!(session.messages[0].content, user_msg);
    }

    /// When every turn produces tool calls the loop must stop after exactly
    /// MAX_TOOL_ITERATIONS and return without spinning forever. After
    /// reaching the limit, one final tool-less inference is run.
    #[tokio::test]
    async fn max_iteration_limit_enforced_exactly() {
        let call_count = Arc::new(AtomicUsize::new(0));

        // Build more rounds than MAX_TOOL_ITERATIONS of tool-call responses with
        // DISTINCT queries so the duplicate-call guard does NOT fire — this test
        // exercises the pure iteration-cap path.
        // Plus a final text response for the tool-less wrap-up call.
        let mut responses: Vec<Vec<StreamingChunk>> = Vec::new();
        for i in 0..8 {
            responses.push(vec![
                StreamingChunk::ToolCallStart {
                    id: format!("tc_{i}"),
                    name: "search_nodes".to_string(),
                    provider_extra: None,
                },
                StreamingChunk::ToolCallArgs {
                    id: format!("tc_{i}"),
                    args_json: format!(r#"{{"query":"loop-{i}"}}"#),
                },
                StreamingChunk::Done {
                    usage: InferenceUsage {
                        prompt_tokens: 10,
                        completion_tokens: 5,
                    },
                },
            ]);
        }

        /// Mock engine that counts how many times generate is called.
        struct CountingEngine {
            inner: MockEngine,
            count: Arc<AtomicUsize>,
        }

        #[async_trait]
        impl ChatInferenceEngine for CountingEngine {
            async fn generate(
                &self,
                request: InferenceRequest,
                on_chunk: Box<dyn Fn(StreamingChunk) + Send>,
            ) -> Result<InferenceUsage, InferenceError> {
                self.count.fetch_add(1, Ordering::SeqCst);
                self.inner.generate(request, on_chunk).await
            }

            async fn model_info(&self) -> Result<Option<ChatModelSpec>, InferenceError> {
                self.inner.model_info().await
            }

            async fn token_count(&self, text: &str) -> Result<u32, InferenceError> {
                self.inner.token_count(text).await
            }
        }

        let engine = Arc::new(CountingEngine {
            inner: MockEngine::new(responses),
            count: Arc::clone(&call_count),
        });
        let executor = Arc::new(MockToolExecutor::new());
        let agent_loop = LocalAgentLoop::new(engine, executor);

        let mut session = new_session();
        let result = agent_loop
            .run_turn(
                &mut session,
                "Keep calling search_nodes past the iteration cap to verify enforcement",
                |_| {},
                |_| {},
                CancellationToken::new(),
            )
            .await
            .unwrap();

        // Tool calls made = MAX_TOOL_ITERATIONS (one per iteration)
        assert_eq!(result.tool_calls_made.len(), MAX_TOOL_ITERATIONS);

        // Engine called MAX_TOOL_ITERATIONS times + 1 final tool-less call
        assert_eq!(
            call_count.load(Ordering::SeqCst),
            MAX_TOOL_ITERATIONS + 1,
            "generate should be called MAX_TOOL_ITERATIONS + 1 (final tool-less) times"
        );

        // Usage summed from all rounds (including final tool-less call)
        let total_rounds = MAX_TOOL_ITERATIONS + 1;
        assert_eq!(result.usage.prompt_tokens, 10 * total_rounds as u32);
        assert_eq!(result.usage.completion_tokens, 5 * total_rounds as u32);
    }

    /// Cancellation during tool execution should stop the loop promptly.
    #[tokio::test]
    async fn cancellation_during_tool_execution() {
        // Engine returns a tool call in the first round
        let engine = Arc::new(MockEngine::new(vec![
            // Round 1: tool call
            vec![
                StreamingChunk::ToolCallStart {
                    id: "tc_1".to_string(),
                    name: "search_nodes".to_string(),
                    provider_extra: None,
                },
                StreamingChunk::ToolCallArgs {
                    id: "tc_1".to_string(),
                    args_json: r#"{"query":"test"}"#.to_string(),
                },
                // Also request a second tool call in the same round
                StreamingChunk::ToolCallStart {
                    id: "tc_2".to_string(),
                    name: "get_node".to_string(),
                    provider_extra: None,
                },
                StreamingChunk::ToolCallArgs {
                    id: "tc_2".to_string(),
                    args_json: r#"{"id":"abc123"}"#.to_string(),
                },
                StreamingChunk::Done {
                    usage: InferenceUsage {
                        prompt_tokens: 20,
                        completion_tokens: 10,
                    },
                },
            ],
        ]));

        /// Executor that cancels the token after executing the first tool.
        struct CancellingExecutor {
            inner: MockToolExecutor,
            cancel: CancellationToken,
            call_count: AtomicUsize,
        }

        #[async_trait]
        impl AgentToolExecutor for CancellingExecutor {
            async fn available_tools(&self) -> Result<Vec<ToolDefinition>, ToolError> {
                self.inner.available_tools().await
            }

            async fn execute(
                &self,
                name: &str,
                args: serde_json::Value,
            ) -> Result<ToolResult, ToolError> {
                let count = self.call_count.fetch_add(1, Ordering::SeqCst);
                let result = self.inner.execute(name, args).await;
                // Cancel after the first tool execution
                if count == 0 {
                    self.cancel.cancel();
                }
                result
            }
        }

        let cancel = CancellationToken::new();
        let executor = Arc::new(CancellingExecutor {
            inner: MockToolExecutor::new(),
            cancel: cancel.clone(),
            call_count: AtomicUsize::new(0),
        });
        let agent_loop = LocalAgentLoop::new(engine, executor);

        let mut session = new_session();
        let result = agent_loop
            .run_turn(
                &mut session,
                "Search the Billing Architecture documents and cancel mid-tool-execution",
                |_| {},
                |_| {},
                cancel,
            )
            .await;

        // Should have been cancelled
        assert!(result.is_err(), "Expected cancellation error");
        match result.unwrap_err() {
            InferenceError::Engine(msg) => assert_eq!(msg, "cancelled"),
            other => panic!("Expected Engine(cancelled), got {:?}", other),
        }
    }

    /// After summarization, the history token count should be below the budget.
    #[tokio::test]
    async fn history_summarization_reduces_token_count() {
        let engine = Arc::new(MockEngine::new(vec![
            // Summarization call — return a short summary
            vec![
                StreamingChunk::Token {
                    text: "User asked about billing.".to_string(),
                },
                StreamingChunk::Done {
                    usage: InferenceUsage {
                        prompt_tokens: 50,
                        completion_tokens: 10,
                    },
                },
            ],
            // Actual response
            vec![
                StreamingChunk::Token {
                    text: "Here you go.".to_string(),
                },
                StreamingChunk::Done {
                    usage: InferenceUsage {
                        prompt_tokens: 30,
                        completion_tokens: 5,
                    },
                },
            ],
        ]));
        let executor = Arc::new(MockToolExecutor::new());
        let agent_loop = LocalAgentLoop::new(Arc::clone(&engine), executor);

        let mut session = new_session();

        // Fill history with enough content to exceed the history budget.
        // ~4 chars/token, budget is 6000 tokens => need > 24000 chars.
        for i in 0..30 {
            let role = if i % 2 == 0 {
                Role::User
            } else {
                Role::Assistant
            };
            session.messages.push(ChatMessage::text(
                role,
                format!("Msg {}: {}", i, "a".repeat(2000)),
            ));
        }

        let _result = agent_loop
            .run_turn(
                &mut session,
                "Summarize the long Billing history to reduce tokens below the budget",
                |_| {},
                |_| {},
                CancellationToken::new(),
            )
            .await
            .unwrap();

        // Calculate token count of post-summarization history
        let mut total_text = String::new();
        for msg in &session.messages {
            total_text.push_str(&msg.content);
            total_text.push(' ');
        }
        let token_count = engine.token_count(&total_text).await.unwrap();
        let history_budget = TOTAL_TOKEN_BUDGET - SYSTEM_PROMPT_BUDGET;

        assert!(
            token_count <= history_budget,
            "After summarization, history tokens ({}) should be at or below budget ({})",
            token_count,
            history_budget
        );
    }

    /// Tool calls with empty or invalid JSON args should be handled gracefully
    /// (defaulting to `{}` rather than panicking).
    #[tokio::test]
    async fn empty_tool_call_args_handled_gracefully() {
        // Engine returns a tool call with empty args, then a final text response
        let engine = Arc::new(MockEngine::new(vec![
            // Round 1: tool call with empty args string
            vec![
                StreamingChunk::ToolCallStart {
                    id: "tc_1".to_string(),
                    name: "search_nodes".to_string(),
                    provider_extra: None,
                },
                // No ToolCallArgs chunks at all — args_json will be ""
                StreamingChunk::Done {
                    usage: InferenceUsage {
                        prompt_tokens: 10,
                        completion_tokens: 5,
                    },
                },
            ],
            // Round 2: final text
            vec![
                StreamingChunk::Token {
                    text: "Done with empty args.".to_string(),
                },
                StreamingChunk::Done {
                    usage: InferenceUsage {
                        prompt_tokens: 20,
                        completion_tokens: 10,
                    },
                },
            ],
        ]));
        let executor = Arc::new(MockToolExecutor::new());
        let agent_loop = LocalAgentLoop::new(engine, executor);

        let mut session = new_session();
        let result = agent_loop
            .run_turn(
                &mut session,
                "Invoke a GitHub tool with empty args and verify the loop does not panic",
                |_| {},
                |_| {},
                CancellationToken::new(),
            )
            .await;

        // Should not panic — empty args_json falls back to json!({})
        assert!(
            result.is_ok(),
            "Empty tool call args should not cause panic"
        );
        let result = result.unwrap();
        assert_eq!(result.response, "Done with empty args.");
        assert_eq!(result.tool_calls_made.len(), 1);
        // Args should have been defaulted to empty object
        assert_eq!(result.tool_calls_made[0].args, json!({}));
    }

    /// Two sessions can exist and operate independently without interference.
    #[tokio::test]
    async fn multiple_concurrent_sessions() {
        // Engine produces different responses based on call order:
        // calls 0,1 are for session A and session B respectively.
        let engine = Arc::new(MockEngine::new(vec![
            // Session A's response
            vec![
                StreamingChunk::Token {
                    text: "Response for session A".to_string(),
                },
                StreamingChunk::Done {
                    usage: InferenceUsage {
                        prompt_tokens: 10,
                        completion_tokens: 5,
                    },
                },
            ],
            // Session B's response
            vec![
                StreamingChunk::Token {
                    text: "Response for session B".to_string(),
                },
                StreamingChunk::Done {
                    usage: InferenceUsage {
                        prompt_tokens: 15,
                        completion_tokens: 8,
                    },
                },
            ],
        ]));
        let executor = Arc::new(MockToolExecutor::new());
        let service = LocalAgentService::new(engine, executor);

        // Create two independent sessions
        let id_a = service.create_session(Some("model-a".into()), vec![]).await;
        let id_b = service.create_session(Some("model-b".into()), vec![]).await;

        assert_ne!(id_a, id_b, "Session IDs should be unique");

        // Send a message to session A
        let result_a = service
            .send_message(&id_a, "Hello from A", |_| {}, |_| {})
            .await
            .unwrap();

        // Send a message to session B
        let result_b = service
            .send_message(&id_b, "Hello from B", |_| {}, |_| {})
            .await
            .unwrap();

        // Verify responses are independent
        assert_eq!(result_a.response, "Response for session A");
        assert_eq!(result_b.response, "Response for session B");

        // Verify each session has its own history
        let session_a = service.get_session(&id_a).await.unwrap();
        let session_b = service.get_session(&id_b).await.unwrap();

        assert_eq!(session_a.messages.len(), 2); // user + assistant
        assert_eq!(session_b.messages.len(), 2); // user + assistant

        assert_eq!(session_a.messages[0].content, "Hello from A");
        assert_eq!(session_b.messages[0].content, "Hello from B");

        assert_eq!(session_a.model_id, Some("model-a".to_string()));
        assert_eq!(session_b.model_id, Some("model-b".to_string()));

        // Ending session A should not affect session B
        service.end_session(&id_a).await;
        assert!(service.get_session(&id_a).await.is_none());
        assert!(service.get_session(&id_b).await.is_some());

        // Session B should still be functional
        let sessions = service.get_sessions().await;
        assert_eq!(sessions.len(), 1);
    }

    // -- multi-round tool dispatch ---------------------------------------

    /// A read followed by a write in one turn dispatches both correctly and
    /// preserves their order. Uses `search_nodes` as the leading read; the
    /// property under test is the loop's round-to-round dispatch, independent
    /// of which tool leads.
    #[tokio::test]
    async fn model_chains_a_read_then_a_write_in_one_turn() {
        // Round 1: model calls search_nodes
        // Round 2: model invokes create_node after seeing the result
        // Round 3: model produces a text summary
        let engine = Arc::new(MockEngine::new(vec![
            vec![
                StreamingChunk::ToolCallStart {
                    id: "tc_1".to_string(),
                    name: "search_nodes".to_string(),
                    provider_extra: None,
                },
                StreamingChunk::ToolCallArgs {
                    id: "tc_1".to_string(),
                    args_json: r#"{"query":"create a new task"}"#.to_string(),
                },
                StreamingChunk::Done {
                    usage: InferenceUsage {
                        prompt_tokens: 10,
                        completion_tokens: 5,
                    },
                },
            ],
            vec![
                StreamingChunk::ToolCallStart {
                    id: "tc_2".to_string(),
                    name: "create_node".to_string(),
                    provider_extra: None,
                },
                StreamingChunk::ToolCallArgs {
                    id: "tc_2".to_string(),
                    args_json: r#"{"content":"new task","node_type":"task"}"#.to_string(),
                },
                StreamingChunk::Done {
                    usage: InferenceUsage {
                        prompt_tokens: 20,
                        completion_tokens: 8,
                    },
                },
            ],
            vec![
                StreamingChunk::Token {
                    text: "Created the task.".to_string(),
                },
                StreamingChunk::Done {
                    usage: InferenceUsage {
                        prompt_tokens: 30,
                        completion_tokens: 5,
                    },
                },
            ],
        ]));

        let executor = MockToolExecutor::new()
            .with_tool(
                "search_nodes",
                json!({"type": "object"}),
                json!({
                    "query": "create a new task",
                    "matches": [
                        {"id": "skill-1", "name": "Node Creation", "confidence": 0.91,
                         "description": "Create new nodes", "tools": ["create_node"]}
                    ]
                }),
            )
            .with_tool(
                "create_node",
                json!({"type": "object"}),
                json!({"id": "nodespace://task-1"}),
            );

        let agent_loop = LocalAgentLoop::new(engine, Arc::new(executor));
        let mut session = new_session();
        let result = agent_loop
            .run_turn(
                &mut session,
                "create a new task to review the GitHub release notes",
                |_| {},
                |_| {},
                CancellationToken::new(),
            )
            .await
            .unwrap();

        assert_eq!(result.response, "Created the task.");
        assert_eq!(result.tool_calls_made.len(), 2);
        assert_eq!(result.tool_calls_made[0].name, "search_nodes");
        assert_eq!(result.tool_calls_made[1].name, "create_node");
    }

    /// Regression coverage: `update_node` must remain reachable through a
    /// find-then-act chain (`search_nodes` then `update_node`) now that
    /// `TOOL_STRATEGY_RULES` no longer hardcodes "ALWAYS search_nodes first
    /// before update_node" in resident prose.
    ///
    /// This scripts the full chain a real model is expected to follow and
    /// asserts the loop dispatches every step correctly against a scripted
    /// `MockEngine` — it proves the DISPATCH PLUMBING supports the routed
    /// path (tool execution order, argument passing, session state), not
    /// that a real model will choose this sequence. `MockEngine` returns a
    /// fixed queue of responses regardless of prompt content, so it cannot
    /// exercise routing choice. Live-model adherence to this routing is a
    /// separate, not-yet-closed concern for the agent-matrix eval
    /// (`scripts/eval/fixtures/agent-matrix.ts`), which is not run as part
    /// of `test:all`.
    #[tokio::test]
    async fn find_then_act_chain_dispatches_correctly() {
        let engine = Arc::new(MockEngine::new(vec![
            vec![
                StreamingChunk::ToolCallStart {
                    id: "tc_1".to_string(),
                    name: "search_nodes".to_string(),
                    provider_extra: None,
                },
                StreamingChunk::ToolCallArgs {
                    id: "tc_1".to_string(),
                    args_json: r#"{"query":"mark an invoice as paid"}"#.to_string(),
                },
                StreamingChunk::Done {
                    usage: InferenceUsage {
                        prompt_tokens: 10,
                        completion_tokens: 5,
                    },
                },
            ],
            vec![
                StreamingChunk::ToolCallStart {
                    id: "tc_2".to_string(),
                    name: "search_nodes".to_string(),
                    provider_extra: None,
                },
                StreamingChunk::ToolCallArgs {
                    id: "tc_2".to_string(),
                    args_json: r#"{"query":"","node_type":"invoice","filters":[{"type":"property","operator":"equals","property":"amount","value":500}]}"#.to_string(),
                },
                StreamingChunk::Done {
                    usage: InferenceUsage {
                        prompt_tokens: 20,
                        completion_tokens: 8,
                    },
                },
            ],
            vec![
                StreamingChunk::ToolCallStart {
                    id: "tc_3".to_string(),
                    name: "update_node".to_string(),
                    provider_extra: None,
                },
                StreamingChunk::ToolCallArgs {
                    id: "tc_3".to_string(),
                    args_json: r#"{"id":"nodespace://invoice-1","field_values":{"status":"paid"}}"#.to_string(),
                },
                StreamingChunk::Done {
                    usage: InferenceUsage {
                        prompt_tokens: 30,
                        completion_tokens: 8,
                    },
                },
            ],
            vec![
                StreamingChunk::Token {
                    text: "Marked the invoice as paid.".to_string(),
                },
                StreamingChunk::Done {
                    usage: InferenceUsage {
                        prompt_tokens: 40,
                        completion_tokens: 6,
                    },
                },
            ],
        ]));

        let executor = MockToolExecutor::new()
            .with_tool(
                "search_nodes",
                json!({"type": "object"}),
                json!({
                    "query": "mark an invoice as paid",
                    "matches": [
                        {"id": "skill-graph-editing", "name": "Graph Editing", "confidence": 0.9,
                         "description": "Modify existing nodes in the knowledge graph",
                         "tools": ["update_node", "search_nodes", "resolve_query"]}
                    ]
                }),
            )
            .with_tool(
                "search_nodes",
                json!({"type": "object"}),
                json!({"results": [{"id": "nodespace://invoice-1", "title": "Invoice #001", "properties": {"amount": 500, "status": "open"}}]}),
            )
            .with_tool(
                "update_node",
                json!({"type": "object"}),
                json!({"id": "nodespace://invoice-1", "properties": {"status": "paid"}}),
            );

        let agent_loop = LocalAgentLoop::new(engine, Arc::new(executor));
        let mut session = new_session();
        let result = agent_loop
            .run_turn(
                &mut session,
                "Mark the $500 invoice as paid",
                |_| {},
                |_| {},
                CancellationToken::new(),
            )
            .await
            .unwrap();

        assert_eq!(result.response, "Marked the invoice as paid.");
        assert_eq!(result.tool_calls_made.len(), 3);
        assert_eq!(result.tool_calls_made[0].name, "search_nodes");
        assert_eq!(result.tool_calls_made[1].name, "search_nodes");
        assert_eq!(result.tool_calls_made[2].name, "update_node");
    }

    /// When the model decides no skill is needed, it responds directly —
    /// no clarification short-circuit, no canned string.
    #[tokio::test]
    async fn model_can_respond_without_calling_any_tool() {
        let engine = Arc::new(MockEngine::single_text("Hi there — how can I help?"));
        let executor = Arc::new(MockToolExecutor::new());
        let agent_loop = LocalAgentLoop::new(engine, executor);

        let mut session = new_session();
        let result = agent_loop
            .run_turn(&mut session, "hi", |_| {}, |_| {}, CancellationToken::new())
            .await
            .unwrap();

        assert_eq!(result.response, "Hi there — how can I help?");
        assert!(result.tool_calls_made.is_empty());
        // Crucially: the model was actually invoked (no pre-LLM short-circuit).
        assert!(result.usage.prompt_tokens > 0);
    }

    /// Multi-skill turn: the model calls a read tool
    /// for each sub-task, then invokes the matched skill's tool. This test
    /// exercises a full chain — search_nodes (notes) → search_semantic →
    /// search_nodes (task) → create_node — not just two back-to-back
    /// searches, so a regression that breaks tool dispatch after a second
    /// tool call is caught here.
    #[tokio::test]
    async fn multi_skill_turn_invokes_skill_tools_between_searches() {
        let engine = Arc::new(MockEngine::new(vec![
            // Round 1: search_nodes for "find notes"
            vec![
                StreamingChunk::ToolCallStart {
                    id: "tc_1".to_string(),
                    name: "search_nodes".to_string(),
                    provider_extra: None,
                },
                StreamingChunk::ToolCallArgs {
                    id: "tc_1".to_string(),
                    args_json: r#"{"query":"find notes"}"#.to_string(),
                },
                StreamingChunk::Done {
                    usage: InferenceUsage {
                        prompt_tokens: 10,
                        completion_tokens: 5,
                    },
                },
            ],
            // Round 2: invoke search_semantic (the matched skill's tool)
            vec![
                StreamingChunk::ToolCallStart {
                    id: "tc_2".to_string(),
                    name: "search_semantic".to_string(),
                    provider_extra: None,
                },
                StreamingChunk::ToolCallArgs {
                    id: "tc_2".to_string(),
                    args_json: r#"{"query":"Q2 budget notes"}"#.to_string(),
                },
                StreamingChunk::Done {
                    usage: InferenceUsage {
                        prompt_tokens: 15,
                        completion_tokens: 5,
                    },
                },
            ],
            // Round 3: search_nodes for "create task"
            vec![
                StreamingChunk::ToolCallStart {
                    id: "tc_3".to_string(),
                    name: "search_nodes".to_string(),
                    provider_extra: None,
                },
                StreamingChunk::ToolCallArgs {
                    id: "tc_3".to_string(),
                    args_json: r#"{"query":"create task"}"#.to_string(),
                },
                StreamingChunk::Done {
                    usage: InferenceUsage {
                        prompt_tokens: 18,
                        completion_tokens: 5,
                    },
                },
            ],
            // Round 4: invoke create_node (the matched skill's tool)
            vec![
                StreamingChunk::ToolCallStart {
                    id: "tc_4".to_string(),
                    name: "create_node".to_string(),
                    provider_extra: None,
                },
                StreamingChunk::ToolCallArgs {
                    id: "tc_4".to_string(),
                    args_json: r#"{"content":"Review Q2 notes","node_type":"task"}"#.to_string(),
                },
                StreamingChunk::Done {
                    usage: InferenceUsage {
                        prompt_tokens: 22,
                        completion_tokens: 6,
                    },
                },
            ],
            // Round 5: final summary
            vec![
                StreamingChunk::Token {
                    text: "Found the notes and created the review task.".to_string(),
                },
                StreamingChunk::Done {
                    usage: InferenceUsage {
                        prompt_tokens: 25,
                        completion_tokens: 9,
                    },
                },
            ],
        ]));

        let executor = MockToolExecutor::new()
            .with_tool(
                "search_nodes",
                json!({"type": "object"}),
                // Same canned response works for both calls; in production
                // the embeddings would distinguish them, but the agent loop
                // doesn't introspect the match payload — it just relays it
                // back to the model.
                json!({
                    "query": "x",
                    "matches": [
                        {"id": "skill-1", "name": "Match", "confidence": 0.9,
                         "description": "matched skill", "tools": ["search_semantic"]}
                    ]
                }),
            )
            .with_tool(
                "search_semantic",
                json!({"type": "object"}),
                json!({"count": 1, "results": [
                    {"id": "note-1", "title": "Q2 Budget", "score": 0.87,
                     "snippet": "Quarterly budget summary…"}
                ]}),
            )
            .with_tool(
                "create_node",
                json!({"type": "object"}),
                json!({"id": "nodespace://task-1"}),
            );

        let agent_loop = LocalAgentLoop::new(engine, Arc::new(executor));
        let mut session = new_session();
        // Loosen the iteration cap since this turn legitimately needs 4 tool
        // rounds (2 searches + 2 invocations) plus a text round. MAX_TOOL_ITERATIONS
        // is 5, so we're at the boundary on purpose.
        let result = agent_loop
            .run_turn(
                &mut session,
                "find my Q2 budget notes and create a task to review them",
                |_| {},
                |_| {},
                CancellationToken::new(),
            )
            .await
            .unwrap();

        assert_eq!(
            result.tool_calls_made.len(),
            4,
            "{:?}",
            result.tool_calls_made
        );
        assert_eq!(result.tool_calls_made[0].name, "search_nodes");
        assert_eq!(result.tool_calls_made[1].name, "search_semantic");
        assert_eq!(result.tool_calls_made[2].name, "search_nodes");
        assert_eq!(result.tool_calls_made[3].name, "create_node");
        assert_eq!(
            result.response,
            "Found the notes and created the review task."
        );
    }

    /// An empty tool result → model judges and produces a contextual
    /// clarification (referencing what it searched), rather than the prior
    /// hardcoded `CLARIFYING_QUESTION` string. This is the "no relevant skill"
    /// path.
    #[tokio::test]
    async fn empty_tool_result_lets_the_model_clarify_with_context() {
        let engine = Arc::new(MockEngine::new(vec![
            // Round 1: model calls a read tool
            vec![
                StreamingChunk::ToolCallStart {
                    id: "tc_1".to_string(),
                    name: "search_nodes".to_string(),
                    provider_extra: None,
                },
                StreamingChunk::ToolCallArgs {
                    id: "tc_1".to_string(),
                    args_json: r#"{"query":"send carrier pigeons"}"#.to_string(),
                },
                StreamingChunk::Done {
                    usage: InferenceUsage {
                        prompt_tokens: 10,
                        completion_tokens: 5,
                    },
                },
            ],
            // Round 2: model produces a contextual clarification referencing
            // the search it just performed. The exact wording isn't checked —
            // only that the model gets to respond after seeing matches=[].
            vec![
                StreamingChunk::Token {
                    text: "I searched for skills related to that but didn't find anything relevant. Could you describe what you'd like to do?".to_string(),
                },
                StreamingChunk::Done {
                    usage: InferenceUsage {
                        prompt_tokens: 20,
                        completion_tokens: 18,
                    },
                },
            ],
        ]));

        let executor = MockToolExecutor::new().with_tool(
            "search_nodes",
            json!({"type": "object"}),
            // Empty matches array — the meaningful "no skill applies" signal.
            json!({"query": "send carrier pigeons", "matches": []}),
        );

        let agent_loop = LocalAgentLoop::new(engine, Arc::new(executor));
        let mut session = new_session();
        let result = agent_loop
            .run_turn(
                &mut session,
                "can you send carrier pigeons",
                |_| {},
                |_| {},
                CancellationToken::new(),
            )
            .await
            .unwrap();

        assert_eq!(result.tool_calls_made.len(), 1);
        assert_eq!(result.tool_calls_made[0].name, "search_nodes");
        // Crucially: the response is the model's contextual text, not a
        // canned constant. Just check it's non-empty and acknowledges the
        // search — exact wording belongs to the model.
        assert!(!result.response.is_empty());
        assert!(
            result.response.to_lowercase().contains("search")
                || result.response.to_lowercase().contains("didn't find"),
            "model response should reference what it searched: {:?}",
            result.response
        );
    }

    // -- contains_action_claim tests -----------------------------------------

    #[test]
    fn action_claim_detects_creation_verbs() {
        assert!(contains_action_claim("I created a new node for this."));
        assert!(contains_action_claim(
            "I've created the invoice successfully."
        ));
        assert!(contains_action_claim("I updated the task status."));
        assert!(contains_action_claim("Successfully created the schema."));
        assert!(contains_action_claim("The node has been created."));
    }

    /// Regression for #1865: the "I have created" phrasing (as opposed to
    /// "I created" / "I've created") reached the user unguarded — observed
    /// verbatim in a fabricated venue-creation claim with zero tool calls.
    #[test]
    fn action_claim_detects_have_created_phrasing() {
        assert!(contains_action_claim(
            "I have created a new node for \"The Blue Note\" under your Venue Booking Tracker."
        ));
        assert!(contains_action_claim("I have updated the record."));
        assert!(contains_action_claim("I have added the contact email."));
    }

    #[test]
    fn action_claim_does_not_fire_on_capability_statements() {
        assert!(!contains_action_claim("I can help you create a node."));
        assert!(!contains_action_claim(
            "I would create a node if you'd like."
        ));
        assert!(!contains_action_claim(
            "I could search for that information."
        ));
        assert!(!contains_action_claim("Sure, I'll look that up for you."));
        assert!(!contains_action_claim("Hello! How can I help?"));
    }

    #[test]
    fn action_claim_does_not_fire_on_conversational_text() {
        assert!(!contains_action_claim("What would you like to do?"));
        assert!(!contains_action_claim("Let me search for that."));
        assert!(!contains_action_claim(
            "The billing architecture node describes a system."
        ));
    }

    // -- looks_like_narrated_tool_call tests ---------------------------------

    #[test]
    fn narrated_tool_call_detects_registered_tool_invocation() {
        // The exact shape reproduced with mistral:7b via Ollama.
        assert!(looks_like_narrated_tool_call(
            r#"search_nodes(node_type='task', filters=[{"property":"status"}])"#
        ));
        assert!(looks_like_narrated_tool_call("create_node(title='Foo')"));
        // Whitespace between name and paren still counts.
        assert!(looks_like_narrated_tool_call(
            "update_node ({\"id\": \"x\"})"
        ));
        // Embedded in surrounding prose.
        assert!(looks_like_narrated_tool_call(
            "Let me run search_nodes(query='invoice') to find it."
        ));
    }

    #[test]
    fn narrated_tool_call_detects_a_json_tool_call_emitted_as_text() {
        // A model emitting a well-formed tool call as *text* leaves the loop
        // with no tool call to execute, so the turn silently does nothing.
        assert!(looks_like_narrated_tool_call(
            r#"[{"name":"create_node","arguments":{"content":"Review billing docs"}}]"#
        ));
        assert!(looks_like_narrated_tool_call(
            r#"{"name": "search_nodes", "arguments": {"query": "billing"}}"#
        ));
    }

    /// A brace-delimited argument object behind a `call:` prefix — the shape
    /// that reached the user verbatim on the locked model. Parens were the only
    /// bracket the detector knew, so this was persisted as the assistant's
    /// answer while nothing executed.
    #[test]
    fn narrated_tool_call_detects_a_brace_delimited_call() {
        assert!(looks_like_narrated_tool_call(
            r#"call:create_schema{fields:[{"friendlyName":"Status","name":"status"}], name:"Feature Writeups"}"#
        ));
        assert!(looks_like_narrated_tool_call(
            "tool:update_node{\"id\":\"x\",\"field_values\":{}}"
        ));
        assert!(looks_like_narrated_tool_call(
            ">>>update_node{\"id\":\"x\"}"
        ));
    }

    /// A bare `create_node{…}` at the very start of a response, with no marker
    /// prefix at all, is deliberately NOT detected.
    ///
    /// It is not distinguishable from documentation —
    /// "update_node {id, field_values} takes two arguments." has the identical
    /// shape — and the two errors are not symmetric. Missing a narrated call
    /// shows the user pseudo-code, which is bad but visible and recoverable.
    /// A false positive DISCARDS a correct answer and replaces it with a
    /// confirmation request, which is a silent regression on a working turn.
    /// This detector's whole purpose is that a turn must not misreport what
    /// happened, so it errs toward letting text through.
    #[test]
    fn narrated_tool_call_declines_an_unmarked_call_at_text_start() {
        assert!(!looks_like_narrated_tool_call(
            r#"create_node{"content":"Review billing docs"}"#
        ));
    }

    /// The brace shape must not fire on prose that merely quotes an argument
    /// object. Without the call-prefix requirement, ordinary explanations of a
    /// tool's JSON would be suppressed and replaced with a summary.
    #[test]
    fn narrated_tool_call_ignores_a_brace_in_explanatory_prose() {
        assert!(!looks_like_narrated_tool_call(
            "Pass create_node {\"content\": \"...\"} style arguments when you want a node."
        ));
        assert!(!looks_like_narrated_tool_call(
            "The create_schema {fields} parameter is required."
        ));
    }

    /// The brace branch's prefix test decides whether a CORRECT answer survives:
    /// a positive verdict discards the model's text and substitutes a
    /// confirmation request. These are the shapes an earlier version of that
    /// test admitted — "call" as an ordinary English verb, and a response that
    /// merely begins with a tool name.
    #[test]
    fn narrated_tool_call_ignores_call_as_an_english_verb() {
        // `ends_with("call")` matched the verb in "I would call create_node {…}",
        // which is a model correctly explaining what it is about to do.
        assert!(!looks_like_narrated_tool_call(
            "To do this I would call create_node {\"content\":\"Buy milk\"} but I need the id first."
        ));
        assert!(!looks_like_narrated_tool_call(
            "Recall create_node {a} is one option."
        ));
    }

    #[test]
    fn narrated_tool_call_ignores_a_response_that_merely_starts_with_a_tool_name() {
        // `prefix.is_empty()` fired on any response beginning with the name, so
        // a sentence documenting a tool's arguments was suppressed outright.
        assert!(!looks_like_narrated_tool_call(
            "update_node {id, field_values} takes two arguments."
        ));
    }

    /// A response documenting the tools one per line puts a tool name straight
    /// after a newline, which once had a dedicated "own line" admission — an
    /// unqualified "preceded by \n" test caught the identical sentence the
    /// index-0 case deliberately lets through, and a positive verdict discards
    /// the whole reply, not just the matched line. That admission is gone now
    /// (see `narrated_tool_call_ignores_a_tool_name_starting_a_later_line`,
    /// which found it could not be made safe even with an adjacency
    /// requirement), so this pins the same property against the marker-only
    /// detector: no marker precedes any of these, so none should fire.
    #[test]
    fn narrated_tool_call_ignores_a_tool_documented_at_the_start_of_a_line() {
        assert!(!looks_like_narrated_tool_call(
            "Here are the tools:\nupdate_node {id, field_values} — updates a node.\n\
             create_node {content} — creates one."
        ));
        assert!(!looks_like_narrated_tool_call(
            "Two options:\n\ncreate_node {content}\n"
        ));
    }

    /// Found in re-review: the reasoning that rejects a tool name at the START
    /// of the text ("documentation, not a call") does not stop applying at
    /// line two. Worst real case: a model correctly reporting a completed
    /// write inside a fenced code block — exactly the honest-report turn this
    /// PR's sibling no-op-success fix exists to protect.
    #[test]
    fn narrated_tool_call_ignores_a_tool_name_starting_a_later_line() {
        assert!(!looks_like_narrated_tool_call(
            "I created the task. For reference, the payload was:\n```json\ncreate_node {\"content\":\"Buy milk\"}\n```\nAnything else?"
        ));
        assert!(!looks_like_narrated_tool_call(
            "The signature is:\n    create_node {content, node_type}\nand it returns the new id."
        ));
        // The UNSPACED form — the shape that actually matters. The adjacency
        // requirement (name immediately followed by `{`) was meant to
        // distinguish a narrated call from documentation, on the reasoning
        // that documentation puts a space there. It does not distinguish a
        // narrated call from a model's own honest report of a completed
        // write inside a fenced JSON block, which is ordinary, idiomatic
        // formatting with no space before the brace. This is real output a
        // model can produce at any time, and the two prior assertions above
        // (both spaced) never exercised it — found in a post-merge audit.
        assert!(!looks_like_narrated_tool_call(
            "I created the task. For reference, the payload was:\n```json\ncreate_node{\"content\":\"Buy milk\"}\n```\nAnything else?"
        ));
        assert!(!looks_like_narrated_tool_call(
            "Tools:\ncreate_node{content}\nupdate_node{id}"
        ));
    }

    /// `ends_with("call:")` is a suffix test, and "recall:" ends with "call:".
    /// The marker has to stand as its own token or it re-admits the English
    /// verb the `ends_with("call")` removal was meant to exclude.
    #[test]
    fn narrated_tool_call_ignores_recall_as_a_sentence_opener() {
        assert!(!looks_like_narrated_tool_call(
            "Recall: create_node {content} is the one you want."
        ));
        assert!(!looks_like_narrated_tool_call(
            "Recall:create_node{\"content\":\"x\"} was the earlier suggestion."
        ));
        // The genuine marker, still detected, with and without a leading word.
        assert!(looks_like_narrated_tool_call(
            "I'll run this. call: create_node {\"content\":\"x\"}"
        ));
    }

    #[test]
    fn narrated_tool_call_ignores_a_tool_merely_mentioned_in_prose() {
        assert!(!looks_like_narrated_tool_call(
            "I used create_node to add that for you."
        ));
        assert!(!looks_like_narrated_tool_call(
            "The search_nodes results were empty."
        ));
    }

    #[test]
    fn narrated_tool_call_does_not_fire_on_normal_prose() {
        assert!(!looks_like_narrated_tool_call(
            "I can help you query your tasks. What status are you looking for?"
        ));
        // Tool name mentioned but not as a call (no following paren).
        assert!(!looks_like_narrated_tool_call(
            "You can use search_nodes to filter by property."
        ));
        // A generic function-call shape that is NOT a registered tool must not
        // trip the narrow detector.
        assert!(!looks_like_narrated_tool_call(
            "The helper foo(x) returns a value."
        ));
        assert!(!looks_like_narrated_tool_call(""));
    }

    // -- persisted_field_count / no-op-success tests --------------------------

    #[test]
    fn persisted_field_count_reads_both_write_signals() {
        // create_schema reports what it persisted as an array...
        assert_eq!(
            persisted_field_count("create_schema", &json!({"fields": ["title", "due_date"]})),
            Some(2)
        );
        // ...create_node / update_node as a bare count.
        assert_eq!(
            persisted_field_count("create_node", &json!({"property_count": 3})),
            Some(3)
        );
        // The zero that this issue's call returned alongside `updated: true`.
        assert_eq!(
            persisted_field_count("update_node", &json!({"property_count": 0})),
            Some(0)
        );
    }

    #[test]
    fn persisted_field_count_is_none_for_non_write_results() {
        // A read tool carries neither signal. `None`, not `Some(0)` — absence of
        // the signal must not be read as "persisted nothing", or every
        // search-only turn would trip the no-op guard.
        assert_eq!(
            persisted_field_count("search_nodes", &json!({"results": []})),
            None
        );
        assert_eq!(persisted_field_count("get_node", &json!({})), None);
    }

    #[test]
    fn persisted_field_count_ignores_a_schema_read_that_looks_like_a_write() {
        // `get_node` on a SCHEMA node returns that schema's own `fields` array,
        // which is shape-identical to create_schema's report of what it just
        // wrote. Keyed by tool name precisely so this cannot be mistaken for a
        // write: a schema defining zero fields would otherwise read as
        // "persisted nothing" and trip the no-op guard on a read-only turn.
        assert_eq!(
            persisted_field_count("get_node", &json!({"fields": [], "nodeType": "schema"})),
            None
        );
        assert_eq!(
            persisted_field_count(
                "get_node",
                &json!({"fields": [{"name": "amount"}], "nodeType": "schema"})
            ),
            None
        );
        // update_task_status is a genuine write, but reports neither signal —
        // it must not be counted as an empty write either.
        assert_eq!(
            persisted_field_count("update_task_status", &json!({"updated": true})),
            None
        );
    }

    #[test]
    fn persisted_field_count_ignores_writes_that_had_no_properties_to_persist() {
        // A plain `text` note has no schema fields, so create_node stores it
        // with zero properties and that is a COMPLETE success. Counting it as
        // an empty write would let the no-op guard suppress a truthful "I've
        // added that note" — turning a correct confirmation into a spurious
        // "please confirm" and making the guard a UX regression.
        assert_eq!(
            persisted_field_count(
                "create_node",
                &json!({"id": "nodespace://x", "property_count": 0, "content_only": true})
            ),
            None
        );
        // Same for a content-only update: content genuinely changed, and
        // `property_count` is 0 only because no properties were sent.
        assert_eq!(
            persisted_field_count(
                "update_node",
                &json!({"property_count": 0, "updated_content_only": true})
            ),
            None
        );
        // But a create that DID ask for properties and persisted none is still
        // reported — that is the dropped-particulars case the guard exists for.
        assert_eq!(
            persisted_field_count("create_node", &json!({"property_count": 0})),
            Some(0)
        );
    }

    #[test]
    fn passive_action_claim_still_fires_when_a_qualifier_appears_elsewhere_in_the_text() {
        // The under-fire direction, which the over-fire test below cannot see.
        // Every string here is a GENUINE fabricated-write claim that a
        // whole-text qualifier check silently waived: a trailing sign-off, a
        // second sentence mentioning the UI, or "not"/"n't" inside an ordinary
        // word ("cannot", "isn't") was enough to disable the guard entirely.
        //
        // This matters beyond the new no-op guard: contains_action_claim also
        // gates the pre-existing zero-tool-call anti-fabrication guard, so a
        // regression here silently narrows a guard that was already shipping.
        for text in [
            "The due date was set to 2026-08-06. I did not change anything else.",
            "The task was updated. I cannot see the old value.",
            "The due date was set to the 6th. It will be visible on the task page.",
            "The node was updated; there isn't anything else to change.",
            "The node was updated. Anything else?",
            "The task was updated. The page now shows the new date.",
            "The node was updated. There is no node named X.",
            "The due date was set to the 6th. It was never set before.",
            "The node was updated. That should be all.",
        ] {
            assert!(
                contains_action_claim(text),
                "a real claim must not be waived by a qualifier in a DIFFERENT clause: {text:?}"
            );
        }
    }

    #[test]
    fn passive_action_claim_does_not_fire_on_questions_negations_or_reports() {
        // Measured, not assumed: with the bare phrases matched unconditionally,
        // every string below returned `true`. Each would convert a perfectly
        // good response into a spurious "please confirm", so these qualifiers
        // are load-bearing.
        //
        // A negation is the OPPOSITE of an action claim.
        assert!(!contains_action_claim(
            "Nothing was updated because the value already matched."
        ));
        // A question asks about an action, it does not claim one.
        assert!(!contains_action_claim("Do you know when it was updated?"));
        // Reporting what storage holds is not claiming to have changed it.
        assert!(!contains_action_claim(
            "The record shows it was marked returned on the 12th."
        ));
        assert!(!contains_action_claim(
            "It shows when the item was set to return."
        ));
        // Hypotheticals describe an action not yet taken.
        assert!(!contains_action_claim(
            "The due date would be set to the 6th if you confirm."
        ));
    }

    #[test]
    fn update_node_content_only_note_is_not_itself_an_action_claim() {
        // exec_update_node returns this `note` verbatim to the model on a
        // content-only edit — i.e. on exactly the turn the no-op guard watches.
        // The guard reads only the model's own response, never a tool result,
        // so the note cannot trip it directly; but models routinely echo a
        // result's wording back in their final text, which would. An earlier
        // draft opened with "Content was updated." and did trip this. Keeping
        // the assertion means the note can never be reworded back into a phrase
        // the guard keys on without a test failing.
        assert!(!contains_action_claim(
            "This call changed only the node's text. It did not change any property value, \
             so the node's state (status, dates, and other properties) remains exactly as it was."
        ));
    }

    #[test]
    fn action_claim_detects_bare_passive_past_tense() {
        // The exact live generation that passed the guard untouched: the phrase
        // list had "has been updated" but no bare "was updated".
        assert!(contains_action_claim(
            "Fact: node nodespace://d7e3bb35-170a-4865-a6f6-063fbd1e0a09 was updated."
        ));
        assert!(contains_action_claim("The due date was set to 2026-08-06."));
        assert!(contains_action_claim("Three nodes were created."));
    }

    #[test]
    fn action_claim_still_ignores_capability_and_question_phrasing() {
        // Guard against the new passive phrases over-firing: these describe a
        // possible or requested action, not a performed one.
        assert!(!contains_action_claim(
            "I can update that node if you'd like."
        ));
        assert!(!contains_action_claim(
            "Would you like the due date set to the 6th?"
        ));
    }

    // -- summarize_executions tests ------------------------------------------

    fn exec_record(name: &str, is_error: bool) -> ToolExecutionRecord {
        ToolExecutionRecord {
            tool_call_id: format!("tc_{name}"),
            name: name.to_string(),
            args: json!({}),
            result: json!({}),
            is_error,
            duration_ms: 1,
        }
    }

    fn exec_record_with(
        name: &str,
        args: serde_json::Value,
        result: serde_json::Value,
    ) -> ToolExecutionRecord {
        ToolExecutionRecord {
            tool_call_id: format!("tc_{name}"),
            name: name.to_string(),
            args,
            result,
            is_error: false,
            duration_ms: 1,
        }
    }

    #[test]
    fn summarize_executions_marks_all_failed_calls_as_failed() {
        // The looping-search case: every call errored → "failed", never
        // an optimistic "completed".
        let summary = summarize_executions(&[
            exec_record("search_nodes", true),
            exec_record("search_nodes", true),
        ]);
        assert_eq!(summary, "• node search failed (2×)");
    }

    #[test]
    fn summarize_executions_marks_successful_calls_as_completed() {
        let summary = summarize_executions(&[exec_record("create_node", false)]);
        assert_eq!(summary, "• node creation completed");
    }

    #[test]
    fn summarize_executions_treats_a_recovered_retry_as_completed() {
        // Failed, then succeeded: the model reacted to the error and the turn
        // ended on the success. Reporting the transient would describe
        // something the user has no action to take on — the same narrowing the
        // tool-failure-surfacing guard applies.
        let summary = summarize_executions(&[
            exec_record("search_nodes", true),
            exec_record("search_nodes", false),
        ]);
        assert_eq!(summary, "• node search completed (2×)");
    }

    /// The mirror image of the retry case, and the one that hid a failed write:
    /// same tool, success FIRST and failure LAST. Grouping by name collapses the
    /// pair to one bullet, and a single verb chosen by "did any call succeed?"
    /// reported the whole group as completed — so `create_node` for record A
    /// succeeding masked `create_node` for record B failing, on exactly the
    /// empty-generation path this summarizer was newly wired into.
    #[test]
    fn summarize_executions_surfaces_a_failure_that_follows_a_success() {
        let summary = summarize_executions(&[
            exec_record("create_node", false),
            exec_record("create_node", true),
        ]);
        assert_eq!(summary, "• node creation completed (1×), failed (1×)");
        assert!(
            summary.contains("failed"),
            "a write that failed last must be visible, not collapsed into the \
             earlier success: {summary}"
        );
    }

    /// Order is what decides, not merely the presence of a failure: the same
    /// two verdicts with more calls on each side still report both counts.
    #[test]
    fn summarize_executions_reports_both_counts_when_a_group_ends_on_a_failure() {
        let summary = summarize_executions(&[
            exec_record("update_node", false),
            exec_record("update_node", false),
            exec_record("update_node", true),
        ]);
        assert_eq!(summary, "• node update completed (2×), failed (1×)");
    }

    /// A failed write reached the user as a success report because the
    /// empty-generation fallback formatted the LAST execution's name into
    /// "Done — {} completed successfully." without consulting `is_error`.
    ///
    /// Observed live against the locked model on a seeded chain: `create_schema`
    /// failed, so the `node_type` the following `create_node` named did not
    /// exist, and the turn rendered
    ///
    ///     [tool] create_node [ERROR]
    ///     assistant> Done — node creation completed successfully.
    ///
    /// None of the three response guards can catch this shape: anti-fabrication,
    /// no-op success, and tool-failure surfacing are each gated on
    /// `!normalized.is_empty()`, and this is precisely the empty-text path.
    ///
    /// Asserted through `summarize_executions` rather than the loop because
    /// that is the function the fallback now delegates to; the property that
    /// matters is that a sole errored execution never renders as "completed".
    #[test]
    fn sole_failed_write_is_never_summarized_as_success() {
        let summary = summarize_executions(&[exec_record("create_node", true)]);
        assert_eq!(summary, "• node creation failed");
        assert!(
            !summary.contains("completed"),
            "a write that only ever errored must not read as completed: {summary}"
        );
    }

    /// The same failure mid-chain: an earlier tool succeeded, so the naive
    /// "last execution" formatting would have reported the FAILED trailing call
    /// as the turn's success. Each tool keeps its own verdict.
    #[test]
    fn failed_trailing_write_keeps_its_own_verdict_after_a_successful_read() {
        let summary = summarize_executions(&[
            exec_record("search_nodes", false),
            exec_record("create_node", true),
        ]);
        assert_eq!(summary, "• node search completed\n• node creation failed");
    }

    // -- Anti-fabrication guard tests ----------------------------------------

    /// Model claims action with zero tool calls → response should be converted
    /// to a confirmation request.
    #[tokio::test]
    async fn anti_fabrication_guard_fires_on_ungrounded_action_claim() {
        let engine = Arc::new(MockEngine::single_text(
            "I created invoice ID 104 and marked it as paid.",
        ));
        let executor = Arc::new(MockToolExecutor::new());
        let agent_loop = LocalAgentLoop::new(engine, executor);

        let mut session = new_session();
        let result = agent_loop
            .run_turn(
                &mut session,
                "create an invoice for $500",
                |_| {},
                |_| {},
                CancellationToken::new(),
            )
            .await
            .unwrap();

        assert!(result.tool_calls_made.is_empty());
        // The fabricated claim should not reach the user verbatim.
        assert!(
            !result.response.to_lowercase().contains("created invoice"),
            "fabricated claim should be suppressed: {:?}",
            result.response
        );
        // Should ask for confirmation instead.
        assert!(
            result.response.to_lowercase().contains("confirm")
                || result.response.to_lowercase().contains("would you like"),
            "should ask for confirmation: {:?}",
            result.response
        );
    }

    /// Model produces legitimate conversational text with no tool calls →
    /// anti-fabrication guard must NOT fire.
    #[tokio::test]
    async fn anti_fabrication_guard_does_not_fire_on_conversational_response() {
        let engine = Arc::new(MockEngine::single_text(
            "I can help you create a node. What title would you like to give it?",
        ));
        let executor = Arc::new(MockToolExecutor::new());
        let agent_loop = LocalAgentLoop::new(engine, executor);

        let mut session = new_session();
        let result = agent_loop
            .run_turn(
                &mut session,
                "I want to create a note",
                |_| {},
                |_| {},
                CancellationToken::new(),
            )
            .await
            .unwrap();

        assert!(result.tool_calls_made.is_empty());
        assert_eq!(
            result.response, "I can help you create a node. What title would you like to give it?",
            "conversational response should pass through unchanged"
        );
    }

    /// Model claims action AFTER successfully executing tool calls → guard
    /// must NOT fire (the claim is grounded in real tool executions).
    #[tokio::test]
    async fn anti_fabrication_guard_does_not_fire_after_real_tool_calls() {
        let engine = Arc::new(MockEngine::tool_then_text(
            "search_nodes",
            r#"{"query":"invoice"}"#,
            "I found 2 invoice nodes in your workspace.",
        ));
        let executor = Arc::new(MockToolExecutor::new());
        let agent_loop = LocalAgentLoop::new(engine, executor);

        let mut session = new_session();
        let result = agent_loop
            .run_turn(
                &mut session,
                "find my invoice nodes",
                |_| {},
                |_| {},
                CancellationToken::new(),
            )
            .await
            .unwrap();

        assert_eq!(result.tool_calls_made.len(), 1);
        assert_eq!(
            result.response, "I found 2 invoice nodes in your workspace.",
            "grounded response after real tool call should pass through unchanged"
        );
    }

    // -- Fabricated-id guard: unit tests -------------------------------------

    #[test]
    fn extract_node_uris_finds_bare_uri_and_stops_at_whitespace() {
        let uris = extract_node_uris("See nodespace://abc-123 for details.");
        assert_eq!(uris, vec!["nodespace://abc-123"]);
    }

    #[test]
    fn extract_node_uris_dedupes_repeated_ids() {
        let uris = extract_node_uris("nodespace://abc and again nodespace://abc later.");
        assert_eq!(uris, vec!["nodespace://abc"]);
    }

    #[test]
    fn extract_node_uris_stops_at_markdown_delimiters() {
        // A link/backtick form must not sweep the closing delimiter into the id.
        assert_eq!(
            extract_node_uris("[Task](nodespace://abc-123)"),
            vec!["nodespace://abc-123"]
        );
        assert_eq!(
            extract_node_uris("`nodespace://abc-123`"),
            vec!["nodespace://abc-123"]
        );
    }

    #[test]
    fn extract_node_uris_stops_at_a_json_string_delimiter() {
        // `grounded_node_uris_from_history` scans raw serialized tool-result
        // JSON, where an id is always immediately followed by a closing `"`.
        let json = r#"{"id":"nodespace://abc-123","property_count":0}"#;
        assert_eq!(extract_node_uris(json), vec!["nodespace://abc-123"]);
    }

    #[test]
    fn extract_node_uris_empty_when_none_present() {
        assert!(extract_node_uris("No ids mentioned here.").is_empty());
    }

    #[test]
    fn grounded_node_uris_collects_from_result_and_args() {
        let executions = vec![
            exec_record_with(
                "create_node",
                json!({"content": "Rebuild reports page"}),
                json!({"id": "nodespace://real-1", "property_count": 0}),
            ),
            exec_record_with(
                "update_node",
                json!({"id": "nodespace://real-1", "content": "..."}),
                json!({"id": "nodespace://real-1", "property_count": 1}),
            ),
        ];
        let grounded = grounded_node_uris(&executions);
        assert_eq!(grounded.len(), 1);
        assert!(grounded.contains("nodespace://real-1"));
    }

    #[test]
    fn grounded_node_uris_walks_nested_search_results() {
        let executions = vec![exec_record_with(
            "search_nodes",
            json!({"query": "invoice"}),
            json!({"count": 2, "results": [
                {"id": "nodespace://a"},
                {"id": "nodespace://b"},
            ]}),
        )];
        let grounded = grounded_node_uris(&executions);
        assert!(grounded.contains("nodespace://a"));
        assert!(grounded.contains("nodespace://b"));
    }

    #[test]
    fn ungrounded_node_uris_flags_id_no_tool_ever_produced() {
        let executions = vec![exec_record_with(
            "create_node",
            json!({"content": "Rebuild reports page"}),
            json!({"id": "nodespace://real-1", "property_count": 0}),
        )];
        let text = "The task was created as nodespace://cbaedefg-abcd-1234-wxyz-deadbeefcafe.";
        let session = new_session();
        let bad = ungrounded_node_uris(text, &executions, &session);
        assert_eq!(
            bad,
            vec!["nodespace://cbaedefg-abcd-1234-wxyz-deadbeefcafe".to_string()]
        );
    }

    #[test]
    fn ungrounded_node_uris_empty_when_every_id_is_grounded() {
        let executions = vec![exec_record_with(
            "create_node",
            json!({"content": "Rebuild reports page"}),
            json!({"id": "nodespace://real-1", "property_count": 0}),
        )];
        let text = "Created as nodespace://real-1.";
        let session = new_session();
        assert!(ungrounded_node_uris(text, &executions, &session).is_empty());
    }

    #[test]
    fn ungrounded_node_uris_does_not_flag_an_id_grounded_only_by_args() {
        // The args-laundering path a reviewer flagged: a fabricated id passed
        // as an argument to some unrelated executing call must NOT enter the
        // grounded set merely by having been echoed back in that call's args.
        let executions = vec![exec_record_with(
            "search_nodes",
            json!({"query": "nodespace://invented-id"}),
            json!({"count": 0, "results": []}),
        )];
        let text = "Found it: nodespace://invented-id.";
        let session = new_session();
        let bad = ungrounded_node_uris(text, &executions, &session);
        assert_eq!(bad, vec!["nodespace://invented-id".to_string()]);
    }

    #[test]
    fn ungrounded_node_uris_is_grounded_by_a_prior_turns_tool_result_in_history() {
        // A real id created several turns ago is legitimately still
        // referenceable even with zero tool calls in the CURRENT turn.
        let mut session = new_session();
        session.messages.push(ChatMessage::tool_result(
            serde_json::to_string(&json!({"id": "nodespace://old-real-id", "property_count": 0}))
                .unwrap(),
            "tc_prior",
            "create_node",
        ));
        let text = "That was the task created earlier as nodespace://old-real-id.";
        assert!(ungrounded_node_uris(text, &[], &session).is_empty());
    }

    // -- Fabricated-id guard: loop-level tests -------------------------------

    /// The exact shape observed in #2281: two `create_node` calls actually ran
    /// and returned a real id, but the model's text names a different,
    /// invented `nodespace://` id (not a UUID — contains g/w/x/y/z, ends in a
    /// placeholder literal). This must never reach the user; it must not be
    /// confused for the real id either.
    #[tokio::test]
    async fn fabricated_id_guard_converts_a_response_naming_an_invented_id() {
        let response = run_guard_turn(
            "create_node",
            r#"{"content":"Rebuild reports page functionality","node_type":"task"}"#,
            json!({"id": "nodespace://d7e3bb35-170a-4865-a6f6-063fbd1e0a09", "property_count": 0}),
            "The task \"Rebuild reports page functionality to support client-side rendering (CSR) instead of SSR.\" was created as a new record with ID nodespace://cbaedefg-abcd-1234-wxyz-deadbeefcafe in the 'task' schema.",
        )
        .await;
        assert_eq!(
            response, CONFIRMATION_REQUEST,
            "a response naming an id no tool call produced must not reach the user"
        );
    }

    #[tokio::test]
    async fn fabricated_id_guard_leaves_a_response_naming_the_real_id_alone() {
        let response = run_guard_turn(
            "create_node",
            r#"{"content":"Rebuild reports page","node_type":"task"}"#,
            json!({"id": "nodespace://d7e3bb35-170a-4865-a6f6-063fbd1e0a09", "property_count": 0}),
            "Created the task as nodespace://d7e3bb35-170a-4865-a6f6-063fbd1e0a09.",
        )
        .await;
        assert_eq!(
            response,
            "Created the task as nodespace://d7e3bb35-170a-4865-a6f6-063fbd1e0a09."
        );
    }

    #[tokio::test]
    async fn fabricated_id_guard_does_not_fire_when_response_names_no_id() {
        let response = run_guard_turn(
            "search_nodes",
            r#"{"query":"invoice"}"#,
            json!({"count": 2, "results": [{"id": "nodespace://a"}, {"id": "nodespace://b"}]}),
            "I found 2 invoice nodes in your workspace.",
        )
        .await;
        assert_eq!(response, "I found 2 invoice nodes in your workspace.");
    }

    // -- No-op-success guard: loop-level tests -------------------------------
    //
    // The guard's correctness depends on the INTERACTION of two predicates
    // (`persisted_field_count` and `contains_action_claim`) with the turn's
    // accumulated `all_tool_executions`. Unit tests of either predicate alone
    // cannot show that, so these drive the real `run_turn`.

    /// Executor returning one fixed successful tool result.
    struct FixedResultExecutor {
        tool: &'static str,
        result: serde_json::Value,
    }

    #[async_trait]
    impl AgentToolExecutor for FixedResultExecutor {
        async fn available_tools(&self) -> Result<Vec<ToolDefinition>, ToolError> {
            Ok(vec![ToolDefinition {
                name: self.tool.into(),
                description: "test tool".into(),
                parameters_schema: json!({"type": "object"}),
            }])
        }
        async fn execute(
            &self,
            name: &str,
            _a: serde_json::Value,
        ) -> Result<ToolResult, ToolError> {
            Ok(ToolResult {
                tool_call_id: "tc_1".into(),
                name: name.into(),
                result: self.result.clone(),
                is_error: false,
            })
        }
    }

    async fn run_guard_turn(
        tool: &'static str,
        args: &'static str,
        result: serde_json::Value,
        final_text: &'static str,
    ) -> String {
        let engine = Arc::new(MockEngine::tool_then_text(tool, args, final_text));
        let agent_loop =
            LocalAgentLoop::new(engine, Arc::new(FixedResultExecutor { tool, result }));
        let mut session = new_session();
        agent_loop
            .run_turn(
                &mut session,
                "do the thing",
                |_| {},
                |_| {},
                CancellationToken::new(),
            )
            .await
            .unwrap()
            .response
    }

    #[tokio::test]
    async fn noop_guard_converts_a_claim_backed_only_by_an_empty_write() {
        // The #1937 shape: update_node ran, succeeded, persisted nothing, and
        // the model reported the write as done.
        let response = run_guard_turn(
            "update_node",
            r#"{"id":"abc","content":"Schedule chip upgrade on the Polestar"}"#,
            json!({"id": "nodespace://abc", "property_count": 0}),
            "I have updated the due date to 2026-08-06.",
        )
        .await;
        assert_eq!(
            response, CONFIRMATION_REQUEST,
            "a claim backed only by a write that persisted nothing must not reach the user"
        );
    }

    #[tokio::test]
    async fn noop_guard_leaves_a_claim_backed_by_a_real_write_alone() {
        let response = run_guard_turn(
            "update_node",
            r#"{"id":"abc","field_values":{"due_date":"2026-08-06"}}"#,
            json!({"id": "nodespace://abc", "updated": true, "property_count": 1}),
            "I have updated the due date to 2026-08-06.",
        )
        .await;
        assert_eq!(
            response, "I have updated the due date to 2026-08-06.",
            "a claim grounded in a real write must pass through untouched"
        );
    }

    #[tokio::test]
    async fn noop_guard_leaves_a_truthful_content_only_edit_alone() {
        // A rename genuinely persists a change but reports property_count 0.
        // Converting this would turn a correct confirmation into a spurious
        // "please confirm" — the guard as a UX regression.
        let response = run_guard_turn(
            "update_node",
            r#"{"id":"abc","content":"Buy milk and eggs"}"#,
            json!({
                "id": "nodespace://abc",
                "property_count": 0,
                "updated_content_only": true,
            }),
            "The title was updated to \"Buy milk and eggs\".",
        )
        .await;
        assert_eq!(response, "The title was updated to \"Buy milk and eggs\".");
    }

    #[tokio::test]
    async fn noop_guard_leaves_a_plain_note_creation_alone() {
        // A `text` node has no schema fields, so zero properties is a complete
        // success, not dropped particulars.
        let response = run_guard_turn(
            "create_node",
            r#"{"content":"Remember to call the dentist","node_type":"text"}"#,
            json!({"id": "nodespace://xyz", "property_count": 0, "content_only": true}),
            "I created a note: \"Remember to call the dentist\".",
        )
        .await;
        assert_eq!(
            response,
            "I created a note: \"Remember to call the dentist\"."
        );
    }

    // -- Tool-failure surfacing tests ----------------------------------------

    /// When a tool fails and the model doesn't mention the error, an error
    /// note should be appended to the response.
    #[tokio::test]
    async fn tool_failure_appended_when_model_ignores_error() {
        // Tool executor that always reports an error
        struct ErrorToolExecutor;

        #[async_trait]
        impl AgentToolExecutor for ErrorToolExecutor {
            async fn available_tools(&self) -> Result<Vec<ToolDefinition>, ToolError> {
                Ok(vec![ToolDefinition {
                    name: "update_node".into(),
                    description: "Update a node".into(),
                    parameters_schema: json!({"type": "object"}),
                }])
            }

            async fn execute(
                &self,
                name: &str,
                _args: serde_json::Value,
            ) -> Result<ToolResult, ToolError> {
                Ok(ToolResult {
                    tool_call_id: "tc_1".into(),
                    name: name.into(),
                    result: json!({"error": "node not found"}),
                    is_error: true,
                })
            }
        }

        let engine = Arc::new(MockEngine::tool_then_text(
            "update_node",
            r#"{"id":"abc"}"#,
            // Model papers over the error with a success claim
            "All done! The node has been updated.",
        ));
        let executor = Arc::new(ErrorToolExecutor);
        let agent_loop = LocalAgentLoop::new(engine, executor);

        let mut session = new_session();
        let result = agent_loop
            .run_turn(
                &mut session,
                "update node abc",
                |_| {},
                |_| {},
                CancellationToken::new(),
            )
            .await
            .unwrap();

        assert_eq!(result.tool_calls_made.len(), 1);
        assert!(
            result.tool_calls_made[0].is_error,
            "tool execution should be marked as error"
        );
        // Error note should be appended since model didn't mention the failure.
        assert!(
            result.response.contains("⚠️") || result.response.to_lowercase().contains("error"),
            "error note should be present: {:?}",
            result.response
        );
    }

    /// A same-turn retry that succeeds must clear the failure: the model
    /// called `search_nodes` with malformed args (error), self-corrected with
    /// valid args in the next iteration (success), and the final answer is
    /// grounded in the successful retry. The guard must NOT replace this with
    /// the generic error message — the earlier failure is superseded.
    #[tokio::test]
    async fn tool_failure_superseded_by_later_success_does_not_trip_guard() {
        struct FailThenSucceedExecutor {
            calls: Arc<AtomicUsize>,
        }

        #[async_trait]
        impl AgentToolExecutor for FailThenSucceedExecutor {
            async fn available_tools(&self) -> Result<Vec<ToolDefinition>, ToolError> {
                Ok(vec![ToolDefinition {
                    name: "search_nodes".into(),
                    description: "Search nodes".into(),
                    parameters_schema: json!({"type": "object"}),
                }])
            }

            async fn execute(
                &self,
                name: &str,
                _args: serde_json::Value,
            ) -> Result<ToolResult, ToolError> {
                let call_idx = self.calls.fetch_add(1, Ordering::SeqCst);
                if call_idx == 0 {
                    Ok(ToolResult {
                        tool_call_id: "tc_0".into(),
                        name: name.into(),
                        result: json!({"error": "missing field 'type'"}),
                        is_error: true,
                    })
                } else {
                    Ok(ToolResult {
                        tool_call_id: "tc_1".into(),
                        name: name.into(),
                        result: json!({"count": 1, "nodes": [
                            {"id": "abc123", "title": "Buy something from grocery store", "type": "task"},
                        ]}),
                        is_error: false,
                    })
                }
            }
        }

        let rounds = vec![
            // Iteration 0: malformed filter — fails.
            vec![
                StreamingChunk::ToolCallStart {
                    id: "tc_0".to_string(),
                    name: "search_nodes".to_string(),
                    provider_extra: None,
                },
                StreamingChunk::ToolCallArgs {
                    id: "tc_0".to_string(),
                    args_json: r#"{"filters":[{"field":"title"}]}"#.to_string(),
                },
                StreamingChunk::Done {
                    usage: InferenceUsage {
                        prompt_tokens: 10,
                        completion_tokens: 5,
                    },
                },
            ],
            // Iteration 1: self-corrected, valid call — succeeds.
            vec![
                StreamingChunk::ToolCallStart {
                    id: "tc_1".to_string(),
                    name: "search_nodes".to_string(),
                    provider_extra: None,
                },
                StreamingChunk::ToolCallArgs {
                    id: "tc_1".to_string(),
                    args_json: r#"{"node_type":"task","query":"grocery store"}"#.to_string(),
                },
                StreamingChunk::Done {
                    usage: InferenceUsage {
                        prompt_tokens: 10,
                        completion_tokens: 5,
                    },
                },
            ],
            // Iteration 2: final answer, grounded in the successful retry.
            vec![
                StreamingChunk::Token {
                    text: "Yes, there is a task titled \"Buy something from grocery store\"."
                        .to_string(),
                },
                StreamingChunk::Done {
                    usage: InferenceUsage {
                        prompt_tokens: 20,
                        completion_tokens: 10,
                    },
                },
            ],
        ];

        let engine = Arc::new(MockEngine::new(rounds));
        let executor = Arc::new(FailThenSucceedExecutor {
            calls: Arc::new(AtomicUsize::new(0)),
        });
        let agent_loop = LocalAgentLoop::new(engine, executor);

        let mut session = new_session();
        let result = agent_loop
            .run_turn(
                &mut session,
                "Was any of the task called \"Buy something from grocery store\"?",
                |_| {},
                |_| {},
                CancellationToken::new(),
            )
            .await
            .unwrap();

        assert_eq!(result.tool_calls_made.len(), 2);
        assert!(result.tool_calls_made[0].is_error);
        assert!(!result.tool_calls_made[1].is_error);
        assert_eq!(
            result.response, "Yes, there is a task titled \"Buy something from grocery store\".",
            "the correct, self-corrected answer must pass through unchanged, not be \
             replaced with the generic tool-error message"
        );
    }

    /// A tool failure with NO successful retry — the model's final response
    /// ignores the failure — must still trip the guard. This is the case the
    /// superseded-failure narrowing above must not reintroduce: a genuinely
    /// unaddressed failure has no later success to supersede it.
    #[tokio::test]
    async fn tool_failure_without_retry_still_trips_guard() {
        struct AlwaysFailExecutor;

        #[async_trait]
        impl AgentToolExecutor for AlwaysFailExecutor {
            async fn available_tools(&self) -> Result<Vec<ToolDefinition>, ToolError> {
                Ok(vec![ToolDefinition {
                    name: "search_nodes".into(),
                    description: "Search nodes".into(),
                    parameters_schema: json!({"type": "object"}),
                }])
            }

            async fn execute(
                &self,
                name: &str,
                _args: serde_json::Value,
            ) -> Result<ToolResult, ToolError> {
                Ok(ToolResult {
                    tool_call_id: "tc_0".into(),
                    name: name.into(),
                    result: json!({"error": "missing field 'type'"}),
                    is_error: true,
                })
            }
        }

        let engine = Arc::new(MockEngine::tool_then_text(
            "search_nodes",
            r#"{"node_type":"task","query":"groceries"}"#,
            // Model ignores the failure and claims success anyway.
            "Yes, I found that task.",
        ));
        let executor = Arc::new(AlwaysFailExecutor);
        let agent_loop = LocalAgentLoop::new(engine, executor);

        let mut session = new_session();
        let result = agent_loop
            .run_turn(
                &mut session,
                "Was any of the task called groceries?",
                |_| {},
                |_| {},
                CancellationToken::new(),
            )
            .await
            .unwrap();

        assert_eq!(result.tool_calls_made.len(), 1);
        assert!(result.tool_calls_made[0].is_error);
        assert!(
            result.response.contains("⚠️") || result.response.to_lowercase().contains("error"),
            "an unretried failure must still surface the error, got: {:?}",
            result.response
        );
    }

    /// The supersession filter is per-tool, not turn-wide: a failed
    /// `search_nodes` call superseded by a later successful `search_nodes`
    /// call must NOT mask an unrelated, never-retried `update_node` failure
    /// earlier in the same turn. Guards against an implementation that clears
    /// ALL failures once ANY later success appears anywhere in the turn,
    /// rather than scoping supersession to same-named tool executions.
    #[tokio::test]
    async fn superseded_failure_of_one_tool_does_not_mask_unretried_failure_of_another() {
        struct TwoToolExecutor;

        #[async_trait]
        impl AgentToolExecutor for TwoToolExecutor {
            async fn available_tools(&self) -> Result<Vec<ToolDefinition>, ToolError> {
                Ok(vec![
                    ToolDefinition {
                        name: "update_node".into(),
                        description: "Update a node".into(),
                        parameters_schema: json!({"type": "object"}),
                    },
                    ToolDefinition {
                        name: "search_nodes".into(),
                        description: "Search nodes".into(),
                        parameters_schema: json!({"type": "object"}),
                    },
                ])
            }

            async fn execute(
                &self,
                name: &str,
                _args: serde_json::Value,
            ) -> Result<ToolResult, ToolError> {
                match name {
                    // update_node always fails and is never retried.
                    "update_node" => Ok(ToolResult {
                        tool_call_id: "tc_update".into(),
                        name: name.into(),
                        result: json!({"error": "node not found"}),
                        is_error: true,
                    }),
                    // search_nodes fails once, then succeeds on retry.
                    "search_nodes" => Ok(ToolResult {
                        tool_call_id: "tc_search".into(),
                        name: name.into(),
                        result: json!({"count": 1, "nodes": [{"id": "abc123"}]}),
                        is_error: false,
                    }),
                    _ => unreachable!(),
                }
            }
        }

        let rounds = vec![
            // Iteration 0: update_node fails, never retried.
            vec![
                StreamingChunk::ToolCallStart {
                    id: "tc_update".to_string(),
                    name: "update_node".to_string(),
                    provider_extra: None,
                },
                StreamingChunk::ToolCallArgs {
                    id: "tc_update".to_string(),
                    args_json: r#"{"id":"missing-id"}"#.to_string(),
                },
                StreamingChunk::Done {
                    usage: InferenceUsage {
                        prompt_tokens: 10,
                        completion_tokens: 5,
                    },
                },
            ],
            // Iteration 1: search_nodes fails with malformed filter.
            vec![
                StreamingChunk::ToolCallStart {
                    id: "tc_search_0".to_string(),
                    name: "search_nodes".to_string(),
                    provider_extra: None,
                },
                StreamingChunk::ToolCallArgs {
                    id: "tc_search_0".to_string(),
                    args_json: r#"{"filters":[{"field":"title"}]}"#.to_string(),
                },
                StreamingChunk::Done {
                    usage: InferenceUsage {
                        prompt_tokens: 10,
                        completion_tokens: 5,
                    },
                },
            ],
            // Iteration 2: search_nodes retried and succeeds.
            vec![
                StreamingChunk::ToolCallStart {
                    id: "tc_search_1".to_string(),
                    name: "search_nodes".to_string(),
                    provider_extra: None,
                },
                StreamingChunk::ToolCallArgs {
                    id: "tc_search_1".to_string(),
                    args_json: r#"{"node_type":"task","query":"grocery store"}"#.to_string(),
                },
                StreamingChunk::Done {
                    usage: InferenceUsage {
                        prompt_tokens: 10,
                        completion_tokens: 5,
                    },
                },
            ],
            // Iteration 3: final answer ignores the still-failed update.
            vec![
                StreamingChunk::Token {
                    text: "I found the task you were looking for.".to_string(),
                },
                StreamingChunk::Done {
                    usage: InferenceUsage {
                        prompt_tokens: 20,
                        completion_tokens: 10,
                    },
                },
            ],
        ];

        let engine = Arc::new(MockEngine::new(rounds));
        let executor = Arc::new(TwoToolExecutor);
        let agent_loop = LocalAgentLoop::new(engine, executor);

        let mut session = new_session();
        let result = agent_loop
            .run_turn(
                &mut session,
                "Update the node and find the grocery task",
                |_| {},
                |_| {},
                CancellationToken::new(),
            )
            .await
            .unwrap();

        assert_eq!(result.tool_calls_made.len(), 3);
        assert!(
            result.response.contains("⚠️") || result.response.to_lowercase().contains("error"),
            "update_node's unretried failure must still surface even though \
             search_nodes's failure was superseded by its own later success, got: {:?}",
            result.response
        );
    }

    /// Unparseable tool arguments must be reported as unparseable — never
    /// silently replaced with `{}` and executed.
    ///
    /// Substituting an empty object sends the tool a payload the model never
    /// wrote, so the failure comes back as a missing required field. The model
    /// then "repairs" an argument it did not get wrong, which is how a single
    /// malformed call turns into a run of progressively worse retries.
    #[tokio::test]
    async fn malformed_tool_arguments_are_not_executed_as_empty_object() {
        use std::sync::atomic::{AtomicUsize, Ordering};

        struct RecordingExecutor {
            calls: Arc<AtomicUsize>,
        }

        #[async_trait]
        impl AgentToolExecutor for RecordingExecutor {
            async fn available_tools(&self) -> Result<Vec<ToolDefinition>, ToolError> {
                Ok(vec![ToolDefinition {
                    name: "create_node".into(),
                    description: "Create a node".into(),
                    parameters_schema: json!({"type": "object"}),
                }])
            }

            async fn execute(
                &self,
                name: &str,
                _args: serde_json::Value,
            ) -> Result<ToolResult, ToolError> {
                self.calls.fetch_add(1, Ordering::SeqCst);
                Ok(ToolResult {
                    tool_call_id: "tc_1".into(),
                    name: name.into(),
                    result: json!({"id": "nodespace://created"}),
                    is_error: false,
                })
            }
        }

        let calls = Arc::new(AtomicUsize::new(0));
        let engine = Arc::new(MockEngine::tool_then_text(
            "create_node",
            // Truncated mid-object — a real shape observed from small models.
            r#"{"content":"Kind of Blue","node_type":"#,
            "Added it for you.",
        ));
        let executor = Arc::new(RecordingExecutor {
            calls: calls.clone(),
        });
        let agent_loop = LocalAgentLoop::new(engine, executor);

        let mut session = new_session();
        let result = agent_loop
            .run_turn(
                &mut session,
                "put down Kind of Blue",
                |_| {},
                |_| {},
                CancellationToken::new(),
            )
            .await
            .unwrap();

        assert_eq!(
            calls.load(Ordering::SeqCst),
            0,
            "the tool must not run at all when the model's arguments are not valid JSON"
        );
        assert_eq!(result.tool_calls_made.len(), 1);
        assert!(
            result.tool_calls_made[0].is_error,
            "the malformed call must be recorded as an error"
        );
    }

    /// End-to-end: a tool call whose keys the model over-quoted must reach the
    /// tool with those keys repaired, and the turn must complete normally.
    ///
    /// Without the repair this call is valid JSON carrying unusable keys, so it
    /// reaches the tool intact, is rejected on its merits, and the rejected
    /// shape stays in the conversation for the model to copy on every retry —
    /// the reported retry loop, which no parse-failure or duplicate guard can
    /// break because the arguments always parse and the model can vary the tool.
    #[tokio::test]
    async fn over_quoted_argument_keys_reach_the_tool_repaired() {
        use std::sync::Mutex;

        struct CapturingExecutor {
            seen: Arc<Mutex<Vec<serde_json::Value>>>,
        }

        #[async_trait]
        impl AgentToolExecutor for CapturingExecutor {
            async fn available_tools(&self) -> Result<Vec<ToolDefinition>, ToolError> {
                Ok(vec![ToolDefinition {
                    name: "create_schema".into(),
                    description: "Create a schema".into(),
                    parameters_schema: json!({"type": "object"}),
                }])
            }

            async fn execute(
                &self,
                name: &str,
                args: serde_json::Value,
            ) -> Result<ToolResult, ToolError> {
                self.seen.lock().unwrap().push(args);
                Ok(ToolResult {
                    tool_call_id: "tc_1".into(),
                    name: name.into(),
                    result: json!({"schemaId": "venue"}),
                    is_error: false,
                })
            }
        }

        let seen = Arc::new(Mutex::new(Vec::new()));
        let engine = Arc::new(MockEngine::tool_then_text(
            "create_schema",
            // Exactly the wire shape measured from gemma-4-e4b on a retry after
            // a rejected call (see nlp-engine/tests/toolcall_json_shape.rs).
            r#"{"name":"Venue","fields":[{"\"name\"":"capacity","\"type\"":"number"}]}"#,
            "Created the Venue type.",
        ));
        let executor = Arc::new(CapturingExecutor { seen: seen.clone() });
        let agent_loop = LocalAgentLoop::new(engine, executor);

        let mut session = new_session();
        let result = agent_loop
            .run_turn(
                &mut session,
                "create a Venue type with a capacity number field",
                |_| {},
                |_| {},
                CancellationToken::new(),
            )
            .await
            .unwrap();

        let captured = seen.lock().unwrap().clone();
        assert_eq!(captured.len(), 1, "the tool must run exactly once");
        assert_eq!(
            captured[0],
            json!({"name":"Venue","fields":[{"name":"capacity","type":"number"}]}),
            "the tool must receive the repaired keys, not the model's over-quoted ones"
        );
        assert_eq!(result.tool_calls_made.len(), 1);
        assert!(
            !result.tool_calls_made[0].is_error,
            "a repaired call must succeed rather than be rejected"
        );
    }

    /// #1943, the propagation half: the assistant turn persisted into history
    /// must carry the *repaired* arguments, not the malformed ones the model
    /// emitted.
    ///
    /// This is what made the reported retry loop unrecoverable. Repairing only
    /// the copy handed to the tool (which the sibling test above covers) still
    /// left `arguments_json` malformed in the record, and the chat template
    /// replays that string verbatim into the next prompt — where the model
    /// copies the shape it reads, measured at 8 of 8. So every attempt was
    /// repaired for the tool and simultaneously re-taught to the model. Asserted
    /// on `session.messages` rather than on what the tool saw, because the
    /// record is the channel the defect travelled through.
    #[tokio::test]
    async fn history_carries_the_repaired_arguments_not_the_malformed_ones() {
        struct OkExecutor;

        #[async_trait]
        impl AgentToolExecutor for OkExecutor {
            async fn available_tools(&self) -> Result<Vec<ToolDefinition>, ToolError> {
                Ok(vec![ToolDefinition {
                    name: "search_nodes".into(),
                    description: "Search".into(),
                    parameters_schema: json!({"type": "object"}),
                }])
            }

            async fn execute(
                &self,
                name: &str,
                _args: serde_json::Value,
            ) -> Result<ToolResult, ToolError> {
                Ok(ToolResult {
                    tool_call_id: "tc_1".into(),
                    name: name.into(),
                    result: json!({"count": 1}),
                    is_error: false,
                })
            }
        }

        // The issue's verbatim 3-of-3 reproduction payload.
        let engine = Arc::new(MockEngine::tool_then_text(
            "search_nodes",
            r#"{"filters":[{"\"operator\"":"equals","\"property\"":"status","\"type\"":"task\",\"value\":"}],"\"node_type\"":"task","query":null}"#,
            "There is 1 open task.",
        ));
        let agent_loop = LocalAgentLoop::new(engine, Arc::new(OkExecutor));

        let mut session = new_session();
        agent_loop
            .run_turn(
                &mut session,
                "How many tasks are still open?",
                |_| {},
                |_| {},
                CancellationToken::new(),
            )
            .await
            .unwrap();

        let persisted: Vec<&str> = session
            .messages
            .iter()
            .flat_map(|m| m.tool_calls.iter())
            .map(|tc| tc.arguments_json.as_str())
            .collect();
        assert_eq!(
            persisted.len(),
            1,
            "exactly one tool call should have been persisted, got {persisted:?}"
        );
        let recorded: serde_json::Value =
            serde_json::from_str(persisted[0]).expect("the persisted arguments must be valid JSON");
        assert_eq!(
            recorded,
            json!({
                "filters": [{"operator": "equals", "property": "status", "type": "task"}],
                "node_type": "task",
                "query": null
            }),
            "history must carry the repaired shape — a malformed record is replayed \
             into the next prompt and copied by the model, which is the loop this fixes"
        );
    }

    /// A model emitting *differently*-malformed arguments each round must not be
    /// able to burn every iteration.
    ///
    /// The duplicate-call guard cannot catch this: it keys on canonical argument
    /// strings, and no two malformed attempts are identical, so nothing matches.
    #[tokio::test]
    async fn repeated_unparseable_arguments_break_the_turn() {
        use std::sync::atomic::{AtomicUsize, Ordering};

        struct NeverCalledExecutor {
            calls: Arc<AtomicUsize>,
        }

        #[async_trait]
        impl AgentToolExecutor for NeverCalledExecutor {
            async fn available_tools(&self) -> Result<Vec<ToolDefinition>, ToolError> {
                Ok(vec![ToolDefinition {
                    name: "create_node".into(),
                    description: "Create a node".into(),
                    parameters_schema: json!({"type": "object"}),
                }])
            }

            async fn execute(
                &self,
                name: &str,
                _args: serde_json::Value,
            ) -> Result<ToolResult, ToolError> {
                self.calls.fetch_add(1, Ordering::SeqCst);
                Ok(ToolResult {
                    tool_call_id: "tc".into(),
                    name: name.into(),
                    result: json!({}),
                    is_error: false,
                })
            }
        }

        // Each round is malformed in a DIFFERENT way, so canonical-args dedup
        // never matches. Supply more rounds than the guard should allow.
        let malformed = [
            r#"{"content":"a","node_type":"#,
            r#"{"content":"b",,}"#,
            r#"{"content":"c""#,
            r#"{"content":"d"}}"#,
        ];
        let rounds: Vec<Vec<StreamingChunk>> = malformed
            .iter()
            .enumerate()
            .map(|(i, args)| {
                vec![
                    StreamingChunk::ToolCallStart {
                        id: format!("tc_{i}"),
                        name: "create_node".to_string(),
                        provider_extra: None,
                    },
                    StreamingChunk::ToolCallArgs {
                        id: format!("tc_{i}"),
                        args_json: (*args).to_string(),
                    },
                    StreamingChunk::Done {
                        usage: InferenceUsage {
                            prompt_tokens: 10,
                            completion_tokens: 5,
                        },
                    },
                ]
            })
            .collect();

        let calls = Arc::new(AtomicUsize::new(0));
        let engine = Arc::new(MockEngine::new(rounds));
        let executor = Arc::new(NeverCalledExecutor {
            calls: calls.clone(),
        });
        let agent_loop = LocalAgentLoop::new(engine, executor);

        let mut session = new_session();
        let result = agent_loop
            .run_turn(
                &mut session,
                "put down Kind of Blue",
                |_| {},
                |_| {},
                CancellationToken::new(),
            )
            .await
            .unwrap();

        assert_eq!(
            calls.load(Ordering::SeqCst),
            0,
            "no tool should ever execute — every call had invalid JSON"
        );
        assert!(
            result.tool_calls_made.len() <= MAX_CONSECUTIVE_PARSE_FAILURES,
            "the turn must stop after {} consecutive parse failures, got {} attempts",
            MAX_CONSECUTIVE_PARSE_FAILURES,
            result.tool_calls_made.len()
        );
        assert!(
            result.tool_calls_made.iter().all(|r| r.is_error),
            "every recorded attempt must be marked as an error"
        );
    }

    /// A tool call carrying no arguments at all is a different case: `{}` is the
    /// faithful reading, so the call proceeds and the tool's own required-field
    /// error is the correct message.
    #[tokio::test]
    async fn absent_tool_arguments_still_reach_the_tool() {
        use std::sync::atomic::{AtomicUsize, Ordering};

        struct RecordingExecutor {
            calls: Arc<AtomicUsize>,
        }

        #[async_trait]
        impl AgentToolExecutor for RecordingExecutor {
            async fn available_tools(&self) -> Result<Vec<ToolDefinition>, ToolError> {
                Ok(vec![ToolDefinition {
                    name: "create_node".into(),
                    description: "Create a node".into(),
                    parameters_schema: json!({"type": "object"}),
                }])
            }

            async fn execute(
                &self,
                name: &str,
                args: serde_json::Value,
            ) -> Result<ToolResult, ToolError> {
                self.calls.fetch_add(1, Ordering::SeqCst);
                assert_eq!(args, json!({}), "empty arguments should arrive as {{}}");
                Ok(ToolResult {
                    tool_call_id: "tc_1".into(),
                    name: name.into(),
                    result: json!({"error": "missing field `node_type`"}),
                    is_error: true,
                })
            }
        }

        let calls = Arc::new(AtomicUsize::new(0));
        let engine = Arc::new(MockEngine::tool_then_text("create_node", "", "Done."));
        let executor = Arc::new(RecordingExecutor {
            calls: calls.clone(),
        });
        let agent_loop = LocalAgentLoop::new(engine, executor);

        let mut session = new_session();
        agent_loop
            .run_turn(
                &mut session,
                "put down Kind of Blue",
                |_| {},
                |_| {},
                CancellationToken::new(),
            )
            .await
            .unwrap();

        assert_eq!(
            calls.load(Ordering::SeqCst),
            1,
            "an argument-less call is well-formed and must still reach the tool"
        );
    }

    /// An empty response with no tool calls is an inference bug
    /// (model should always produce text or a tool call), not a UX surface.
    /// Surface as an error so it lands in logs/metrics rather than being
    /// silently masked.
    #[tokio::test]
    async fn empty_model_response_with_no_tools_returns_error() {
        let engine = Arc::new(MockEngine::single_text(""));
        let executor = Arc::new(MockToolExecutor::new());
        let agent_loop = LocalAgentLoop::new(engine, executor);

        let mut session = new_session();
        let result = agent_loop
            .run_turn(&mut session, "hi", |_| {}, |_| {}, CancellationToken::new())
            .await;

        let err = result.expect_err("empty model response should surface as an error");
        match err {
            InferenceError::Engine(msg) => assert!(msg.contains("empty response")),
            other => panic!("Expected Engine error, got {:?}", other),
        }
    }

    /// A mid-generation context-window overflow (`ChatChunk::Error`,
    /// surfaced to this loop as `StreamingChunk::Error` by the inference
    /// bridge) must fail the turn loudly, not have the truncated partial
    /// text silently accepted and persisted as a normal, complete response
    /// -- the exact "turn succeeds with truncated output and zero visible
    /// signal" failure mode the issue reported.
    #[tokio::test]
    async fn mid_generation_error_returns_error_and_does_not_persist_truncated_text() {
        let engine = Arc::new(MockEngine::new(vec![vec![
            StreamingChunk::Token {
                text: "This looks like a normal reply that got cut off mid".to_string(),
            },
            StreamingChunk::Error {
                message: "Context window full".to_string(),
            },
            // A real overflow still lets the engine finish and emit Done --
            // the mock mirrors that so this test exercises the same shape
            // `generate` actually returns (Ok(usage)), not an early bail.
            StreamingChunk::Done {
                usage: InferenceUsage {
                    prompt_tokens: 16000,
                    completion_tokens: 880,
                },
            },
        ]]));
        let executor = Arc::new(MockToolExecutor::new());
        let agent_loop = LocalAgentLoop::new(engine, executor);

        let mut session = new_session();
        let result = agent_loop
            .run_turn(&mut session, "hi", |_| {}, |_| {}, CancellationToken::new())
            .await;

        let err = result.expect_err(
            "a mid-generation error must fail the turn, not succeed with truncated output",
        );
        match err {
            InferenceError::Engine(msg) => assert!(
                msg.contains("Context window full"),
                "error must carry the underlying reason, got: {msg}"
            ),
            other => panic!("Expected Engine error, got {:?}", other),
        }

        // The user's message is pushed unconditionally before generation
        // runs, so it's expected to still be there -- what must NOT be there
        // is a fabricated assistant reply built from the truncated text.
        assert_eq!(
            session.messages.last().map(|m| &m.role),
            Some(&Role::User),
            "a failed turn must not push a fabricated assistant response to history"
        );
    }

    /// The main loop's mid-generation error check (tested above) hard-fails
    /// the whole turn -- correct there, since nothing useful has necessarily
    /// happened yet. This "max iterations reached" fallback has a different,
    /// deliberate design: it always synthesizes *some* response from
    /// whatever tool executions already happened, rather than ever failing
    /// the turn outright (see `EMPTY_RESPONSE_FALLBACK`/`summarize_executions`
    /// a few lines below in `run_turn`). A mid-generation error on the
    /// "final, tool-less inference" this branch runs must therefore be
    /// treated the same way this branch already treats an empty or
    /// narrated-tool-call response: don't trust it, fall through to the
    /// existing synthesis path -- not accepted as if it were a complete,
    /// real answer.
    #[tokio::test]
    async fn max_iterations_final_inference_error_falls_through_to_synthesis() {
        let tool_round = |i: usize| {
            vec![
                StreamingChunk::ToolCallStart {
                    id: format!("tc_{i}"),
                    name: "search_nodes".to_string(),
                    provider_extra: None,
                },
                StreamingChunk::ToolCallArgs {
                    id: format!("tc_{i}"),
                    args_json: format!(r#"{{"query":"test-{i}"}}"#),
                },
                StreamingChunk::Done {
                    usage: InferenceUsage {
                        prompt_tokens: 10,
                        completion_tokens: 5,
                    },
                },
            ]
        };

        let mut rounds: Vec<_> = (0..MAX_TOOL_ITERATIONS).map(tool_round).collect();
        // The "+1 extra" final, tool-less inference call: real text that got
        // cut off by a mid-generation error, not a genuine complete answer.
        rounds.push(vec![
            StreamingChunk::Token {
                text: "This looks like a real answer but".to_string(),
            },
            StreamingChunk::Error {
                message: "Context window full".to_string(),
            },
            StreamingChunk::Done {
                usage: InferenceUsage {
                    prompt_tokens: 16000,
                    completion_tokens: 880,
                },
            },
        ]);

        let engine = Arc::new(MockEngine::new(rounds));
        let executor = Arc::new(MockToolExecutor::new());
        let agent_loop = LocalAgentLoop::new(engine, executor);

        let mut session = new_session();
        let result = agent_loop
            .run_turn(
                &mut session,
                "Keep running search_nodes forever — verify the iteration cap stops it",
                |_| {},
                |_| {},
                CancellationToken::new(),
            )
            .await
            .expect(
                "this branch always synthesizes a response, even when the final \
                 inference call hit a mid-generation error",
            );

        assert!(
            !result
                .response
                .contains("This looks like a real answer but"),
            "a truncated response from a mid-generation error must not be accepted \
             as the final answer: {:?}",
            result.response
        );
    }

    /// Same principle as the test above, for the sibling "tail inference
    /// after loop break" branch (reached via the duplicate-call guard here,
    /// mirroring `duplicate_tool_call_breaks_loop`): a mid-generation error
    /// on the tail call must fall through to `summarize_executions`, not be
    /// accepted as the real response.
    #[tokio::test]
    async fn tail_inference_error_falls_through_to_synthesis() {
        let dup_call = || {
            vec![
                StreamingChunk::ToolCallStart {
                    id: "tc_dup".to_string(),
                    name: "search_nodes".to_string(),
                    provider_extra: None,
                },
                StreamingChunk::ToolCallArgs {
                    id: "tc_dup".to_string(),
                    args_json: r#"{"node_type":"task","query":"Test Task"}"#.to_string(),
                },
                StreamingChunk::Done {
                    usage: InferenceUsage {
                        prompt_tokens: 10,
                        completion_tokens: 5,
                    },
                },
            ]
        };

        let rounds = vec![
            // Round 0: first call — executes normally
            dup_call(),
            // Round 1: identical call — guard detects duplicate, breaks loop
            dup_call(),
            // Round 2: tail tool-less inference — text truncated by a
            // mid-generation error, not a genuine complete answer.
            vec![
                StreamingChunk::Token {
                    text: "I found the task, and also".to_string(),
                },
                StreamingChunk::Error {
                    message: "Context window full".to_string(),
                },
                StreamingChunk::Done {
                    usage: InferenceUsage {
                        prompt_tokens: 16000,
                        completion_tokens: 880,
                    },
                },
            ],
        ];

        let engine = Arc::new(MockEngine::new(rounds));
        let executor = Arc::new(MockToolExecutor::new());
        let agent_loop = LocalAgentLoop::new(engine, executor);

        let mut session = new_session();
        let result = agent_loop
            .run_turn(
                &mut session,
                "Find the task named Test Task",
                |_| {},
                |_| {},
                CancellationToken::new(),
            )
            .await
            .expect(
                "this branch always synthesizes a response, even when the tail \
                 inference call hit a mid-generation error",
            );

        assert!(
            !result.response.contains("I found the task, and also"),
            "a truncated response from a mid-generation error must not be accepted \
             as the final answer: {:?}",
            result.response
        );
    }

    // -- Silent-failure guard regression tests -----------------------

    /// Scenario B: the model prints a tool call as plain text instead of using
    /// the structured tool_calls field. `tool_calls == 0`, so the tool-failure
    /// and (phrase-based) fabrication guards don't fire — the narrated-call guard
    /// must catch it so the raw pseudo-code is never persisted as the answer.
    #[tokio::test]
    async fn narrated_tool_call_as_text_is_not_persisted_verbatim() {
        let engine = Arc::new(MockEngine::single_text(
            r#"search_nodes(node_type='task', filters=[{"property":"status", "operator": "equals", "value": "open"}])"#,
        ));
        let executor = Arc::new(MockToolExecutor::new());
        let agent_loop = LocalAgentLoop::new(engine, executor);

        let mut session = new_session();
        let result = agent_loop
            .run_turn(
                &mut session,
                "show my open tasks",
                |_| {},
                |_| {},
                CancellationToken::new(),
            )
            .await
            .unwrap();

        // No tool actually executed.
        assert!(result.tool_calls_made.is_empty());
        // The leaked pseudo-code must not reach the user.
        assert!(
            !result.response.contains("search_nodes("),
            "narrated tool call should be suppressed, not persisted: {:?}",
            result.response
        );
        // Never empty; should ask the user to confirm instead.
        assert!(!result.response.trim().is_empty());
        assert!(
            result.response.to_lowercase().contains("confirm")
                || result.response.to_lowercase().contains("would you like"),
            "should ask for confirmation: {:?}",
            result.response
        );
        // And the persisted assistant message matches the returned response.
        let last = session.messages.last().unwrap();
        assert!(!last.content.contains("search_nodes("));
        assert!(!last.content.trim().is_empty());
    }

    /// Scenario A: a tool fails, the model loops on the identical broken call
    /// (tripping the duplicate-call breaker), and the forced final inference
    /// returns empty text. The turn must still surface *something* user-visible
    /// — never a blank assistant bubble.
    #[tokio::test]
    async fn empty_final_inference_after_tool_failure_still_surfaces_response() {
        // Executor whose tool always errors.
        struct ErrorToolExecutor;

        #[async_trait]
        impl AgentToolExecutor for ErrorToolExecutor {
            async fn available_tools(&self) -> Result<Vec<ToolDefinition>, ToolError> {
                Ok(vec![ToolDefinition {
                    name: "search_nodes".into(),
                    description: "Find, list, and filter nodes".into(),
                    parameters_schema: json!({"type": "object"}),
                }])
            }

            async fn execute(
                &self,
                name: &str,
                _args: serde_json::Value,
            ) -> Result<ToolResult, ToolError> {
                Ok(ToolResult {
                    tool_call_id: "tc_1".into(),
                    name: name.into(),
                    result: json!({"error": "Invalid metadata field: status"}),
                    is_error: true,
                })
            }
        }

        // Round 1: the broken tool call. Round 2: the identical call again (the
        // duplicate-call breaker fires here, popping the assistant turn and
        // breaking to the tail final-inference). Round 3 (tail, tools stripped):
        // empty text — nothing usable from the model.
        let broken_call = || {
            vec![
                StreamingChunk::ToolCallStart {
                    id: "tc_1".to_string(),
                    name: "search_nodes".to_string(),
                    provider_extra: None,
                },
                StreamingChunk::ToolCallArgs {
                    id: "tc_1".to_string(),
                    args_json: r#"{"query":"","node_type":"task"}"#.to_string(),
                },
                StreamingChunk::Done {
                    usage: InferenceUsage {
                        prompt_tokens: 10,
                        completion_tokens: 5,
                    },
                },
            ]
        };
        let engine = Arc::new(MockEngine::new(vec![
            broken_call(),
            broken_call(),
            // Tail final inference: empty.
            vec![StreamingChunk::Done {
                usage: InferenceUsage {
                    prompt_tokens: 5,
                    completion_tokens: 0,
                },
            }],
        ]));
        let executor = Arc::new(ErrorToolExecutor);
        let agent_loop = LocalAgentLoop::new(engine, executor);

        let mut session = new_session();
        let result = agent_loop
            .run_turn(
                &mut session,
                "show my open tasks",
                |_| {},
                |_| {},
                CancellationToken::new(),
            )
            .await
            .unwrap();

        // The tool ran and failed at least once.
        assert!(!result.tool_calls_made.is_empty());
        assert!(result.tool_calls_made.iter().any(|r| r.is_error));
        // Never a blank bubble: the tool-result synthesis produces a bullet
        // summary from the (failing) execution.
        assert!(
            !result.response.trim().is_empty(),
            "empty final inference must not yield a blank response"
        );
        // And no leaked internal call syntax.
        assert!(!result.response.contains("search_nodes("));
        // The synthesized summary is honest about the failure — a turn that only
        // ever failed must not be reported as "completed".
        assert!(
            result.response.to_lowercase().contains("failed"),
            "failed-only executions should be summarized as failed: {:?}",
            result.response
        );
        assert!(!result.response.contains("completed"));
        let last = session.messages.last().unwrap();
        assert!(!last.content.trim().is_empty());
    }

    /// Guard the last-resort constant itself: the defensive nets that fire when
    /// there is genuinely nothing to synthesize (no tool executions, no usable
    /// model text) depend on it being a non-empty, honest notice.
    #[test]
    fn empty_response_fallback_is_never_blank() {
        assert!(!EMPTY_RESPONSE_FALLBACK.trim().is_empty());
        assert!(EMPTY_RESPONSE_FALLBACK.contains("try again"));
    }

    // -- Cross-turn duplicate-write guard --------------------------------

    /// Build a session that already carries one completed `create_node` write,
    /// as a rebuilt turn N+1 would after loading persisted history.
    fn session_with_prior_create(canonical: &str) -> AgentSession {
        let mut session = new_session();
        session.prior_writes = vec![PriorWrite {
            tool: "create_node".to_string(),
            canonical_args: canonical.to_string(),
            node_id: Some("nodespace://n1".to_string()),
            summary: Some("Buy milk".to_string()),
        }];
        session
    }

    fn create_node_executor() -> MockToolExecutor {
        MockToolExecutor::new().with_tool(
            "create_node",
            json!({"type": "object", "properties": {"content": {"type": "string"}}}),
            json!({"id": "nodespace://n2", "created": true}),
        )
    }

    /// The acceptance criterion: a repeated write with identical canonical args
    /// in a later turn must not execute a second time.
    #[tokio::test]
    async fn cross_turn_duplicate_write_is_not_executed() {
        let args = r#"{"content":"Buy milk"}"#;
        let engine = Arc::new(MockEngine::tool_then_text(
            "create_node",
            args,
            "It already exists.",
        ));
        let executor = Arc::new(RecordingToolExecutor::new(create_node_executor()));
        let calls = executor.calls_handle();
        let agent_loop = LocalAgentLoop::new(engine, executor);

        let mut session = session_with_prior_create(&canonical_args(args));
        let result = agent_loop
            .run_turn(
                &mut session,
                "add a task to buy milk",
                |_| {},
                |_| {},
                CancellationToken::new(),
            )
            .await
            .expect("turn should succeed");

        assert!(
            !calls.lock().unwrap().iter().any(|c| c == "create_node"),
            "the duplicate create must never reach the executor, got {:?}",
            calls.lock().unwrap()
        );

        // The model still receives a result, and it is not an error.
        let rec = result
            .tool_calls_made
            .iter()
            .find(|r| r.name == "create_node")
            .expect("a tool result must still be produced");
        assert!(!rec.is_error, "a refused duplicate is not a failure");
    }

    /// The result must name the already-written node, so the model can tell the
    /// user the thing exists instead of reporting an opaque refusal.
    #[tokio::test]
    async fn refused_duplicate_names_the_existing_node() {
        let args = r#"{"content":"Buy milk"}"#;
        let engine = Arc::new(MockEngine::tool_then_text("create_node", args, "Exists."));
        let executor = Arc::new(RecordingToolExecutor::new(create_node_executor()));
        let agent_loop = LocalAgentLoop::new(engine, executor);

        let mut session = session_with_prior_create(&canonical_args(args));
        let result = agent_loop
            .run_turn(
                &mut session,
                "add it",
                |_| {},
                |_| {},
                CancellationToken::new(),
            )
            .await
            .expect("turn should succeed");

        let rec = result
            .tool_calls_made
            .iter()
            .find(|r| r.name == "create_node")
            .expect("tool result");
        let rendered = rec.result.to_string();
        assert!(
            rendered.contains("nodespace://n1"),
            "must name the existing node, got {rendered}"
        );
        assert!(
            rendered.contains("Buy milk"),
            "must describe what already exists, got {rendered}"
        );
    }

    /// Key order must not defeat the guard: the same call re-emitted with
    /// reordered keys is the same write.
    #[tokio::test]
    async fn guard_matches_regardless_of_argument_key_order() {
        let engine = Arc::new(MockEngine::tool_then_text(
            "create_node",
            r#"{"node_type":"task","content":"Buy milk"}"#,
            "Exists.",
        ));
        let executor = Arc::new(RecordingToolExecutor::new(create_node_executor()));
        let calls = executor.calls_handle();
        let agent_loop = LocalAgentLoop::new(engine, executor);

        let mut session = session_with_prior_create(&canonical_args(
            r#"{"content":"Buy milk","node_type":"task"}"#,
        ));
        agent_loop
            .run_turn(
                &mut session,
                "add it",
                |_| {},
                |_| {},
                CancellationToken::new(),
            )
            .await
            .expect("turn should succeed");

        assert!(
            !calls.lock().unwrap().iter().any(|c| c == "create_node"),
            "reordered keys are the same call and must still be refused"
        );
    }

    /// A genuinely different write must still go through. The guard keys on the
    /// arguments, not merely the tool name.
    #[tokio::test]
    async fn a_different_write_still_executes() {
        let engine = Arc::new(MockEngine::tool_then_text(
            "create_node",
            r#"{"content":"Buy bread"}"#,
            "Added.",
        ));
        let executor = Arc::new(RecordingToolExecutor::new(create_node_executor()));
        let calls = executor.calls_handle();
        let agent_loop = LocalAgentLoop::new(engine, executor);

        let mut session = session_with_prior_create(&canonical_args(r#"{"content":"Buy milk"}"#));
        agent_loop
            .run_turn(
                &mut session,
                "add bread",
                |_| {},
                |_| {},
                CancellationToken::new(),
            )
            .await
            .expect("turn should succeed");

        assert!(
            calls.lock().unwrap().iter().any(|c| c == "create_node"),
            "a distinct create must not be blocked"
        );
    }

    // -- Second create_schema-in-one-turn guard ------------------

    fn schema_executor() -> MockToolExecutor {
        MockToolExecutor::new().with_tool(
            "create_schema",
            json!({"type": "object", "properties": {"name": {"type": "string"}}}),
            json!({"schemaId": "invoice", "isCore": false, "version": 1, "fields": []}),
        )
    }

    /// The acceptance criterion: a second `create_schema` call within the same
    /// skill invocation must not reach the executor, even though nothing in
    /// the tool whitelist or max_iterations mechanically prevents the model
    /// from attempting it.
    #[tokio::test]
    async fn second_create_schema_in_one_turn_is_not_executed() {
        let engine = Arc::new(MockEngine::new(vec![
            // Round 1: create_schema for "Invoice"
            vec![
                StreamingChunk::ToolCallStart {
                    id: "tc_1".to_string(),
                    name: "create_schema".to_string(),
                    provider_extra: None,
                },
                StreamingChunk::ToolCallArgs {
                    id: "tc_1".to_string(),
                    args_json: r#"{"name":"Invoice","fields":[]}"#.to_string(),
                },
                StreamingChunk::Done {
                    usage: InferenceUsage {
                        prompt_tokens: 10,
                        completion_tokens: 5,
                    },
                },
            ],
            // Round 2: model tries a second, different create_schema call —
            // e.g. splitting "track invoices and customers" into two types.
            vec![
                StreamingChunk::ToolCallStart {
                    id: "tc_2".to_string(),
                    name: "create_schema".to_string(),
                    provider_extra: None,
                },
                StreamingChunk::ToolCallArgs {
                    id: "tc_2".to_string(),
                    args_json: r#"{"name":"Customer","fields":[]}"#.to_string(),
                },
                StreamingChunk::Done {
                    usage: InferenceUsage {
                        prompt_tokens: 12,
                        completion_tokens: 5,
                    },
                },
            ],
            // Round 3: final summary
            vec![
                StreamingChunk::Token {
                    text: "Created the Invoice type.".to_string(),
                },
                StreamingChunk::Done {
                    usage: InferenceUsage {
                        prompt_tokens: 15,
                        completion_tokens: 6,
                    },
                },
            ],
        ]));

        let executor = Arc::new(RecordingToolExecutor::new(schema_executor()));
        let calls = executor.calls_handle();
        let agent_loop = LocalAgentLoop::new(engine, executor);

        let mut session = new_session();
        let result = agent_loop
            .run_turn(
                &mut session,
                "keep track of invoices and customers",
                |_| {},
                |_| {},
                CancellationToken::new(),
            )
            .await
            .expect("turn should succeed");

        assert_eq!(
            calls
                .lock()
                .unwrap()
                .iter()
                .filter(|c| *c == "create_schema")
                .count(),
            1,
            "only the first create_schema call should reach the executor, got {:?}",
            calls.lock().unwrap()
        );

        let refused = result
            .tool_calls_made
            .iter()
            .filter(|r| r.name == "create_schema")
            .nth(1)
            .expect("the second create_schema call must still produce a tool result");
        assert!(
            refused.is_error,
            "a refused second schema creation is a policy violation, not a benign no-op"
        );
    }

    /// A single `create_schema` call in a turn must execute normally — the
    /// guard only fires on a *second* call after a successful first one.
    #[tokio::test]
    async fn single_create_schema_in_one_turn_still_executes() {
        let engine = Arc::new(MockEngine::tool_then_text(
            "create_schema",
            r#"{"name":"Invoice","fields":[]}"#,
            "Created the Invoice type.",
        ));
        let executor = Arc::new(RecordingToolExecutor::new(schema_executor()));
        let calls = executor.calls_handle();
        let agent_loop = LocalAgentLoop::new(engine, executor);

        let mut session = new_session();
        agent_loop
            .run_turn(
                &mut session,
                "keep track of invoices",
                |_| {},
                |_| {},
                CancellationToken::new(),
            )
            .await
            .expect("turn should succeed");

        assert!(
            calls.lock().unwrap().iter().any(|c| c == "create_schema"),
            "a single create_schema call must not be blocked"
        );
    }

    /// A `create_schema` call that *fails* (e.g. validation error) must not
    /// count against the guard — the model needs to be able to retry with a
    /// corrected payload per SCHEMA_VALIDATION_ERROR_RETRY.
    #[tokio::test]
    async fn failed_create_schema_does_not_block_a_retry() {
        let engine = Arc::new(MockEngine::new(vec![
            vec![
                StreamingChunk::ToolCallStart {
                    id: "tc_1".to_string(),
                    name: "create_schema".to_string(),
                    provider_extra: None,
                },
                StreamingChunk::ToolCallArgs {
                    id: "tc_1".to_string(),
                    args_json: r#"{"name":"Invoice"}"#.to_string(),
                },
                StreamingChunk::Done {
                    usage: InferenceUsage {
                        prompt_tokens: 10,
                        completion_tokens: 5,
                    },
                },
            ],
            vec![
                StreamingChunk::ToolCallStart {
                    id: "tc_2".to_string(),
                    name: "create_schema".to_string(),
                    provider_extra: None,
                },
                StreamingChunk::ToolCallArgs {
                    id: "tc_2".to_string(),
                    args_json: r#"{"name":"Invoice","fields":[]}"#.to_string(),
                },
                StreamingChunk::Done {
                    usage: InferenceUsage {
                        prompt_tokens: 12,
                        completion_tokens: 5,
                    },
                },
            ],
            vec![
                StreamingChunk::Token {
                    text: "Created the Invoice type.".to_string(),
                },
                StreamingChunk::Done {
                    usage: InferenceUsage {
                        prompt_tokens: 15,
                        completion_tokens: 6,
                    },
                },
            ],
        ]));

        // First call errors (missing required "fields"); second call succeeds.
        struct FirstFailsThenSucceeds {
            calls: Arc<std::sync::Mutex<Vec<String>>>,
        }
        #[async_trait]
        impl AgentToolExecutor for FirstFailsThenSucceeds {
            async fn available_tools(&self) -> Result<Vec<ToolDefinition>, ToolError> {
                Ok(vec![ToolDefinition {
                    name: "create_schema".into(),
                    description: "Mock tool".into(),
                    parameters_schema: json!({"type": "object"}),
                }])
            }

            async fn execute(
                &self,
                name: &str,
                args: serde_json::Value,
            ) -> Result<ToolResult, ToolError> {
                self.calls.lock().unwrap().push(name.to_string());
                let has_fields = args.get("fields").is_some();
                Ok(ToolResult {
                    tool_call_id: "tc".to_string(),
                    name: name.to_string(),
                    result: if has_fields {
                        json!({"schemaId": "invoice", "isCore": false, "version": 1, "fields": []})
                    } else {
                        json!({"error": "\"fields\" is required"})
                    },
                    is_error: !has_fields,
                })
            }
        }

        let calls = Arc::new(std::sync::Mutex::new(Vec::new()));
        let executor = Arc::new(FirstFailsThenSucceeds {
            calls: Arc::clone(&calls),
        });
        let agent_loop = LocalAgentLoop::new(engine, executor);

        let mut session = new_session();
        let result = agent_loop
            .run_turn(
                &mut session,
                "keep track of invoices",
                |_| {},
                |_| {},
                CancellationToken::new(),
            )
            .await
            .expect("turn should succeed");

        assert_eq!(
            calls.lock().unwrap().len(),
            2,
            "both attempts must reach the executor — the first failed, so it doesn't count \
             as \"a schema already created this turn\": {:?}",
            calls.lock().unwrap()
        );
        let second = result
            .tool_calls_made
            .iter()
            .filter(|r| r.name == "create_schema")
            .nth(1)
            .expect("second attempt must produce a tool result");
        assert!(
            !second.is_error,
            "the corrected retry should succeed, not be refused as a duplicate schema"
        );
    }

    /// Build a session carrying one completed markdown import, stored the way
    /// the record side stores it — through `canonical_args_identity`, so an
    /// oversized import is held as a digest.
    fn session_with_prior_import(markdown: &str) -> AgentSession {
        let mut session = new_session();
        session.prior_writes = vec![PriorWrite {
            tool: "create_nodes_from_markdown".to_string(),
            canonical_args: canonical_args_identity(&canonical_args(
                &json!({ "markdown": markdown }).to_string(),
            )),
            node_id: Some("nodespace://n1".to_string()),
            summary: Some("Project plan".to_string()),
        }];
        session
    }

    fn import_executor() -> MockToolExecutor {
        MockToolExecutor::new().with_tool(
            "create_nodes_from_markdown",
            json!({"type": "object", "properties": {"markdown": {"type": "string"}}}),
            json!({"created": 12}),
        )
    }

    /// An import large enough to exceed the args cap — the realistic case, since
    /// any real import does.
    fn oversized_markdown(topic: &str) -> String {
        format!(
            "# {topic}\n{}",
            "- a task line\n".repeat(CANONICAL_ARGS_MAX_CHARS / 8)
        )
    }

    /// The acceptance criterion for the oversized-args gap: a repeated import
    /// must be refused across turns. This is the tool where a repeat is worst —
    /// it duplicates an entire subtree — and the one the cap used to exempt
    /// wholesale, leaving it unguarded in every real case.
    #[tokio::test]
    async fn cross_turn_duplicate_oversized_import_is_not_executed() {
        let markdown = oversized_markdown("Project plan");
        assert!(
            markdown.chars().count() > CANONICAL_ARGS_MAX_CHARS,
            "the fixture must actually exceed the cap"
        );
        let args = json!({ "markdown": markdown }).to_string();

        let engine = Arc::new(MockEngine::tool_then_text(
            "create_nodes_from_markdown",
            &args,
            "It already exists.",
        ));
        let executor = Arc::new(RecordingToolExecutor::new(import_executor()));
        let calls = executor.calls_handle();
        let agent_loop = LocalAgentLoop::new(engine, executor);

        let mut session = session_with_prior_import(&markdown);
        agent_loop
            .run_turn(
                &mut session,
                "import my project plan",
                |_| {},
                |_| {},
                CancellationToken::new(),
            )
            .await
            .expect("turn should succeed");

        assert!(
            !calls
                .lock()
                .unwrap()
                .iter()
                .any(|c| c == "create_nodes_from_markdown"),
            "a repeated import must never reach the executor, got {:?}",
            calls.lock().unwrap()
        );
    }

    /// The negative control for the digest. Refusing a *different* import would
    /// be a wrong suppression — worse than the missed duplicate this change
    /// fixes — so the identity must distinguish two large calls, not merely
    /// recognise that both are large.
    #[tokio::test]
    async fn a_different_oversized_import_still_executes() {
        let engine = Arc::new(MockEngine::tool_then_text(
            "create_nodes_from_markdown",
            &json!({ "markdown": oversized_markdown("Recipes") }).to_string(),
            "Imported.",
        ));
        let executor = Arc::new(RecordingToolExecutor::new(import_executor()));
        let calls = executor.calls_handle();
        let agent_loop = LocalAgentLoop::new(engine, executor);

        let mut session = session_with_prior_import(&oversized_markdown("Project plan"));
        agent_loop
            .run_turn(
                &mut session,
                "import my recipes",
                |_| {},
                |_| {},
                CancellationToken::new(),
            )
            .await
            .expect("turn should succeed");

        assert!(
            calls
                .lock()
                .unwrap()
                .iter()
                .any(|c| c == "create_nodes_from_markdown"),
            "a distinct import must not be blocked"
        );
    }

    fn delete_node_executor() -> MockToolExecutor {
        MockToolExecutor::new().with_tool(
            "delete_node",
            json!({"type": "object", "properties": {"id": {"type": "string"}}}),
            json!({"deleted": true}),
        )
    }

    /// Build a session carrying one completed `delete_node`, recorded under the
    /// given key spelling.
    fn session_with_prior_delete(args: &str) -> AgentSession {
        let mut session = new_session();
        session.prior_writes = vec![PriorWrite {
            tool: "delete_node".to_string(),
            canonical_args: canonical_args_identity(&canonical_args(args)),
            node_id: Some("nodespace://n1".to_string()),
            summary: Some("Old note".to_string()),
        }];
        session
    }

    /// Run a turn where the model re-issues a delete under `incoming`, against a
    /// prior write recorded under `stored`. Returns whether the executor ran.
    async fn delete_repeat_reaches_executor(stored: &str, incoming: &str) -> bool {
        let engine = Arc::new(MockEngine::tool_then_text(
            "delete_node",
            incoming,
            "Already gone.",
        ));
        let executor = Arc::new(RecordingToolExecutor::new(delete_node_executor()));
        let calls = executor.calls_handle();
        let agent_loop = LocalAgentLoop::new(engine, executor);

        let mut session = session_with_prior_delete(stored);
        agent_loop
            .run_turn(
                &mut session,
                "delete that node",
                |_| {},
                |_| {},
                CancellationToken::new(),
            )
            .await
            .expect("turn should succeed");

        // Bound, not returned directly: the temporary `MutexGuard` in the
        // expression would outlive `calls` at the end of the function.
        let ran = calls.lock().unwrap().iter().any(|c| c == "delete_node");
        ran
    }

    /// The acceptance criterion for the alias gap. `delete_node` takes its target
    /// under either `id` or `node_id` (serde alias), but serde resolves that only
    /// when deserialising into the params struct — after canonicalisation. So a
    /// repeat that switched spelling used to produce a second identity and slip
    /// the guard. Both directions, since either spelling can be recorded first.
    #[tokio::test]
    async fn delete_repeat_is_refused_across_the_node_id_alias() {
        let id_form = r#"{"id":"nodespace://n1"}"#;
        let alias_form = r#"{"node_id":"nodespace://n1"}"#;

        assert!(
            !delete_repeat_reaches_executor(id_form, alias_form).await,
            "a repeat spelled `node_id` must be recognised as the stored `id` call"
        );
        assert!(
            !delete_repeat_reaches_executor(alias_form, id_form).await,
            "a repeat spelled `id` must be recognised as the stored `node_id` call"
        );
    }

    /// The alias rule must not merge genuinely different deletes: it renames a
    /// key, it does not ignore the value.
    #[tokio::test]
    async fn delete_of_a_different_node_still_executes() {
        assert!(
            delete_repeat_reaches_executor(
                r#"{"id":"nodespace://n1"}"#,
                r#"{"node_id":"nodespace://n2"}"#
            )
            .await,
            "a delete targeting another node must not be blocked"
        );
    }

    /// The alias is resolved at the top level only. A nested `field_values` blob
    /// is the user's own data, where a field named `node_id` means whatever their
    /// schema says — renaming it would silently rewrite user data into a
    /// different field for identity purposes.
    #[test]
    fn alias_normalisation_does_not_reach_into_nested_user_data() {
        // A realistic `update_node`: the tool's own target under the alias, plus
        // a user-defined field that happens to be named `node_id` inside the
        // `field_values` blob. The top-level key is renamed; the user's field is
        // not, and its value must survive untouched.
        let canonical = canonical_args(
            r#"{"node_id":"nodespace://n1","field_values":{"node_id":"user-value","capacity":10}}"#,
        );
        assert!(
            canonical.contains(r#""id":"nodespace://n1""#),
            "the top-level alias must be resolved, got {canonical}"
        );
        assert!(
            canonical.contains(r#""node_id":"user-value""#),
            "a nested user field must be left alone, got {canonical}"
        );
    }

    /// When a call carries both spellings there is no safe rename: serde's own
    /// precedence decides which wins, and guessing here could make two genuinely
    /// different calls compare equal — the one failure this guard must not have.
    #[test]
    fn alias_normalisation_leaves_ambiguous_calls_untouched() {
        let canonical = canonical_args(r#"{"id":"a","node_id":"b"}"#);
        assert!(canonical.contains(r#""id":"a""#), "got {canonical}");
        assert!(canonical.contains(r#""node_id":"b""#), "got {canonical}");
    }

    /// Under the cap the identity is the canonical args verbatim, so a stored
    /// identity stays readable when diagnosing a refusal.
    #[test]
    fn identity_is_verbatim_below_the_cap() {
        let canonical = canonical_args(r#"{"content":"Buy milk"}"#);
        assert_eq!(canonical_args_identity(&canonical), canonical);
    }

    /// The two identity forms must be mutually unambiguous: canonical JSON always
    /// starts with `{`, a digest with `sha256:`. Were they confusable, a call
    /// whose arguments happened to spell a digest could impersonate another
    /// call's identity.
    #[test]
    fn digest_and_verbatim_identities_cannot_be_confused() {
        let small = canonical_args_identity(&canonical_args(r#"{"content":"x"}"#));
        let large = canonical_args_identity(&"x".repeat(CANONICAL_ARGS_MAX_CHARS + 1));
        assert!(small.starts_with('{'));
        assert!(large.starts_with("sha256:"));
        assert_ne!(small, large);
    }

    /// The digest must be a function of the content: equal args hash equal,
    /// different args do not. Exact-match equality is the whole guarantee the
    /// guard needs, and hashing has to preserve it in both directions.
    #[test]
    fn digest_identity_is_stable_and_content_sensitive() {
        let a = "a".repeat(CANONICAL_ARGS_MAX_CHARS + 1);
        let b = format!("{}b", "a".repeat(CANONICAL_ARGS_MAX_CHARS));
        assert_eq!(canonical_args_identity(&a), canonical_args_identity(&a));
        assert_ne!(canonical_args_identity(&a), canonical_args_identity(&b));
    }

    /// Idempotent repeats must survive the guard. Setting the same task status
    /// twice is a no-op, not a duplicate, and blocking it would break a user
    /// legitimately re-asserting a value.
    #[tokio::test]
    async fn idempotent_update_repeat_is_not_blocked() {
        let args = r#"{"id":"nodespace://t1","status":"done"}"#;
        let engine = Arc::new(MockEngine::tool_then_text(
            "update_task_status",
            args,
            "Marked done.",
        ));
        let executor = Arc::new(RecordingToolExecutor::new(
            MockToolExecutor::new().with_tool(
                "update_task_status",
                json!({"type": "object", "properties": {"status": {"type": "string"}}}),
                json!({"id": "nodespace://t1", "status": "done"}),
            ),
        ));
        let calls = executor.calls_handle();
        let agent_loop = LocalAgentLoop::new(engine, executor);

        let mut session = new_session();
        // Even with an exact-match record present, an update must execute.
        session.prior_writes = vec![PriorWrite {
            tool: "update_task_status".to_string(),
            canonical_args: canonical_args(args),
            node_id: Some("nodespace://t1".to_string()),
            summary: Some("t1".to_string()),
        }];

        agent_loop
            .run_turn(
                &mut session,
                "mark it done",
                |_| {},
                |_| {},
                CancellationToken::new(),
            )
            .await
            .expect("turn should succeed");

        assert!(
            calls
                .lock()
                .unwrap()
                .iter()
                .any(|c| c == "update_task_status"),
            "idempotent updates must not be blocked by the cross-turn guard"
        );
    }

    /// A session with no prior writes — every caller that does not persist
    /// history — must behave exactly as before.
    #[tokio::test]
    async fn no_prior_writes_leaves_execution_untouched() {
        let engine = Arc::new(MockEngine::tool_then_text(
            "create_node",
            r#"{"content":"Buy milk"}"#,
            "Added.",
        ));
        let executor = Arc::new(RecordingToolExecutor::new(create_node_executor()));
        let calls = executor.calls_handle();
        let agent_loop = LocalAgentLoop::new(engine, executor);

        let mut session = new_session();
        agent_loop
            .run_turn(
                &mut session,
                "add milk",
                |_| {},
                |_| {},
                CancellationToken::new(),
            )
            .await
            .expect("turn should succeed");

        assert!(calls.lock().unwrap().iter().any(|c| c == "create_node"));
    }

    /// The reported malformation: keys the model wrapped in literal quote
    /// characters. Repairing them is what lets a rejected call's retry succeed —
    /// without it the model reads its own malformed call back out of the
    /// conversation and reproduces it verbatim on every attempt.
    #[test]
    fn over_quoted_keys_are_repaired_at_every_depth() {
        let mut v = serde_json::json!({
            "name": "Venue",
            "fields": [
                {"\"name\"": "capacity", "\"type\"": "number"},
                {"\"name\"": "address", "type": "text"}
            ]
        });
        repair_over_quoted_keys(&mut v);
        assert_eq!(
            v,
            serde_json::json!({
                "name": "Venue",
                "fields": [
                    {"name": "capacity", "type": "number"},
                    {"name": "address", "type": "text"}
                ]
            })
        );
    }

    /// A key that merely *contains* quotes, or whose repair would collide with a
    /// real key already present, is not the mechanical malformation this repairs
    /// — rewriting either would destroy caller data rather than restore it.
    #[test]
    fn repair_leaves_ambiguous_and_colliding_keys_untouched() {
        let mut colliding = serde_json::json!({"\"name\"": "quoted", "name": "real"});
        repair_over_quoted_keys(&mut colliding);
        assert_eq!(
            colliding,
            serde_json::json!({"\"name\"": "quoted", "name": "real"}),
            "a repair that collides with an existing key must discard neither value"
        );

        let mut inner_quote = serde_json::json!({"\"a\"b\"": 1, "\"\"": 2, "plain": 3});
        repair_over_quoted_keys(&mut inner_quote);
        assert_eq!(
            inner_quote,
            serde_json::json!({"\"a\"b\"": 1, "\"\"": 2, "plain": 3}),
            "only a cleanly quote-wrapped key is an unambiguous repair"
        );
    }

    /// User data lives in these payloads. A string *value* that happens to carry
    /// quote characters means whatever the user wrote and must survive untouched
    /// — only keys are ever rewritten.
    #[test]
    fn repair_never_rewrites_values() {
        let mut v = serde_json::json!({
            "properties": {"quote": "she said \"hello\"", "\"title\"": "Report"}
        });
        repair_over_quoted_keys(&mut v);
        assert_eq!(
            v,
            serde_json::json!({
                "properties": {"quote": "she said \"hello\"", "title": "Report"}
            })
        );
    }

    /// The repaired form is what the duplicate guards compare on, so a malformed
    /// call and its repaired retry resolve to one identity rather than two.
    #[test]
    fn canonical_args_repairs_over_quoted_keys() {
        let malformed = r#"{"fields":[{"\"name\"":"capacity","\"type\"":"number"}]}"#;
        let clean = r#"{"fields":[{"name":"capacity","type":"number"}]}"#;
        assert_eq!(canonical_args(malformed), canonical_args(clean));
    }

    /// #1943: the verbatim payload from the issue's 3-of-3 reproduction, run
    /// through the one entry point the loop actually calls. Both malformations
    /// are present at once — over-quoted keys at two nesting levels, and a
    /// `type` value that swallowed the `value` key's delimiters.
    #[test]
    fn reported_malformed_search_filter_is_repaired() {
        let mut args = r#"{"filters":[{"\"operator\"":"equals","\"property\"":"status","\"type\"":"task\",\"value\":"}],"\"node_type\"":"task","query":null}"#.to_string();
        repair_tool_call_arguments(&mut args);

        let repaired: serde_json::Value =
            serde_json::from_str(&args).expect("repaired args must still be valid JSON");
        assert_eq!(
            repaired,
            serde_json::json!({
                "filters": [{"operator": "equals", "property": "status", "type": "task"}],
                "node_type": "task",
                "query": null
            }),
            "the reported payload must reach the tool with usable keys and a \
             truncated-but-clean `type`; got {repaired}"
        );
    }

    /// The swallowed key's value was never emitted, so the repair must not
    /// invent one. Recovering `type` while leaving `value` absent is the honest
    /// outcome — the filter still fails, but it fails naming the field that is
    /// genuinely missing rather than reporting a nonsense filter type.
    #[test]
    fn spliced_repair_does_not_invent_the_swallowed_key() {
        let mut v = serde_json::json!({"type": "task\",\"value\":"});
        repair_spliced_object_values(&mut v);
        assert_eq!(v, serde_json::json!({"type": "task"}));
        assert!(
            v.get("value").is_none(),
            "a value the model never emitted must not be fabricated"
        );
    }

    /// User data lives in these payloads. Only the exact mechanical splice is
    /// claimed — a string that merely contains quotes, commas or colons means
    /// whatever the user wrote and must survive untouched.
    #[test]
    fn spliced_repair_leaves_ordinary_strings_untouched() {
        let original = serde_json::json!({
            "quote": "she said \"hello\"",
            "csv": "a\",\"b",
            "colon": "note: see \"ref\":",
            "ends_with_delims": "plain\":",
            "empty_key": "task\",\"\":",
            "plain": "task",
        });
        let mut v = original.clone();
        repair_spliced_object_values(&mut v);
        assert_eq!(
            v, original,
            "only a value whose entire tail is the `\",\"<name>\":` delimiter run is repairable"
        );
    }

    /// The repair rewrites *values*, so it must never reach the user's own
    /// authored text. A pasted CSV header fragment matches the mechanical splice
    /// shape exactly, and truncating it would silently corrupt stored data —
    /// so `content` and `field_values` are skipped at any depth.
    #[test]
    fn spliced_repair_never_touches_user_authored_content() {
        let original = serde_json::json!({
            "id": "node-1",
            "content": "name\",\"email\":",
            "field_values": {
                "notes": "Reviewed the paper\",\"Notes\":",
                "nested": {"deep": "a\",\"b\":"}
            }
        });
        let mut v = original.clone();
        repair_spliced_object_values(&mut v);
        assert_eq!(
            v, original,
            "user-authored content and field values must survive repair verbatim"
        );

        // But the model's structural slots are still repaired.
        let mut structural = serde_json::json!({
            "filters": [{"type": "task\",\"value\":"}]
        });
        repair_spliced_object_values(&mut structural);
        assert_eq!(
            structural,
            serde_json::json!({"filters": [{"type": "task"}]})
        );
    }

    /// #2182: verbatim shape from the golden-runner trace behind the issue —
    /// `in` given a comma-joined string where the array it requires was meant.
    /// Reaches the tool as `["cut","soak"]` through the same entry point
    /// production uses, not just through the repair called directly.
    #[test]
    fn scalar_in_operator_value_is_split_into_an_array() {
        let mut args = r#"{"filters":[{"operator":"in","property":"stage","value":"cut,soak"}],"node_type":"release"}"#.to_string();
        repair_tool_call_arguments(&mut args);
        let repaired: serde_json::Value =
            serde_json::from_str(&args).expect("repaired args must still be valid JSON");
        assert_eq!(
            repaired,
            serde_json::json!({
                "filters": [{
                    "operator": "in",
                    "property": "stage",
                    "value": ["cut", "soak"]
                }],
                "node_type": "release"
            }),
            "the `in` filter must reach the tool with the array QueryService \
             requires; got {repaired}"
        );
    }

    /// The separator the model adds is punctuation, not part of a stored value.
    /// A single member is wrapped rather than left alone because `IN (x)` and
    /// `= x` select the same rows, so the wrap is safe whichever operator the
    /// model meant — where leaving it a bare string keeps failing for the reason
    /// this exists.
    #[test]
    fn in_operator_values_are_trimmed_and_single_values_are_wrapped() {
        // Wrapped in `filters` because that is the only slot the repair walks —
        // a bare filter-shaped object is deliberately out of range, and the test
        // has to exercise the real entry point rather than a shape that would
        // only work if the scoping were absent.
        let repaired_value = |raw: serde_json::Value| {
            let mut args = serde_json::json!({
                "filters": [{"operator": "in", "property": "stage", "value": raw}]
            });
            repair_scalar_in_operator_values(&mut args);
            args["filters"][0]["value"].clone()
        };

        assert_eq!(
            repaired_value(serde_json::json!("cut, soak , shipped")),
            serde_json::json!(["cut", "soak", "shipped"])
        );

        assert_eq!(
            repaired_value(serde_json::json!("soak")),
            serde_json::json!(["soak"])
        );

        // A trailing comma is a typo, not a filter on the empty string.
        assert_eq!(
            repaired_value(serde_json::json!("cut,soak,")),
            serde_json::json!(["cut", "soak"])
        );
    }

    /// Within `filters`, only a scalar `in` value is in range: another
    /// operator's value keeps its scalar shape, an `in` that already sent an
    /// array is untouched, a non-string value is left alone, and a value with no
    /// members left after splitting stays exactly as sent rather than becoming
    /// an empty `IN ()` that matches nothing while looking like a working
    /// filter. The `content` and `field_values` entries are covered by their own
    /// test — here they only confirm siblings of `filters` are not walked.
    #[test]
    fn in_operator_repair_leaves_everything_else_untouched() {
        let original = serde_json::json!({
            "filters": [
                {"operator": "equals", "property": "stage", "value": "cut,soak"},
                {"operator": "contains", "property": "owner", "value": "sam, dana"},
                {"operator": "in", "property": "stage", "value": ["cut", "soak"]},
                {"operator": "in", "property": "stage", "value": ""},
                {"operator": "in", "property": "stage", "value": " , "},
                {"operator": "in", "property": "count", "value": 3},
            ],
            "content": "cut, soak",
            "field_values": {"notes": "cut,soak"}
        });
        let mut v = original.clone();
        repair_scalar_in_operator_values(&mut v);
        assert_eq!(
            v, original,
            "only a scalar string under an `in` operator, carrying at least one \
             member, is this repair's to rewrite"
        );
    }

    /// The repair rewrites *values*, so it must never reach the user's own data.
    ///
    /// `field_values` carries the fields of a user-DEFINED schema and
    /// `create_schema` reserves no field names, so a type with fields called
    /// `operator` and `value` produces exactly the shape a filter item has. A
    /// repair keyed on that shape alone would silently rewrite a stored company
    /// name into two members — inside a write tool, and with no error anywhere.
    /// Scoping the entry point to the `filters` slot is what makes it safe, and
    /// this pins that: the previous version of this test looked like it covered
    /// the case but its entries carried no `operator` key, so it asserted a
    /// property the code did not have.
    #[test]
    fn in_operator_repair_never_touches_user_authored_field_values() {
        let original = serde_json::json!({
            "id": "node-1",
            "field_values": {
                "operator": "in",
                "value": "Acme Corp, Ltd",
                "nested": {"operator": "in", "value": "a,b"}
            },
            "content": "cut,soak"
        });
        let mut v = original.clone();
        repair_scalar_in_operator_values(&mut v);
        assert_eq!(
            v, original,
            "a user-defined field named `operator` must not turn their stored \
             value into a list"
        );

        // A top-level object shaped like a filter item is likewise out of range:
        // only the `filters` parameter is this repair's to walk.
        let mut bare = serde_json::json!({"operator": "in", "value": "a,b"});
        let bare_before = bare.clone();
        repair_scalar_in_operator_values(&mut bare);
        assert_eq!(bare, bare_before);

        // And the slot that IS in range still works, so the scoping did not
        // silently disable the repair.
        let mut scoped = serde_json::json!({
            "filters": [{"operator": "in", "property": "stage", "value": "cut,soak"}]
        });
        repair_scalar_in_operator_values(&mut scoped);
        assert_eq!(
            scoped["filters"][0]["value"],
            serde_json::json!(["cut", "soak"])
        );
    }

    /// The identity and the execution path must apply the same repair set, or
    /// the guards read a malformed call and its repaired retry as two different
    /// calls and the loop they exist to break still runs.
    #[test]
    fn canonical_args_applies_the_in_operator_repair() {
        let malformed = r#"{"filters":[{"operator":"in","property":"stage","value":"cut,soak"}]}"#;
        let repaired =
            r#"{"filters":[{"operator":"in","property":"stage","value":["cut","soak"]}]}"#;
        assert_eq!(
            canonical_args(malformed),
            canonical_args(repaired),
            "a malformed call and its repaired retry must share one identity"
        );
    }

    /// A well-formed call must come back byte-identical, not silently
    /// re-serialised into serde's key order — the loop-breaker compares raw
    /// emitted text elsewhere, and a gratuitous rewrite would churn it.
    #[test]
    fn repair_entry_point_leaves_clean_arguments_byte_identical() {
        let clean = r#"{"query":"tasks","node_type":"task","filters":[]}"#;
        let mut args = clean.to_string();
        repair_tool_call_arguments(&mut args);
        assert_eq!(args, clean);
    }

    /// Unparseable arguments cannot be repaired structurally and must reach the
    /// parse-failure path unchanged, which reports the real malformation rather
    /// than something this function invented on the way past.
    #[test]
    fn repair_entry_point_leaves_unparseable_arguments_unchanged() {
        let broken = r#"{"query": "unterminated"#;
        let mut args = broken.to_string();
        repair_tool_call_arguments(&mut args);
        assert_eq!(args, broken);
    }

    /// #1926: verbatim shape from the daemon.log trace behind the issue — a
    /// leaked Gemma quote token corrupting `create_schema`'s `fields[0].type`
    /// key.
    #[test]
    fn leaked_special_token_keys_are_repaired_at_every_depth() {
        let mut v = serde_json::json!({
            "name": "Venue Booking Tracker",
            "fields": [
                {"<|\"|>type<|\"|>": "number", "name": "replacement_cost"},
                {"<|\"|>type<|\"|>": "date", "name": "booking_date"}
            ]
        });
        repair_leaked_special_token_keys(&mut v);
        assert_eq!(
            v,
            serde_json::json!({
                "name": "Venue Booking Tracker",
                "fields": [
                    {"type": "number", "name": "replacement_cost"},
                    {"type": "date", "name": "booking_date"}
                ]
            })
        );
    }

    /// A repair that would collide with an existing key, or leave nothing
    /// behind, is not the mechanical malformation this repairs — same guard
    /// shape as `repair_over_quoted_keys`'s own collision case.
    #[test]
    fn leaked_special_token_repair_leaves_colliding_and_empty_keys_untouched() {
        let mut colliding = serde_json::json!({"<|\"|>type<|\"|>": "number", "type": "text"});
        repair_leaked_special_token_keys(&mut colliding);
        assert_eq!(
            colliding,
            serde_json::json!({"<|\"|>type<|\"|>": "number", "type": "text"}),
            "a repair that collides with an existing key must discard neither value"
        );

        let mut empty = serde_json::json!({"<|\"|>": 1, "plain": 2});
        repair_leaked_special_token_keys(&mut empty);
        assert_eq!(
            empty,
            serde_json::json!({"<|\"|>": 1, "plain": 2}),
            "a repair that would leave an empty key name is not unambiguous"
        );
    }

    /// Two distinct corrupted keys that repair to the same clean name within
    /// one object: the collision check runs against the live, mutating
    /// object on each loop iteration (not a frozen snapshot), so the first
    /// repair wins and the second — now seeing the just-inserted clean key —
    /// correctly backs off rather than overwriting it. Neither value is
    /// discarded; the second key is left in its original, still-corrupted
    /// form rather than silently dropped.
    #[test]
    fn leaked_special_token_repair_resolves_same_pass_collisions_without_data_loss() {
        let mut v = serde_json::json!({"<|\"|>type": "first", "type<|\"|>": "second"});
        repair_leaked_special_token_keys(&mut v);
        assert_eq!(
            v,
            serde_json::json!({"type": "first", "type<|\"|>": "second"}),
            "the first-processed corrupted key repairs cleanly; the second, now \
             colliding with the repaired key, must be left untouched rather than \
             overwriting or being discarded: {v:?}"
        );
    }

    /// User data lives in these payloads — only keys are ever rewritten, a
    /// value that happens to contain the same substring means whatever the
    /// user wrote.
    #[test]
    fn leaked_special_token_repair_never_rewrites_values() {
        let mut v = serde_json::json!({
            "properties": {"note": "literally <|\"|>type<|\"|>", "<|\"|>type<|\"|>": "text"}
        });
        repair_leaked_special_token_keys(&mut v);
        assert_eq!(
            v,
            serde_json::json!({
                "properties": {"note": "literally <|\"|>type<|\"|>", "type": "text"}
            })
        );
    }

    /// Same rationale as `canonical_args_repairs_over_quoted_keys`: the
    /// malformed call and its repaired retry must resolve to one duplicate-
    /// guard identity, not two.
    #[test]
    fn canonical_args_repairs_leaked_special_token_keys() {
        let malformed = r#"{"fields":[{"<|\"|>type<|\"|>":"number","name":"replacement_cost"}]}"#;
        let clean = r#"{"fields":[{"type":"number","name":"replacement_cost"}]}"#;
        assert_eq!(canonical_args(malformed), canonical_args(clean));
    }

    /// The guard's identity must derive from the *parsed* arguments on both
    /// sides. Empty arguments are read as `{}` before execution, so a
    /// raw-string comparison would store `"{}"` and compare `""` — a write that
    /// could never match itself, silently disarming the guard for that call.
    #[tokio::test]
    async fn empty_arguments_compare_against_their_parsed_form() {
        let engine = Arc::new(MockEngine::tool_then_text("create_node", "", "Exists."));
        let executor = Arc::new(RecordingToolExecutor::new(create_node_executor()));
        let calls = executor.calls_handle();
        let agent_loop = LocalAgentLoop::new(engine, executor);

        let mut session = new_session();
        // What the daemon would have persisted for an empty-args call: the
        // canonical form of the parsed `{}`, not the empty string.
        session.prior_writes = vec![PriorWrite {
            tool: "create_node".to_string(),
            canonical_args: canonical_args("{}"),
            node_id: Some("nodespace://n1".to_string()),
            summary: Some("Buy milk".to_string()),
        }];

        agent_loop
            .run_turn(
                &mut session,
                "add it",
                |_| {},
                |_| {},
                CancellationToken::new(),
            )
            .await
            .expect("turn should succeed");

        assert!(
            !calls.lock().unwrap().iter().any(|c| c == "create_node"),
            "an empty-args repeat must still be recognised as the same call"
        );
    }

    // -----------------------------------------------------------------------
    // Two-stage routing (ADR-038)
    // -----------------------------------------------------------------------

    use crate::agent_types::SkillCandidate;

    /// A tool executor with skill retrieval wired up, so the loop takes the
    /// two-stage path instead of the single-turn fallback.
    struct RoutingToolExecutor {
        inner: MockToolExecutor,
        candidates: Vec<SkillCandidate>,
        /// Queries retrieval was actually asked for, so a test can assert the
        /// system — not the model — issued the retrieval.
        queries: Arc<std::sync::Mutex<Vec<String>>>,
    }

    impl RoutingToolExecutor {
        fn new(inner: MockToolExecutor, candidates: Vec<SkillCandidate>) -> Self {
            Self {
                inner,
                candidates,
                queries: Arc::new(std::sync::Mutex::new(Vec::new())),
            }
        }

        fn queries_handle(&self) -> Arc<std::sync::Mutex<Vec<String>>> {
            self.queries.clone()
        }
    }

    #[async_trait::async_trait]
    impl AgentToolExecutor for RoutingToolExecutor {
        async fn available_tools(&self) -> Result<Vec<ToolDefinition>, ToolError> {
            self.inner.available_tools().await
        }
        async fn execute(
            &self,
            name: &str,
            args: serde_json::Value,
        ) -> Result<ToolResult, ToolError> {
            self.inner.execute(name, args).await
        }
        async fn routing_available(&self) -> bool {
            true
        }
        async fn retrieve_skills(
            &self,
            query: &str,
            limit: usize,
        ) -> Result<crate::agent_types::SkillRetrieval, ToolError> {
            self.queries.lock().unwrap().push(query.to_string());
            let mut c = self.candidates.clone();
            c.truncate(limit);
            Ok(crate::agent_types::SkillRetrieval { candidates: c })
        }
    }

    fn skill_candidate(name: &str, score: f32, tools: &[&str]) -> SkillCandidate {
        SkillCandidate {
            id: format!("skill-{name}"),
            name: name.to_string(),
            description: format!("Use this to {name}"),
            score,
            tools: tools.iter().map(|t| t.to_string()).collect(),
            instructions: format!("INSTRUCTIONS FOR {name}"),
            schema_metadata: json!([]),
        }
    }

    /// A model that calls `route_query` at Stage 1, then the given tool.
    fn routed_engine(
        query: &str,
        tool_name: &str,
        tool_args: &str,
        final_text: &str,
    ) -> MockEngine {
        MockEngine::new(vec![
            // Stage 1: the structural choice, expressed as a tool call.
            vec![
                StreamingChunk::ToolCallStart {
                    id: "r1".into(),
                    name: routing::ROUTE_QUERY_TOOL.into(),
                    provider_extra: None,
                },
                StreamingChunk::ToolCallArgs {
                    id: "r1".into(),
                    args_json: json!({ "query": query }).to_string(),
                },
                StreamingChunk::Done {
                    usage: InferenceUsage {
                        prompt_tokens: 8,
                        completion_tokens: 4,
                    },
                },
            ],
            // Stage 2: act on the judged candidate.
            vec![
                StreamingChunk::ToolCallStart {
                    id: "t1".into(),
                    name: tool_name.into(),
                    provider_extra: None,
                },
                StreamingChunk::ToolCallArgs {
                    id: "t1".into(),
                    args_json: tool_args.into(),
                },
                StreamingChunk::Done {
                    usage: InferenceUsage {
                        prompt_tokens: 20,
                        completion_tokens: 10,
                    },
                },
            ],
            vec![
                StreamingChunk::Token {
                    text: final_text.into(),
                },
                StreamingChunk::Done {
                    usage: InferenceUsage {
                        prompt_tokens: 30,
                        completion_tokens: 12,
                    },
                },
            ],
        ])
    }

    #[tokio::test]
    async fn retrieval_runs_as_a_system_step_on_stage1s_query() {
        // ADR-038: retrieval is a deterministic system step, not a model tool
        // call. The model supplies the query; the system issues the retrieval.
        let engine = routed_engine(
            "create a schema",
            "search_nodes",
            r#"{"query":"x"}"#,
            "Done.",
        );
        let exec = RoutingToolExecutor::new(
            MockToolExecutor::new(),
            vec![skill_candidate("research", 0.9, &["search_nodes"])],
        );
        let queries = exec.queries_handle();
        let loop_ = LocalAgentLoop::new(Arc::new(engine), Arc::new(exec));
        let mut session = new_session();
        loop_
            .run_turn(
                &mut session,
                "find my billing notes",
                |_| {},
                |_| {},
                CancellationToken::new(),
            )
            .await
            .unwrap();

        assert_eq!(
            queries.lock().unwrap().as_slice(),
            &["create a schema".to_string()],
            "the system must issue exactly one retrieval, on Stage 1's query"
        );
    }

    /// A model that calls `route_multi` at Stage 1 with `queries`, then the
    /// given tool.
    fn multi_routed_engine(
        queries: &[&str],
        tool_name: &str,
        tool_args: &str,
        final_text: &str,
    ) -> MockEngine {
        MockEngine::new(vec![
            vec![
                StreamingChunk::ToolCallStart {
                    id: "r1".into(),
                    name: routing::ROUTE_MULTI_TOOL.into(),
                    provider_extra: None,
                },
                StreamingChunk::ToolCallArgs {
                    id: "r1".into(),
                    args_json: json!({ "queries": queries }).to_string(),
                },
                StreamingChunk::Done {
                    usage: InferenceUsage {
                        prompt_tokens: 8,
                        completion_tokens: 4,
                    },
                },
            ],
            vec![
                StreamingChunk::ToolCallStart {
                    id: "t1".into(),
                    name: tool_name.into(),
                    provider_extra: None,
                },
                StreamingChunk::ToolCallArgs {
                    id: "t1".into(),
                    args_json: tool_args.into(),
                },
                StreamingChunk::Done {
                    usage: InferenceUsage {
                        prompt_tokens: 20,
                        completion_tokens: 10,
                    },
                },
            ],
            vec![
                StreamingChunk::Token {
                    text: final_text.into(),
                },
                StreamingChunk::Done {
                    usage: InferenceUsage {
                        prompt_tokens: 30,
                        completion_tokens: 12,
                    },
                },
            ],
        ])
    }

    #[tokio::test]
    async fn route_multi_issues_one_retrieval_per_query_and_merges_deduped_candidates() {
        // #1909: route_multi re-enters retrieval once per intent rather than
        // compressing several intents into Stage 1's single query. This pins
        // the fan-out (one retrieve_skills call per element of `queries`, in
        // order) and the merge behavior downstream (agent_loop.rs's
        // RouteDecision::Multi arm): candidates dedup by skill id across
        // queries — asserted here via Stage 2's actual injected prompt, not
        // just that the turn completes, since a failure to dedup would still
        // let the turn succeed on a duplicated candidate set.
        let engine = RecordingEngine::new(multi_routed_engine(
            &["log an expense", "remind me Friday"],
            "search_nodes",
            r#"{"query":"x"}"#,
            "Done.",
        ));
        let prompts = engine.system_prompts_handle();
        // RoutingToolExecutor returns this same fixed set regardless of query
        // text (it clones and truncates to `limit`, the same RETRIEVAL_TOP_K
        // both queries request), so 2 queries produce 2x3 = 6 raw hits with
        // identical ids before merge. A naive concat would render each
        // candidate's instructions twice; the merge must collapse that to 3.
        let exec = RoutingToolExecutor::new(
            MockToolExecutor::new(),
            vec![
                skill_candidate("highest", 0.9, &["search_nodes"]),
                skill_candidate("second", 0.7, &["search_nodes"]),
                skill_candidate("third", 0.5, &["search_nodes"]),
            ],
        );
        let queries = exec.queries_handle();
        let loop_ = LocalAgentLoop::new(Arc::new(engine), Arc::new(exec));
        let mut session = new_session();
        let result = loop_
            .run_turn(
                &mut session,
                "log a $42 lunch expense and remind me to follow up with Sarah on Friday",
                |_| {},
                |_| {},
                CancellationToken::new(),
            )
            .await
            .unwrap();

        assert_eq!(
            queries.lock().unwrap().as_slice(),
            &["log an expense".to_string(), "remind me Friday".to_string()],
            "retrieval must run once per route_multi query, in order"
        );

        let stage2_prompt = &prompts.lock().unwrap()[1];
        for kept in ["highest", "second", "third"] {
            assert!(
                stage2_prompt.contains(kept),
                "candidate {kept} must survive the merge: {stage2_prompt}"
            );
        }
        // Each candidate's instruction block would appear twice if the two
        // queries' identical hits were concatenated rather than deduped by
        // skill id — this catches a dedup regression the presence checks
        // above would miss (a duplicated-but-present candidate still
        // "contains").
        assert_eq!(
            stage2_prompt.matches("INSTRUCTIONS FOR highest").count(),
            1,
            "a candidate matching both queries must be merged once, not duplicated: {stage2_prompt}"
        );

        assert_eq!(result.tool_calls_made[0].name, "search_nodes");
    }

    #[tokio::test]
    async fn stage2_tool_surface_is_scoped_to_the_matched_skill() {
        // The matched skill is read-only, so a destructive tool must not be in
        // reach even though the executor registers one.
        let engine = routed_engine("find notes", "search_nodes", r#"{"query":"x"}"#, "Done.");
        let inner = MockToolExecutor::new().with_tool(
            "delete_node",
            json!({"type": "object", "properties": {"id": {"type": "string"}}}),
            json!({"deleted": true}),
        );
        let exec = RoutingToolExecutor::new(
            inner,
            vec![skill_candidate(
                "research",
                0.9,
                &["search_nodes", "get_node"],
            )],
        );
        let loop_ = LocalAgentLoop::new(Arc::new(engine), Arc::new(exec));
        let mut session = new_session();
        let result = loop_
            .run_turn(
                &mut session,
                "find my billing notes",
                |_| {},
                |_| {},
                CancellationToken::new(),
            )
            .await
            .unwrap();
        assert_eq!(result.tool_calls_made[0].name, "search_nodes");
    }

    #[tokio::test]
    async fn a_mutating_skill_below_its_bar_is_not_offered() {
        // Identical score, different verdict: 0.2 clears the read bar but not
        // the mutating one, so this candidate is never rendered or actioned.
        let cands = vec![skill_candidate("deletion", 0.2, &["delete_node"])];
        assert!(routing::render_candidates_for_prompt(&cands).is_none());
    }

    // -----------------------------------------------------------------------
    // `session.routing_disabled` (Option C from ADR-038): the
    // routing-reliability matrix in `tests/live_openai_compat_routing.rs`
    // found Stage-2 candidate injection suppresses tool-calling outright on
    // some served models. A cached per-model probe verdict sets this flag;
    // these tests cover what the loop does with it.
    // -----------------------------------------------------------------------

    /// Wraps a `ChatInferenceEngine` and records the system-prompt content of
    /// every `generate` call, so a test can assert on what actually reached
    /// the model rather than only on the tool calls that came back.
    struct RecordingEngine<E: ChatInferenceEngine> {
        inner: E,
        system_prompts: Arc<std::sync::Mutex<Vec<String>>>,
        tool_names: Arc<std::sync::Mutex<Vec<Vec<String>>>>,
    }

    impl<E: ChatInferenceEngine> RecordingEngine<E> {
        fn new(inner: E) -> Self {
            Self {
                inner,
                system_prompts: Arc::new(std::sync::Mutex::new(Vec::new())),
                tool_names: Arc::new(std::sync::Mutex::new(Vec::new())),
            }
        }

        fn system_prompts_handle(&self) -> Arc<std::sync::Mutex<Vec<String>>> {
            Arc::clone(&self.system_prompts)
        }

        /// Tool names offered on each `generate` call, in call order — so a
        /// test can assert on Stage 2's actual offered surface rather than
        /// just the prompt text.
        fn tool_names_handle(&self) -> Arc<std::sync::Mutex<Vec<Vec<String>>>> {
            Arc::clone(&self.tool_names)
        }
    }

    #[async_trait]
    impl<E: ChatInferenceEngine> ChatInferenceEngine for RecordingEngine<E> {
        async fn generate(
            &self,
            request: InferenceRequest,
            on_chunk: Box<dyn Fn(StreamingChunk) + Send>,
        ) -> Result<InferenceUsage, InferenceError> {
            if let Some(system) = request.messages.iter().find(|m| m.role == Role::System) {
                self.system_prompts
                    .lock()
                    .unwrap()
                    .push(system.content.clone());
            }
            self.tool_names.lock().unwrap().push(
                request
                    .tools
                    .as_deref()
                    .unwrap_or_default()
                    .iter()
                    .map(|t| t.name.clone())
                    .collect(),
            );
            self.inner.generate(request, on_chunk).await
        }

        async fn model_info(&self) -> Result<Option<ChatModelSpec>, InferenceError> {
            self.inner.model_info().await
        }

        async fn token_count(&self, text: &str) -> Result<u32, InferenceError> {
            self.inner.token_count(text).await
        }
    }

    #[tokio::test]
    async fn routing_enabled_injects_the_candidate_block() {
        let engine = RecordingEngine::new(routed_engine(
            "find notes",
            "search_nodes",
            r#"{"query":"x"}"#,
            "Done.",
        ));
        let prompts = engine.system_prompts_handle();
        let exec = RoutingToolExecutor::new(
            MockToolExecutor::new(),
            vec![skill_candidate("research", 0.9, &["search_nodes"])],
        );
        let loop_ = LocalAgentLoop::new(Arc::new(engine), Arc::new(exec));
        let mut session = new_session();
        assert!(!session.routing_disabled, "default session is not disabled");

        loop_
            .run_turn(
                &mut session,
                "find my billing notes",
                |_| {},
                |_| {},
                CancellationToken::new(),
            )
            .await
            .expect("turn should succeed");

        let stage2_prompt = &prompts.lock().unwrap()[1];
        assert!(
            stage2_prompt.contains("research"),
            "Stage 2's prompt must carry the matched candidate: {stage2_prompt}"
        );
    }

    #[tokio::test]
    async fn routing_disabled_skips_injection_but_keeps_tool_scoping() {
        // Same matched candidate as the enabled case above, but the session
        // carries a probe verdict that says injection suppresses this model's
        // tool-calling.
        let engine = RecordingEngine::new(routed_engine(
            "find notes",
            "search_nodes",
            r#"{"query":"x"}"#,
            "Done.",
        ));
        let prompts = engine.system_prompts_handle();
        let inner = MockToolExecutor::new().with_tool(
            "delete_node",
            json!({"type": "object", "properties": {"id": {"type": "string"}}}),
            json!({"deleted": true}),
        );
        let exec = RoutingToolExecutor::new(
            inner,
            vec![skill_candidate(
                "research",
                0.9,
                &["search_nodes", "get_node"],
            )],
        );
        let loop_ = LocalAgentLoop::new(Arc::new(engine), Arc::new(exec));
        let mut session = new_session();
        session.routing_disabled = true;

        let result = loop_
            .run_turn(
                &mut session,
                "find my billing notes",
                |_| {},
                |_| {},
                CancellationToken::new(),
            )
            .await
            .expect("turn should succeed");

        let stage2_prompt = &prompts.lock().unwrap()[1];
        assert!(
            !stage2_prompt.contains("INSTRUCTIONS FOR research"),
            "a disabled session must not receive the candidate's instruction subtree: \
             {stage2_prompt}"
        );
        assert!(
            !stage2_prompt.contains("REFERENCE — procedures relevant"),
            "a disabled session must not receive the candidate block header at all: \
             {stage2_prompt}"
        );
        // Tool scoping is a separate mechanism (ADR-038's trust boundary) from
        // prompt injection, and the matrix did not implicate it — it must stay
        // in effect even though injection is off.
        assert_eq!(result.tool_calls_made[0].name, "search_nodes");
    }

    #[tokio::test]
    async fn routing_disabled_still_excludes_resolve_query_when_its_guidance_is_unsent() {
        // A candidate whose whitelist includes resolve_query clears the score
        // gate on its own terms, so stage2_tools's fail-open exclusion never
        // fires — resolve_query is legitimately in `permitted`. But this
        // session's routing is disabled, so `candidate_block` (the EXISTING
        // SCHEMAS block resolve_query's required node_type parameter
        // depends on) is forced to `None` regardless. Offering resolve_query
        // here reproduces #1840's defect through a second door.
        let engine = RecordingEngine::new(routed_engine(
            "find notes",
            "search_nodes",
            r#"{"query":"x"}"#,
            "Done.",
        ));
        let tool_calls = engine.tool_names_handle();
        let inner = MockToolExecutor::new().with_tool(
            "resolve_query",
            json!({"type": "object", "properties": {
                "request": {"type": "string"}, "node_type": {"type": "string"}
            }}),
            json!({"resolved": false, "reason": "no_match"}),
        );
        let exec = RoutingToolExecutor::new(
            inner,
            vec![skill_candidate(
                "Graph Editing",
                0.9,
                &["search_nodes", "resolve_query"],
            )],
        );
        let loop_ = LocalAgentLoop::new(Arc::new(engine), Arc::new(exec));
        let mut session = new_session();
        session.routing_disabled = true;

        loop_
            .run_turn(
                &mut session,
                "find my billing notes",
                |_| {},
                |_| {},
                CancellationToken::new(),
            )
            .await
            .expect("turn should succeed");

        let stage2_tools = &tool_calls.lock().unwrap()[1];
        assert!(
            !stage2_tools.contains(&"resolve_query".to_string()),
            "resolve_query must not be offered when its guidance block was not sent: \
             {stage2_tools:?}"
        );
    }

    #[tokio::test]
    async fn routing_enabled_offers_resolve_query_when_its_entity_types_render() {
        // The genuine happy path: routing is enabled, a Graph Editing
        // candidate clears the gate, and its schema_metadata renders a real
        // EXISTING SCHEMAS sub-block — so resolve_query must still be
        // reachable. Guards against the precise per-candidate guidance check
        // (`routing::tools_with_available_guidance`) over-excluding the tool
        // once it stopped trusting `candidate_block.is_some()` as a whole.
        let engine = RecordingEngine::new(routed_engine(
            "mark invoice paid",
            "resolve_query",
            r#"{"request":"mark the $500 invoice as paid","node_type":"invoice"}"#,
            "Done.",
        ));
        let tool_calls = engine.tool_names_handle();
        let inner = MockToolExecutor::new().with_tool(
            "resolve_query",
            json!({"type": "object", "properties": {
                "request": {"type": "string"}, "node_type": {"type": "string"}
            }}),
            json!({"resolved": false, "reason": "no_match"}),
        );
        let mut candidate =
            skill_candidate("Graph Editing", 0.9, &["search_nodes", "resolve_query"]);
        candidate.schema_metadata = json!([{"type_id": "invoice", "fields": []}]);
        let exec = RoutingToolExecutor::new(inner, vec![candidate]);
        let loop_ = LocalAgentLoop::new(Arc::new(engine), Arc::new(exec));
        let mut session = new_session();
        assert!(!session.routing_disabled);

        loop_
            .run_turn(
                &mut session,
                "mark the $500 invoice as paid",
                |_| {},
                |_| {},
                CancellationToken::new(),
            )
            .await
            .expect("turn should succeed");

        let stage2_tools = &tool_calls.lock().unwrap()[1];
        assert!(
            stage2_tools.contains(&"resolve_query".to_string()),
            "resolve_query must be offered once its candidate's entity-types block \
             actually rendered: {stage2_tools:?}"
        );
    }

    #[tokio::test]
    async fn a_candidate_with_no_schema_metadata_still_excludes_resolve_query_though_the_block_header_renders(
    ) {
        // The edge case the re-reviewer flagged: render_candidates_for_prompt
        // returns Some(..) from its header text alone once ANY candidate
        // clears the gate, even if that candidate's own schema_metadata is
        // empty and so its EXISTING SCHEMAS sub-block never appears.
        // candidate_block.is_some() would have wrongly treated resolve_query
        // as having guidance here; the per-candidate check must not.
        let engine = RecordingEngine::new(routed_engine(
            "find notes",
            "search_nodes",
            r#"{"query":"x"}"#,
            "Done.",
        ));
        let tool_calls = engine.tool_names_handle();
        let prompts = engine.system_prompts_handle();
        let inner = MockToolExecutor::new().with_tool(
            "resolve_query",
            json!({"type": "object", "properties": {
                "request": {"type": "string"}, "node_type": {"type": "string"}
            }}),
            json!({"resolved": false, "reason": "no_match"}),
        );
        // Empty schema_metadata (skill_candidate's default) — no typed
        // entities, so no EXISTING SCHEMAS sub-block for this candidate.
        let exec = RoutingToolExecutor::new(
            inner,
            vec![skill_candidate(
                "Graph Editing",
                0.9,
                &["search_nodes", "resolve_query"],
            )],
        );
        let loop_ = LocalAgentLoop::new(Arc::new(engine), Arc::new(exec));
        let mut session = new_session();
        assert!(!session.routing_disabled);

        loop_
            .run_turn(
                &mut session,
                "find my billing notes",
                |_| {},
                |_| {},
                CancellationToken::new(),
            )
            .await
            .expect("turn should succeed");

        // Sanity check the premise: the block header IS present (this is
        // exactly the case a coarser `candidate_block.is_some()` check would
        // have gotten wrong).
        let stage2_prompt = &prompts.lock().unwrap()[1];
        assert!(
            stage2_prompt.contains("REFERENCE — procedures relevant"),
            "premise of this test requires a rendered block header: {stage2_prompt}"
        );
        assert!(
            !stage2_prompt.contains("EXISTING SCHEMAS"),
            "premise of this test requires no entity-types sub-block: {stage2_prompt}"
        );

        let stage2_tools = &tool_calls.lock().unwrap()[1];
        assert!(
            !stage2_tools.contains(&"resolve_query".to_string()),
            "resolve_query must not be offered when its OWN candidate's entity-types \
             block didn't render, even though the block header did: {stage2_tools:?}"
        );
    }

    #[tokio::test]
    async fn stage1_clarify_answers_the_user_without_running_stage2() {
        let engine = MockEngine::new(vec![vec![
            StreamingChunk::ToolCallStart {
                id: "r1".into(),
                name: routing::ROUTE_CLARIFY_TOOL.into(),
                provider_extra: None,
            },
            StreamingChunk::ToolCallArgs {
                id: "r1".into(),
                args_json: json!({
                    "question": "Did you want to track debts or search notes?",
                    "options": ["Track who owes me money", "Search existing notes"]
                })
                .to_string(),
            },
            StreamingChunk::Done {
                usage: InferenceUsage {
                    prompt_tokens: 8,
                    completion_tokens: 4,
                },
            },
        ]]);
        let exec = RoutingToolExecutor::new(MockToolExecutor::new(), vec![]);
        let queries = exec.queries_handle();
        let loop_ = LocalAgentLoop::new(Arc::new(engine), Arc::new(exec));
        let mut session = new_session();
        let result = loop_
            .run_turn(
                &mut session,
                "keep tabs on who owes me money",
                |_| {},
                |_| {},
                CancellationToken::new(),
            )
            .await
            .unwrap();

        assert!(result.tool_calls_made.is_empty());
        // Specific, with the concrete options — not a bare "what do you mean?".
        assert!(result.response.contains("Track who owes me money"));
        assert!(result.response.contains("Search existing notes"));
        assert!(
            queries.lock().unwrap().is_empty(),
            "a clarification must not trigger retrieval"
        );

        // #1930: the structured question/options must reach the caller
        // unflattened too, not just baked into `response`'s markdown prose —
        // that structure is what lets the frontend render clickable options
        // instead of parsing bullets back out of text.
        let clarify = result
            .clarify
            .expect("a route_clarify turn must carry a structured ClarifyPrompt");
        assert_eq!(
            clarify.question,
            "Did you want to track debts or search notes?"
        );
        assert_eq!(
            clarify.options,
            vec![
                "Track who owes me money".to_string(),
                "Search existing notes".to_string()
            ]
        );
    }

    /// Review follow-up: the model can structurally batch a real
    /// write tool call alongside `route_clarify` in the SAME iteration —
    /// nothing prevents it beyond `route_clarify`'s own description
    /// ("do not call another tool in the same turn"), which is a soft prompt
    /// instruction, not an enforced constraint. If that happened and the
    /// write executed for real, the user would see only the clarifying
    /// question with no indication a change had already landed. The
    /// pre-scan in `run_turn` must supersede the whole batch instead: the
    /// write is reported as skipped, not executed.
    #[tokio::test]
    async fn stage2_route_clarify_supersedes_a_batched_write_in_the_same_iteration() {
        let engine = MockEngine::new(vec![
            vec![
                StreamingChunk::ToolCallStart {
                    id: "r1".into(),
                    name: routing::ROUTE_QUERY_TOOL.into(),
                    provider_extra: None,
                },
                StreamingChunk::ToolCallArgs {
                    id: "r1".into(),
                    args_json: json!({ "query": "update the ticket" }).to_string(),
                },
                StreamingChunk::Done {
                    usage: InferenceUsage {
                        prompt_tokens: 8,
                        completion_tokens: 4,
                    },
                },
            ],
            // Stage 2: the model calls BOTH update_node and route_clarify in
            // the same iteration — the exact batching this guard exists for.
            vec![
                StreamingChunk::ToolCallStart {
                    id: "t1".into(),
                    name: "update_node".into(),
                    provider_extra: None,
                },
                StreamingChunk::ToolCallArgs {
                    id: "t1".into(),
                    args_json: json!({
                        "id": "abc-1",
                        "field_values": { "status": "done" }
                    })
                    .to_string(),
                },
                StreamingChunk::ToolCallStart {
                    id: "t2".into(),
                    name: routing::ROUTE_CLARIFY_TOOL.into(),
                    provider_extra: None,
                },
                StreamingChunk::ToolCallArgs {
                    id: "t2".into(),
                    args_json: json!({
                        "question": "Which ticket did you mean?",
                        "options": [
                            {"id": "abc-1", "label": "First ticket"},
                            {"id": "abc-2", "label": "Second ticket"}
                        ]
                    })
                    .to_string(),
                },
                StreamingChunk::Done {
                    usage: InferenceUsage {
                        prompt_tokens: 20,
                        completion_tokens: 10,
                    },
                },
            ],
        ]);

        let candidates = vec![skill_candidate(
            "Graph Editing",
            0.9,
            &["update_node", "route_clarify"],
        )];
        let exec = RoutingToolExecutor::new(
            MockToolExecutor::new()
                .with_tool(
                    "update_node",
                    json!({"type": "object", "properties": {"id": {"type": "string"}}}),
                    json!({"ok": true, "updated": ["status"]}),
                )
                .with_tool(
                    "route_clarify",
                    json!({"type": "object", "properties": {
                        "question": {"type": "string"},
                        "options": {"type": "array"}
                    }}),
                    json!({}),
                ),
            candidates,
        );
        let loop_ = LocalAgentLoop::new(Arc::new(engine), Arc::new(exec));
        let mut session = new_session();
        let result = loop_
            .run_turn(
                &mut session,
                "update the ticket",
                |_| {},
                |_| {},
                CancellationToken::new(),
            )
            .await
            .unwrap();

        // Both calls are recorded (so the model's own record of what it
        // asked for is preserved), but update_node's result must show it was
        // skipped, NOT the mock's canned "ok: true, updated: [...]" —
        // proving the real executor was never invoked for it.
        assert_eq!(result.tool_calls_made.len(), 2);
        let update_record = result
            .tool_calls_made
            .iter()
            .find(|r| r.name == "update_node")
            .expect("update_node call must still be recorded");
        assert!(!update_record.is_error);
        assert_eq!(
            update_record.result["skipped"],
            "superseded_by_clarification"
        );
        assert_ne!(
            update_record.result.get("updated"),
            Some(&json!(["status"])),
            "the mock's canned write result must never appear — the write must not have executed"
        );

        let clarify_record = result
            .tool_calls_made
            .iter()
            .find(|r| r.name == "route_clarify")
            .expect("route_clarify call must be recorded");
        assert!(!clarify_record.is_error);

        let clarify = result
            .clarify
            .expect("the turn must still end as a clarification");
        assert_eq!(clarify.question, "Which ticket did you mean?");
    }

    /// A Stage-2 skill whose whitelist includes `route_clarify`
    /// calls it instead of guessing or answering in prose — this must end the
    /// turn the same way Stage 1's clarify does, without any further tool
    /// call or inference round.
    #[tokio::test]
    async fn stage2_route_clarify_ends_the_turn_without_further_tool_calls() {
        let engine = MockEngine::new(vec![
            // Stage 1: routes normally.
            vec![
                StreamingChunk::ToolCallStart {
                    id: "r1".into(),
                    name: routing::ROUTE_QUERY_TOOL.into(),
                    provider_extra: None,
                },
                StreamingChunk::ToolCallArgs {
                    id: "r1".into(),
                    args_json: json!({ "query": "update the ticket" }).to_string(),
                },
                StreamingChunk::Done {
                    usage: InferenceUsage {
                        prompt_tokens: 8,
                        completion_tokens: 4,
                    },
                },
            ],
            // Stage 2: the judged candidate whitelists route_clarify and the
            // model calls it — with the richer {id, label} option shape
            // (see tools::def_route_clarify).
            vec![
                StreamingChunk::ToolCallStart {
                    id: "t1".into(),
                    name: routing::ROUTE_CLARIFY_TOOL.into(),
                    provider_extra: None,
                },
                StreamingChunk::ToolCallArgs {
                    id: "t1".into(),
                    args_json: json!({
                        "question": "Which ticket did you mean?",
                        "options": [
                            {"id": "abc-1", "label": "Rotate signing keys"},
                            {"id": "abc-2", "label": "Backfill audit log retention"}
                        ]
                    })
                    .to_string(),
                },
                StreamingChunk::Done {
                    usage: InferenceUsage {
                        prompt_tokens: 20,
                        completion_tokens: 10,
                    },
                },
            ],
            // A third batch that must never be consumed — proves the loop
            // returned immediately rather than running another inference
            // round to "finish" the turn.
            vec![
                StreamingChunk::Token {
                    text: "should never run".into(),
                },
                StreamingChunk::Done {
                    usage: InferenceUsage {
                        prompt_tokens: 99,
                        completion_tokens: 99,
                    },
                },
            ],
        ]);

        let candidates = vec![skill_candidate(
            "Graph Editing",
            0.9,
            &["update_node", "route_clarify"],
        )];
        let exec = RoutingToolExecutor::new(
            MockToolExecutor::new().with_tool(
                "route_clarify",
                json!({"type": "object", "properties": {
                    "question": {"type": "string"},
                    "options": {"type": "array"}
                }}),
                json!({}),
            ),
            candidates,
        );
        let loop_ = LocalAgentLoop::new(Arc::new(engine), Arc::new(exec));
        let mut session = new_session();
        let result = loop_
            .run_turn(
                &mut session,
                "update the ticket",
                |_| {},
                |_| {},
                CancellationToken::new(),
            )
            .await
            .unwrap();

        // The route_clarify call itself is a real, recorded tool execution
        // (unlike Stage 1's clarify, which never reaches the tool layer) —
        // but nothing after it ran.
        assert_eq!(result.tool_calls_made.len(), 1);
        assert_eq!(result.tool_calls_made[0].name, "route_clarify");
        assert!(!result.tool_calls_made[0].is_error);

        let clarify = result
            .clarify
            .expect("a Stage-2 route_clarify turn must carry a structured ClarifyPrompt");
        assert_eq!(clarify.question, "Which ticket did you mean?");
        // Flattened to labels — the id is for the MODEL (so it can name an
        // exact candidate), not part of what the user is shown.
        assert_eq!(
            clarify.options,
            vec![
                "Rotate signing keys".to_string(),
                "Backfill audit log retention".to_string()
            ]
        );
        assert!(result.response.contains("Rotate signing keys"));
        assert!(result.response.contains("Backfill audit log retention"));
        // Direct evidence the third batch was never consumed, not just an
        // inference from the response matching the clarify text: if a third
        // `engine.generate()` round had run, MockEngine would have played it
        // back and this text would appear somewhere in the response.
        assert!(
            !result.response.contains("should never run"),
            "a third inference round ran when the turn should have ended after route_clarify"
        );

        // The conversation stays well-formed: the assistant's tool-calls
        // message and its matching tool result are both present, followed by
        // the clarification text as the final assistant message.
        let last = session.messages.last().expect("at least one message");
        assert_eq!(last.role, Role::Assistant);
        assert!(last.content.contains("Which ticket did you mean?"));
    }

    /// A route_clarify call with a blank question is not a decision to act
    /// on — it must fall through to the normal executor path rather than
    /// silently ending the turn as if it were a real clarification. (This
    /// test's mock executor does not itself validate the blank question the
    /// way the real `GraphToolExecutor::exec_route_clarify` does — see that
    /// function's own unit tests for the validation-error behavior. What
    /// this test proves is narrower and still real: the turn is NOT treated
    /// as a clarification and continues normally.)
    #[tokio::test]
    async fn stage2_route_clarify_with_blank_question_falls_through_to_normal_execution() {
        let engine = MockEngine::new(vec![
            vec![
                StreamingChunk::ToolCallStart {
                    id: "r1".into(),
                    name: routing::ROUTE_QUERY_TOOL.into(),
                    provider_extra: None,
                },
                StreamingChunk::ToolCallArgs {
                    id: "r1".into(),
                    args_json: json!({ "query": "update the ticket" }).to_string(),
                },
                StreamingChunk::Done {
                    usage: InferenceUsage {
                        prompt_tokens: 8,
                        completion_tokens: 4,
                    },
                },
            ],
            vec![
                StreamingChunk::ToolCallStart {
                    id: "t1".into(),
                    name: routing::ROUTE_CLARIFY_TOOL.into(),
                    provider_extra: None,
                },
                StreamingChunk::ToolCallArgs {
                    id: "t1".into(),
                    args_json: json!({ "question": "   ", "options": [] }).to_string(),
                },
                StreamingChunk::Done {
                    usage: InferenceUsage {
                        prompt_tokens: 20,
                        completion_tokens: 10,
                    },
                },
            ],
            vec![
                StreamingChunk::Token {
                    text: "final reply".into(),
                },
                StreamingChunk::Done {
                    usage: InferenceUsage {
                        prompt_tokens: 30,
                        completion_tokens: 12,
                    },
                },
            ],
        ]);

        let candidates = vec![skill_candidate(
            "Graph Editing",
            0.9,
            &["update_node", "route_clarify"],
        )];
        let exec = RoutingToolExecutor::new(
            MockToolExecutor::new().with_tool(
                "route_clarify",
                json!({"type": "object", "properties": {
                    "question": {"type": "string"},
                    "options": {"type": "array"}
                }}),
                json!({}),
            ),
            candidates,
        );
        let loop_ = LocalAgentLoop::new(Arc::new(engine), Arc::new(exec));
        let mut session = new_session();
        let result = loop_
            .run_turn(
                &mut session,
                "update the ticket",
                |_| {},
                |_| {},
                CancellationToken::new(),
            )
            .await
            .unwrap();

        // Not treated as a clarification decision — the fallthrough runs the
        // normal executor path (this test's mock, unlike the real
        // `GraphToolExecutor::exec_route_clarify`, does not itself validate
        // the blank question — that validation is covered directly by
        // `tools::parse_route_clarify_args`'s own unit tests) and the loop
        // continues to a real third inference round, proving the turn was
        // NOT ended early as a clarification.
        assert!(result.clarify.is_none());
        assert_eq!(result.tool_calls_made.len(), 1);
        assert_eq!(result.tool_calls_made[0].name, "route_clarify");
        assert_eq!(result.response, "final reply");
    }

    #[tokio::test]
    async fn clarification_still_reports_thinking_then_idle() {
        // A clarification returns early, but the caller must still see the
        // normal status arc — a reply with no preceding Thinking would leave a
        // UI showing idle while the routing turn is generating.
        let engine = MockEngine::new(vec![vec![
            StreamingChunk::ToolCallStart {
                id: "r1".into(),
                name: routing::ROUTE_CLARIFY_TOOL.into(),
                provider_extra: None,
            },
            StreamingChunk::ToolCallArgs {
                id: "r1".into(),
                args_json: json!({"question": "Which one?", "options": ["A", "B"]}).to_string(),
            },
            StreamingChunk::Done {
                usage: InferenceUsage {
                    prompt_tokens: 8,
                    completion_tokens: 4,
                },
            },
        ]]);
        let statuses = Arc::new(std::sync::Mutex::new(Vec::new()));
        let sink = statuses.clone();
        let loop_ = LocalAgentLoop::new(
            Arc::new(engine),
            Arc::new(RoutingToolExecutor::new(MockToolExecutor::new(), vec![])),
        );
        let mut session = new_session();
        loop_
            .run_turn(
                &mut session,
                "something ambiguous",
                move |s| sink.lock().unwrap().push(format!("{s:?}")),
                |_| {},
                CancellationToken::new(),
            )
            .await
            .unwrap();

        let seen = statuses.lock().unwrap().clone();
        assert!(
            seen.iter().any(|s| s.contains("Thinking")),
            "expected a Thinking status before the clarification: {seen:?}"
        );
        assert!(
            seen.last().is_some_and(|s| s.contains("Idle")),
            "turn must end Idle: {seen:?}"
        );
    }

    #[tokio::test]
    async fn a_second_clarification_for_one_intent_falls_through_to_retrieval() {
        // The contract: at most one clarification per intent. On the turn after
        // the user answered one, Stage 1 asking again must be suppressed and
        // the turn must retrieve on the raw message instead.
        let engine = MockEngine::new(vec![
            vec![
                StreamingChunk::ToolCallStart {
                    id: "r1".into(),
                    name: routing::ROUTE_CLARIFY_TOOL.into(),
                    provider_extra: None,
                },
                StreamingChunk::ToolCallArgs {
                    id: "r1".into(),
                    args_json: json!({"question": "Which one?", "options": ["A", "B"]}).to_string(),
                },
                StreamingChunk::Done {
                    usage: InferenceUsage {
                        prompt_tokens: 8,
                        completion_tokens: 4,
                    },
                },
            ],
            vec![
                StreamingChunk::Token {
                    text: "Here is what I found.".to_string(),
                },
                StreamingChunk::Done {
                    usage: InferenceUsage {
                        prompt_tokens: 20,
                        completion_tokens: 8,
                    },
                },
            ],
        ]);
        let exec = RoutingToolExecutor::new(
            MockToolExecutor::new(),
            vec![skill_candidate("research", 0.9, &["search_nodes"])],
        );
        let queries = exec.queries_handle();
        let loop_ = LocalAgentLoop::new(Arc::new(engine), Arc::new(exec));

        let mut session = new_session();
        // A prior clarification the user has already answered.
        session.messages.push(ChatMessage::text(
            Role::Assistant,
            format!("{CLARIFICATION_OPENER}. Which one?"),
        ));
        session
            .messages
            .push(ChatMessage::text(Role::User, "the first one".to_string()));

        let result = loop_
            .run_turn(
                &mut session,
                "the first one",
                |_| {},
                |_| {},
                CancellationToken::new(),
            )
            .await
            .unwrap();

        assert!(
            !result.response.starts_with(CLARIFICATION_OPENER),
            "never clarify twice for one intent: {:?}",
            result.response
        );
        assert_eq!(
            queries.lock().unwrap().len(),
            1,
            "the suppressed clarification must fall through to retrieval"
        );
    }

    #[tokio::test]
    async fn the_clarification_contract_re_arms_on_a_new_intent() {
        // ADR-038 scopes the contract "per intent", and a conversation is many
        // intents. A clarification answered earlier must not stop a genuinely
        // new ambiguous request from being clarified later.
        let mut session = new_session();
        session.messages.push(ChatMessage::text(
            Role::Assistant,
            format!("{CLARIFICATION_OPENER}. Which one?"),
        ));
        session
            .messages
            .push(ChatMessage::text(Role::User, "the first".to_string()));
        assert!(
            session_already_clarified(&session),
            "still inside the clarified intent"
        );

        // The turn completes: the agent answers rather than asking again.
        // That closes the intent.
        session.messages.push(ChatMessage::text(
            Role::Assistant,
            "Done — I created it.".to_string(),
        ));
        session.messages.push(ChatMessage::text(
            Role::User,
            "now do the other thing".to_string(),
        ));
        assert!(
            !session_already_clarified(&session),
            "a resolved turn starts a new intent; clarifying must be possible again"
        );
    }

    #[test]
    fn a_composed_clarification_is_recognised_as_one() {
        // Round-trip: the contract is carried in user-visible message text, so
        // a rewording of `format_clarification` that broke the read would
        // silently stop the contract enforcing. Tests that hand-build the
        // marker from the constant would still pass; this one would not.
        let mut session = new_session();
        session.messages.push(ChatMessage::text(
            Role::Assistant,
            format_clarification("Which did you mean?", &["Track debts".to_string()]),
        ));
        session
            .messages
            .push(ChatMessage::text(Role::User, "the first".to_string()));
        assert!(
            session_already_clarified(&session),
            "format_clarification output must be recognised by session_already_clarified"
        );
    }

    #[tokio::test]
    async fn stage1_blends_prior_turns_into_its_query() {
        // A follow-up naming its subject by pronoun gives the model nothing to
        // describe on its own. The prior turns must reach Stage 1.
        let mut session = new_session();
        session.messages.push(ChatMessage::text(
            Role::User,
            "create an invoice for Acme".to_string(),
        ));
        session.messages.push(ChatMessage::text(
            Role::Assistant,
            "Created invoice INV-1 for Acme.".to_string(),
        ));
        // run_turn pushes the current message before routing; mirror that.
        session
            .messages
            .push(ChatMessage::text(Role::User, "mark it paid".to_string()));

        let q = stage1_query(&session, "mark it paid");
        assert!(
            q.contains("mark it paid"),
            "current message must be present"
        );
        assert!(
            q.contains("invoice"),
            "prior turns must be blended so a pronoun-only follow-up still routes: {q:?}"
        );
        assert_eq!(
            q.matches("mark it paid").count(),
            1,
            "the current message must not be blended twice: {q:?}"
        );
    }

    #[tokio::test]
    async fn stage1_query_on_a_first_turn_is_just_the_message() {
        let mut session = new_session();
        session
            .messages
            .push(ChatMessage::text(Role::User, "find my notes".to_string()));
        assert_eq!(stage1_query(&session, "find my notes"), "find my notes");
    }

    #[tokio::test]
    async fn stage1_query_labels_prior_context_separately_from_the_current_request() {
        // #1909: an unlabeled blend of prior turns + current message read to
        // the model like several things to do, and route_multi fired on
        // genuinely single-intent turns as a result. The current request
        // must be clearly demarcated from background so the model routes
        // only what the user is asking for *now*.
        let mut session = new_session();
        session.messages.push(ChatMessage::text(
            Role::User,
            "create an invoice for Acme".to_string(),
        ));
        session.messages.push(ChatMessage::text(
            Role::Assistant,
            "Created invoice INV-1 for Acme.".to_string(),
        ));
        session
            .messages
            .push(ChatMessage::text(Role::User, "mark it paid".to_string()));

        let q = stage1_query(&session, "mark it paid");
        assert!(
            q.contains("PRIOR CONTEXT"),
            "prior turns must be introduced as background, not left unlabeled: {q:?}"
        );
        assert!(
            q.contains("CURRENT REQUEST"),
            "the current message must be explicitly marked as what to route: {q:?}"
        );
        let current_request_pos = q.find("CURRENT REQUEST").expect("checked above");
        let mark_it_paid_pos = q
            .find("mark it paid")
            .expect("current message must be present");
        assert!(
            mark_it_paid_pos > current_request_pos,
            "the current message must appear after its CURRENT REQUEST label: {q:?}"
        );
    }

    #[tokio::test]
    async fn an_unanswered_clarification_is_not_treated_as_already_clarified() {
        // The clarification we just asked and are still waiting on is not a
        // prior one — otherwise a single clarification would disable the
        // mechanism for the rest of the conversation.
        let mut session = new_session();
        session.messages.push(ChatMessage::text(
            Role::Assistant,
            format!("{CLARIFICATION_OPENER}. Which one?"),
        ));
        assert!(!session_already_clarified(&session));

        session
            .messages
            .push(ChatMessage::text(Role::User, "the first".to_string()));
        assert!(session_already_clarified(&session));
    }

    #[tokio::test]
    async fn routing_failure_leaves_the_turn_on_the_full_tool_surface() {
        // Retrieval is best-effort: losing it must not cost the user the turn.
        struct FailingRetrieval(MockToolExecutor);
        #[async_trait::async_trait]
        impl AgentToolExecutor for FailingRetrieval {
            async fn available_tools(&self) -> Result<Vec<ToolDefinition>, ToolError> {
                self.0.available_tools().await
            }
            async fn execute(
                &self,
                name: &str,
                args: serde_json::Value,
            ) -> Result<ToolResult, ToolError> {
                self.0.execute(name, args).await
            }
            async fn routing_available(&self) -> bool {
                true
            }
            async fn retrieve_skills(
                &self,
                _q: &str,
                _l: usize,
            ) -> Result<crate::agent_types::SkillRetrieval, ToolError> {
                Err(ToolError::ExecutionFailed("embedding down".into()))
            }
        }

        let engine = routed_engine("find notes", "search_nodes", r#"{"query":"x"}"#, "Done.");
        let loop_ = LocalAgentLoop::new(
            Arc::new(engine),
            Arc::new(FailingRetrieval(MockToolExecutor::new())),
        );
        let mut session = new_session();
        let result = loop_
            .run_turn(
                &mut session,
                "find my notes",
                |_| {},
                |_| {},
                CancellationToken::new(),
            )
            .await
            .unwrap();
        assert_eq!(result.tool_calls_made[0].name, "search_nodes");
        assert_eq!(result.response, "Done.");
    }

    #[tokio::test]
    async fn stage1_usage_is_included_in_the_turns_reported_total() {
        // The routing turn spends tokens; under-reporting them would hide the
        // cost of the extra turn from every consumer of the usage figure.
        let engine = routed_engine("find notes", "search_nodes", r#"{"query":"x"}"#, "Done.");
        let exec = RoutingToolExecutor::new(
            MockToolExecutor::new(),
            vec![skill_candidate("research", 0.9, &["search_nodes"])],
        );
        let loop_ = LocalAgentLoop::new(Arc::new(engine), Arc::new(exec));
        let mut session = new_session();
        let result = loop_
            .run_turn(
                &mut session,
                "find my notes",
                |_| {},
                |_| {},
                CancellationToken::new(),
            )
            .await
            .unwrap();
        // 8 (stage 1) + 20 + 30 from the scripted responses.
        assert_eq!(result.usage.prompt_tokens, 58);
    }
}
