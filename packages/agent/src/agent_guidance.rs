//! Guards over the local in-app agent's guidance.
//!
//! The guidance itself is seed content: the resident prompt's sections are
//! the Markdown bodies of [`crate::prompt_assembler::GUIDANCE_SEEDS`], and the
//! skills' are those of [`crate::skill_pipeline::SKILL_SEEDS`]. This module
//! holds what must stay true of that text: what the resident sections may and
//! may not say, and the eval-contamination check across every model-facing
//! guidance string.
//!
//! External PTY-spawned agent sessions ([`crate::agent_catalog::context_assembly`]) do
//! not read this guidance — they get all tool/capability guidance from
//! `packages/skill/SKILL.md`, the CLI-vocabulary companion doc.
//!
//! `N_CTX_MINIMUM` in `nlp-engine`'s `chat/mod.rs` (16,384) is sized against
//! the full tool-registered system prompt (~6,600 tokens), of which the
//! resident prose was always a minority share — the rest is JSON tool
//! schemas, untouched by any reduction here. A prose-only cut of roughly 600
//! tokens does not on its own justify lowering that floor; doing so needs a
//! live measurement of the full assembled prompt including tool schemas, not
//! just the guidance's character count.

#[cfg(test)]
mod tests {
    use crate::prompt_assembler::GUIDANCE_SEEDS;

    /// The body of the resident section titled `title`.
    fn section(title: &str) -> &'static str {
        GUIDANCE_SEEDS
            .iter()
            .find(|s| s.title == title)
            .unwrap_or_else(|| panic!("no agent-guidance section titled {title:?}"))
            .body
    }

    /// The Tool Strategy Guide: the NODE MODEL line and the TOOL STRATEGY
    /// bullets.
    fn tool_strategy() -> &'static str {
        section("Tool Strategy Guide")
    }

    #[test]
    fn the_node_model_distinguishes_a_kind_from_an_instance() {
        assert!(tool_strategy().starts_with("NODE MODEL:"));
        assert!(tool_strategy().contains("create_schema"));
        assert!(tool_strategy().contains("create_node"));
    }

    #[test]
    fn tool_strategy_rules_non_empty() {
        assert!(tool_strategy().contains("TOOL STRATEGY:"));
        assert!(tool_strategy().contains("BLAST-RADIUS GATE"));
    }

    /// Per-operation routing (which tool to call in what order for a given
    /// request shape) is now owned by skill instructions (`skill_pipeline.rs`)
    /// and tool descriptions (`local_agent/tools.rs`), not resident prose —
    /// see ADR-064 rules 2 and 5. Nothing that names a specific tool-call
    /// sequence for an operation may reappear here.
    #[test]
    fn tool_strategy_rules_contain_no_per_operation_routing() {
        for forbidden in [
            "ALWAYS search_nodes first",
            "READ-THEN-WRITE TURN COMPLETION",
            "SKILL COMPLETION",
            "NODE_TYPE COMES FROM EXISTING SCHEMAS",
            "resolve_query",
            "schema_metadata",
        ] {
            assert!(
                !tool_strategy().contains(forbidden),
                "the Tool Strategy Guide must not contain per-operation routing text {forbidden:?} — it belongs in a skill's instructions or a tool's description"
            );
        }
    }

    /// Rules that exactly duplicate a code guard in `agent_loop.rs` must stay
    /// deleted — `seen_calls` and `contains_action_claim` enforce these
    /// structurally regardless of prompt content, so restating them here
    /// would only be re-adding paid-for-every-turn tokens with no effect.
    #[test]
    fn tool_strategy_rules_do_not_restate_code_enforced_guards() {
        for forbidden in [
            "IDENTICAL TOOL CALLS",
            "NEVER CLAIM ACTION WITHOUT TOOL RESULT",
        ] {
            assert!(
                !tool_strategy().contains(forbidden),
                "the Tool Strategy Guide must not restate {forbidden:?} — agent_loop.rs's \
                 seen_calls/contains_action_claim guards already enforce this in code"
            );
        }
    }

    #[test]
    fn tool_strategy_rules_cover_meta_question_and_schema_guidance() {
        // Meta-question accuracy (confabulation fix)
        assert!(
            tool_strategy().contains("META QUESTIONS"),
            "must instruct agent to answer meta-questions from conversation history only"
        );
        assert!(
            tool_strategy().contains("tool call history"),
            "must refer to tool call history as the source of truth"
        );
        // Schema verification (hallucination fix)
        assert!(
            tool_strategy().contains("SCHEMA CLAIMS WITHOUT VERIFICATION"),
            "must prohibit unverified schema claims"
        );
    }

    #[test]
    fn node_reference_format_specifies_markdown_link() {
        let rules = section("Response Formatting Rules");
        assert!(rules.contains("[Title](nodespace://abc-123)"));
        assert!(rules.contains("Link every node you name"));
        assert!(rules.contains("no bare URI"));
        assert!(rules.contains("no backticks"));
    }

    // ------------------------------------------------------------------
    // Eval-contamination guard
    // ------------------------------------------------------------------

    /// Every guidance string that reaches the model, paired with its source
    /// for error reporting. Add new guidance sources here.
    fn guidance_corpus() -> Vec<(String, String)> {
        let mut corpus: Vec<(String, String)> = Vec::new();
        // A rule's `imperative` fragment is seeded into the DB as prompt
        // content wherever a skill body includes it, so it is guidance too.
        for r in crate::skill_rules::SCHEMA_RULES {
            corpus.push((
                "skill_rules::SCHEMA_RULES".to_string(),
                r.imperative.to_string(),
            ));
        }
        for r in crate::skill_rules::INTERACTION_RULES {
            corpus.push((
                "skill_rules::INTERACTION_RULES".to_string(),
                r.imperative.to_string(),
            ));
        }
        // Every seeded skill's markdown_content is injected as skill
        // instructions when search_skills routes to it, so it is model-facing
        // guidance and equally contaminable. Pulling these from
        // seed_skill_nodes() rather than naming constants means a skill added
        // later is covered automatically — a hand-maintained file list is
        // exactly how the skill_pipeline.rs contamination went unnoticed.
        for t in crate::skill_pipeline::seed_skill_nodes() {
            if !t.markdown_content.is_empty() {
                corpus.push((
                    "skill_pipeline::seed_skill_nodes".to_string(),
                    t.markdown_content,
                ));
            }
        }
        // The seeded prompt nodes ARE the base system prompt.
        for t in crate::prompt_assembler::PromptAssembler::seed_agent_guidance_nodes() {
            if !t.markdown_content.is_empty() {
                corpus.push((
                    "prompt_assembler::seed_agent_guidance_nodes".to_string(),
                    t.markdown_content,
                ));
            }
        }
        // Tool descriptions and their parameter-schema descriptions are
        // model-facing instructional text too — and under ADR-064 rule 1 they
        // are the channel guidance is actively being moved INTO, since argument
        // shape stated on a schema measured 100% compliance where prose did
        // not. Text pushed there was previously unguarded, so a worked example
        // built on an eval scenario could be planted in a tool description with
        // nothing failing. Derived from all_tool_definitions() rather than a
        // hand-listed set of `def_*` constants, so a tool added later is
        // covered automatically — the same property the fixture-side
        // enumeration already has.
        //
        // Both model-facing tool registries are enumerated, not just the main
        // one: Stage 1 routing offers its own three tools straight to the model
        // (`agent_loop.rs` passes `stage1_tool_definitions()` as that turn's
        // tool surface), so it is the same channel and needs the same guard.
        // Guarding only one registry would leave the very hole this
        // enumeration exists to close, one registry over.
        //
        // `all_tool_definitions()` is deliberately the unfiltered set rather
        // than `model_facing_tool_definitions()`: the latter withholds
        // `search_skills` from the local loop, but it stays model-facing to
        // external MCP agents, and over-inclusion in a guard costs nothing.
        for (registry, defs) in [
            (
                "local_agent::tools",
                crate::local_agent::tools::all_tool_definitions(),
            ),
            (
                "routing::stage1_tool_definitions",
                crate::local_agent::routing::stage1_tool_definitions(),
            ),
        ] {
            for def in defs {
                let label = format!("{registry} {}", def.name);
                if !def.description.is_empty() {
                    corpus.push((format!("{label} description"), def.description));
                }
                // Walk the schema rather than reading only top-level
                // `properties.*.description`: descriptions also sit on nested
                // object properties and array `items` (e.g. update_schema's
                // add_fields.items.properties.type), and limiting the walk to a
                // fixed depth would recreate the same structural hole this
                // enumeration exists to close.
                collect_schema_descriptions(
                    &def.parameters_schema,
                    &format!("{label} parameters_schema"),
                    &mut corpus,
                );
            }
        }
        // The agent loop injects this as a System message mid-turn, so it is
        // model-facing instruction like the rest — and it is exactly the kind
        // of text an eval-driven rewording would tune against a scenario.
        corpus.push((
            "agent_loop::ALREADY_CLARIFIED_NUDGE".to_string(),
            crate::local_agent::agent_loop::ALREADY_CLARIFIED_NUDGE.to_string(),
        ));
        corpus
    }

    /// Push every `description` string found anywhere in a JSON Schema value.
    ///
    /// Shape-agnostic on purpose: it recurses through objects and arrays
    /// without assuming where descriptions live, so nested properties, array
    /// `items`, and any schema construct added later are all covered without
    /// this function changing.
    fn collect_schema_descriptions(
        value: &serde_json::Value,
        label: &str,
        corpus: &mut Vec<(String, String)>,
    ) {
        match value {
            serde_json::Value::Object(map) => {
                for (key, child) in map {
                    if key == "description" {
                        if let Some(text) = child.as_str() {
                            if !text.is_empty() {
                                corpus.push((label.to_string(), text.to_string()));
                            }
                        }
                    }
                    collect_schema_descriptions(child, label, corpus);
                }
            }
            serde_json::Value::Array(items) => {
                for item in items {
                    collect_schema_descriptions(item, label, corpus);
                }
            }
            _ => {}
        }
    }

    /// Extract the `prompt: "..."` / `prompt: '...'` literals from an eval
    /// script. Parsing the real file (rather than keeping a copy here) is the
    /// point: a reworded scenario is picked up automatically, so this guard
    /// cannot silently go stale against the eval it protects.
    /// Returns (parsed prompts, number of `prompt:` literal sites seen). The
    /// caller compares the two: a parser that silently skips a literal shape
    /// it does not understand fails OPEN — that prompt simply goes unchecked —
    /// so coverage is asserted rather than assumed.
    fn eval_prompts(source: &str) -> (Vec<String>, usize) {
        let mut prompts = Vec::new();
        let mut sites = 0usize;
        for (idx, _) in source.match_indices("prompt:") {
            // Only treat this as an object key. `prompt:` also appears inside
            // identifiers/field access (e.g. `fixture.prompt:`) and prose.
            //
            // The BACKTICK is excluded for a distinct reason from the
            // identifier characters: a fixture's own doc comment legitimately
            // names the thing this parser looks for, as inline code
            // (`prompt:`). Without this the match is taken as a key and the
            // parser reads on to the next quote, capturing a span of comment
            // prose as one enormous "prompt". That inflates the site count and
            // is a latent false positive, since arbitrary comment text could
            // collide with guidance wording.
            //
            // Deliberately NOT extended to `"` or `'`. Those cannot appear here
            // as a key's own quote anyway — a JSON-style key writes `prompt":`,
            // with the quote between the word and the colon, so this scan never
            // matches it at all (a separate, pinned gap; see
            // `eval_prompt_parser_does_not_yet_see_a_quoted_prompt_key`).
            // Excluding them would only add a rule with nothing to exclude,
            // while making the next reader think the quoted-key shape is
            // handled.
            if source[..idx]
                .chars()
                .next_back()
                .is_some_and(|c| c.is_alphanumeric() || c == '_' || c == '.' || c == '`')
            {
                continue;
            }
            let rest = source[idx + "prompt:".len()..].trim_start();
            let quote = match rest.chars().next() {
                Some(q @ ('"' | '\'' | '`')) => q,
                // e.g. `prompt: fixture.prompt` — a reference, not a literal;
                // nothing to parse, so it is not counted as a site.
                _ => continue,
            };
            sites += 1;
            let body = &rest[quote.len_utf8()..];
            // Locate the closing quote, honoring backslash escapes so an
            // embedded \" does not truncate the literal early.
            let mut end = None;
            let mut escaped = false;
            for (i, c) in body.char_indices() {
                if escaped {
                    escaped = false;
                    continue;
                }
                match c {
                    '\\' => escaped = true,
                    c if c == quote => {
                        end = Some(i);
                        break;
                    }
                    _ => {}
                }
            }
            let Some(end) = end else { continue };
            let literal = &body[..end];
            if !literal.is_empty() {
                prompts.push(literal.to_string());
            }
        }
        (prompts, sites)
    }

    fn normalize(s: &str) -> Vec<String> {
        s.to_lowercase()
            .split(|c: char| !c.is_alphanumeric())
            .filter(|w| !w.is_empty())
            .map(|w| w.to_string())
            .collect()
    }

    /// Longest run of consecutive words shared between an eval prompt and a
    /// guidance string. Catches near-verbatim reuse (a planted example lightly
    /// reworded), not just exact substring matches.
    fn longest_shared_run(prompt_words: &[String], guidance_words: &[String]) -> (usize, String) {
        let mut best = 0usize;
        let mut best_phrase = String::new();
        for i in 0..prompt_words.len() {
            for j in 0..guidance_words.len() {
                let mut k = 0;
                while i + k < prompt_words.len()
                    && j + k < guidance_words.len()
                    && prompt_words[i + k] == guidance_words[j + k]
                {
                    k += 1;
                }
                if k > best {
                    best = k;
                    best_phrase = prompt_words[i..i + k].join(" ");
                }
            }
        }
        (best, best_phrase)
    }

    /// The agent's guidance must not contain the eval's own scenario prompts.
    ///
    /// If a prompt is planted in guidance alongside the tool call it should
    /// produce, a passing scenario proves only that the model can copy a
    /// memorized example — the eval stops measuring generalization and prompt
    /// tuning acquires a degenerate solution (add more worked examples that
    /// match more scenarios). This is trivially easy to reintroduce while
    /// tuning prompts, which is exactly when it does the most damage.
    ///
    /// Fix a failure by rewording the GUIDANCE into a rule that teaches the
    /// shape ("pass the id exactly as shown in EXISTING SCHEMAS") rather
    /// than by deleting the eval scenario.
    #[test]
    fn guidance_is_not_contaminated_by_eval_prompts() {
        // Longest run of consecutive shared words tolerated between an eval
        // prompt and a guidance rule. Ordinary instruction-following overlap
        // ("all my", "set it to") is short; a planted example is not.
        const MAX_SHARED_RUN: usize = 4;

        let repo_root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");

        // Enumerate the fixture directory rather than naming files. A
        // hand-maintained list is exactly how the earlier version of this guard
        // missed a contaminated file, and an eval added later would otherwise
        // be unguarded by default — silently, since nothing fails when a path
        // simply is not listed.
        let fixture_dir = repo_root.join("scripts/eval/fixtures");
        let mut eval_scripts: Vec<std::path::PathBuf> = std::fs::read_dir(&fixture_dir)
            .unwrap_or_else(|e| panic!("failed to read {}: {e}", fixture_dir.display()))
            .filter_map(|entry| {
                let path = entry.ok()?.path();
                let name = path.file_name()?.to_str()?;
                if !name.ends_with(".ts") || name.ends_with(".test.ts") {
                    return None;
                }
                // A fixture is a file that declares scenario prompts. Selecting
                // on that rather than on the filename means a shared helper or
                // constants module dropped in this directory is skipped, not
                // reported as parser drift — which is what a bare
                // "no prompts here" failure would look like.
                let source = std::fs::read_to_string(&path).ok()?;
                (eval_prompts(&source).1 > 0).then_some(path)
            })
            .collect();
        eval_scripts.sort();
        assert!(
            !eval_scripts.is_empty(),
            "no files declaring scenario prompts found in {} — this guard is now \
             vacuous; point it at wherever the eval scenarios moved to",
            fixture_dir.display()
        );

        let corpus = guidance_corpus();
        let tokenized: Vec<(&str, Vec<String>)> = corpus
            .iter()
            .map(|(name, text)| (name.as_str(), normalize(text)))
            .collect();

        let mut violations: Vec<String> = Vec::new();
        let mut checked = 0usize;

        for path in &eval_scripts {
            let script = path
                .strip_prefix(&repo_root)
                .unwrap_or(path)
                .display()
                .to_string();
            let source = std::fs::read_to_string(path)
                .unwrap_or_else(|e| panic!("failed to read {}: {e}", path.display()));

            let (prompts, sites) = eval_prompts(&source);
            assert!(
                !prompts.is_empty(),
                "no `prompt:` literals parsed out of {script} — the parser has \
                 drifted from the eval's format and this guard is now vacuous; \
                 fix eval_prompts() rather than deleting this assertion"
            );
            // A parser that skips a literal shape it does not understand fails
            // OPEN: the prompt goes unchecked and the guard still passes. So
            // every site it saw must have produced a prompt.
            assert_eq!(
                prompts.len(),
                sites,
                "parsed only {} of {sites} `prompt:` literals in {script} — the \
                 unparsed ones are silently exempt from this guard; teach \
                 eval_prompts() the shape it missed",
                prompts.len()
            );

            for prompt in prompts {
                checked += 1;
                let prompt_words = normalize(&prompt);
                if prompt_words.is_empty() {
                    continue;
                }
                for (const_name, guidance_words) in &tokenized {
                    let (run, phrase) = longest_shared_run(&prompt_words, guidance_words);
                    // A short prompt can be quoted in full without ever
                    // exceeding a fixed run length, so also flag any prompt
                    // reproduced in its entirety.
                    // Below three words this degenerates into "guidance
                    // mentions this word anywhere", which flags ordinary
                    // vocabulary and reports it as contamination.
                    let quoted_whole = run == prompt_words.len() && run >= 3;
                    if run > MAX_SHARED_RUN || quoted_whole {
                        let why = if quoted_whole {
                            "the ENTIRE prompt appears"
                        } else {
                            "shares a long phrase"
                        };
                        violations.push(format!(
                            "  {const_name} {why} ({run} consecutive words) from \
                             {script} prompt {prompt:?}\n      shared phrase: {phrase:?}"
                        ));
                    }
                }
            }
        }

        assert!(checked > 0, "no eval prompts were checked");
        assert!(
            violations.is_empty(),
            "agent guidance contains eval scenario prompts — the eval would be \
             measuring recall of a planted example, not generalization:\n{}\n\n\
             Reword the GUIDANCE into a rule that teaches the shape rather than \
             a memorized literal; do not weaken this test or delete the scenario.",
            violations.join("\n")
        );
    }

    /// The guard above is only meaningful if it actually fires — this pins the
    /// detector against the real contamination that motivated it (the former
    /// worked example in the resident guidance, built on scenario 6's prompt).
    #[test]
    fn contamination_guard_detects_a_planted_example() {
        let planted = normalize(
            "Example: \"Mark the $500 invoice as paid\" \
             → resolve_query(request=\"Mark the $500 invoice as paid\", node_type=\"invoice\")",
        );
        let prompt = normalize("Mark the $500 invoice as paid");
        let (run, _) = longest_shared_run(&prompt, &planted);
        assert!(
            run > 4,
            "detector failed to flag a known-contaminated example (run={run})"
        );
    }

    /// The corpus must actually contain the tool channel — otherwise the guard
    /// passes for the boring reason that it never looked. Asserts against real
    /// registry text rather than a constructed value, so a refactor that stops
    /// reaching either registry fails here.
    ///
    /// Covers Stage 1 routing as well as the main registry: both are handed
    /// straight to the model, so guarding only one would leave the same hole
    /// one registry over.
    #[test]
    fn guidance_corpus_covers_tool_descriptions_and_parameter_schemas() {
        let corpus = guidance_corpus();
        let entry_texts: Vec<&str> = corpus.iter().map(|(_, text)| text.as_str()).collect();

        let registries = [
            crate::local_agent::tools::all_tool_definitions(),
            crate::local_agent::routing::stage1_tool_definitions(),
        ];
        let mut tools_checked = 0usize;
        for def in registries.into_iter().flatten() {
            assert!(
                entry_texts.contains(&def.description.as_str()),
                "tool {}'s description is absent from guidance_corpus() — text in \
                 that channel would be unguarded against eval contamination",
                def.name
            );
            tools_checked += 1;
        }
        assert!(
            tools_checked > crate::local_agent::tools::all_tool_definitions().len(),
            "both tool registries must be checked — only the main one was, so \
             Stage 1 routing text would be unguarded"
        );

        // A nested description, not just a top-level one: update_schema states
        // its field types on add_fields.items.properties.type.
        let nested = crate::local_agent::tools::all_tool_definitions()
            .into_iter()
            .find(|d| d.name == "update_schema")
            .expect("update_schema must exist in the registry")
            .parameters_schema["properties"]["add_fields"]["items"]["properties"]["type"]
            ["description"]
            .as_str()
            .expect("update_schema add_fields item type must carry a description")
            .to_string();
        assert!(
            entry_texts.contains(&nested.as_str()),
            "a nested parameter-schema description ({nested:?}) is absent from \
             guidance_corpus() — the schema walk is not reaching nested properties"
        );
    }

    /// The schema walk must find descriptions regardless of where they sit,
    /// including shapes the current registry happens not to use yet.
    #[test]
    fn schema_description_walk_reaches_arbitrary_nesting() {
        let schema = serde_json::json!({
            "type": "object",
            "properties": {
                "top": { "type": "string", "description": "top level" },
                "list": {
                    "type": "array",
                    "description": "array itself",
                    "items": {
                        "type": "object",
                        "properties": {
                            "deep": { "type": "string", "description": "deeply nested" }
                        }
                    }
                },
                "variants": {
                    "oneOf": [
                        { "type": "string", "description": "inside an array of schemas" }
                    ]
                },
                "empty": { "type": "string", "description": "" }
            }
        });

        let mut corpus: Vec<(String, String)> = Vec::new();
        collect_schema_descriptions(&schema, "test", &mut corpus);
        let mut found: Vec<String> = corpus.into_iter().map(|(_, text)| text).collect();
        found.sort();

        assert_eq!(
            found,
            vec![
                "array itself",
                "deeply nested",
                "inside an array of schemas",
                "top level",
            ],
            "the walk must reach nested properties, array items, and schema \
             arrays, and must skip empty descriptions"
        );
    }

    /// Pins the detector against contamination planted in the tool channel
    /// specifically — the gap this corpus extension closes. Mirrors
    /// `contamination_guard_detects_a_planted_example`, but for a parameter
    /// description rather than resident prose.
    #[test]
    fn contamination_guard_detects_a_planted_example_in_a_tool_schema() {
        let schema = serde_json::json!({
            "type": "object",
            "properties": {
                "request": {
                    "type": "string",
                    "description": "The user's request, e.g. 'Mark the $500 invoice as paid'"
                }
            }
        });

        let mut corpus: Vec<(String, String)> = Vec::new();
        collect_schema_descriptions(&schema, "test", &mut corpus);
        let planted = normalize(&corpus[0].1);
        let prompt = normalize("Mark the $500 invoice as paid");

        let (run, _) = longest_shared_run(&prompt, &planted);
        assert!(
            run > 4,
            "detector failed to flag a known-contaminated parameter description (run={run})"
        );
    }

    #[test]
    fn eval_prompt_parser_extracts_literals() {
        let src = r#"
            { scenario: "a", prompt: "Hi there", expect: x },
            { scenario: "b", prompt: 'Add a book called "Dune"', expect: y },
            { scenario: "c", prompt: fixture.prompt },
        "#;
        let (got, sites) = eval_prompts(src);
        // The single-quoted literal keeps its embedded double quotes; the
        // non-literal `fixture.prompt` reference is skipped and not counted
        // as a site, so coverage stays exact.
        assert_eq!(got, vec!["Hi there", "Add a book called \"Dune\""]);
        assert_eq!(sites, got.len(), "every counted site must be parsed");
    }

    #[test]
    fn eval_prompt_parser_handles_backticks_and_escapes() {
        let src = r#"
            { prompt: `a template literal prompt` },
            { prompt: "an \"escaped\" quote inside" },
        "#;
        let (got, sites) = eval_prompts(src);
        assert_eq!(sites, 2, "both literal shapes must be counted as sites");
        assert_eq!(
            got,
            vec![
                "a template literal prompt",
                "an \\\"escaped\\\" quote inside"
            ],
            "backticked and escaped literals must parse in full, not truncate"
        );
    }

    /// A fixture's own doc comment may name `prompt:` as inline code without
    /// that mention being taken for a scenario prompt.
    ///
    /// Before the backtick exclusion, the mention below counted as a site and
    /// the parser read on to the next quote, swallowing the following prose
    /// into one enormous "prompt" — inflating the site count and leaving
    /// arbitrary comment text able to collide with guidance wording.
    #[test]
    fn eval_prompt_parser_ignores_a_backticked_mention_in_a_comment() {
        let src = r#"
            /**
             * This guard parses the `prompt:` literals out of this file, so a
             * planted example cannot masquerade as generalization.
             */
            { prompt: "the only real prompt here" },
        "#;
        let (got, sites) = eval_prompts(src);
        assert_eq!(sites, 1, "the backticked mention must not count as a site");
        assert_eq!(got, vec!["the only real prompt here"]);
    }

    /// KNOWN GAP, pinned rather than fixed: a JSON-style quoted key
    /// (`{"prompt": "..."}`) is invisible to this parser.
    ///
    /// The scan is `match_indices("prompt:")`, and a quoted key writes
    /// `prompt":` — the quote sits BETWEEN the word and the colon, so the
    /// literal never matches and the site is never even considered. This is a
    /// fail-open shape: such a prompt would be silently exempt from the
    /// contamination guard, and the `prompts.len() == sites` assertion cannot
    /// catch it because the site is not counted either.
    ///
    /// Not fixed here because widening what the scan matches is a change to
    /// the guard's own detection surface and deserves its own measurement
    /// rather than riding along with a prose fix. No fixture uses the shape
    /// today — this test is what will notice if that stops being true, since
    /// it fails the moment the parser learns the shape and someone has to
    /// consciously flip it.
    #[test]
    fn eval_prompt_parser_does_not_yet_see_a_quoted_prompt_key() {
        let (got, sites) = eval_prompts(r#"{ "prompt": "a quoted key is missed" }"#);
        assert_eq!(
            sites, 0,
            "if this now finds the site, the parser learned the quoted-key shape — \
             update this test to assert it PARSES, and drop the caveat above"
        );
        assert!(got.is_empty());
    }

    /// No model-facing channel may recommend a relationship name the validator
    /// rejects.
    ///
    /// `create_relationship` accepts a relation DECLARED on the source node's
    /// own schema, or one of `BUILTIN_RELATIONSHIP_NAMES`. Anything else is
    /// refused by `NodeService::create_relationship`.
    ///
    /// Both the tool's `relationship_type` description and the seeded
    /// Relationship Management skill previously offered `related_to` as a
    /// generic fallback. It is not built-in and is declared on no seeded
    /// schema, so a model following either instruction had its write rejected —
    /// found when the agent matrix's relationship scenario turned out to be
    /// unwinnable by construction (#1977, #2234).
    ///
    /// Keyed off `BUILTIN_RELATIONSHIP_NAMES` rather than a hardcoded list, so
    /// changing the universal set updates what this test permits — the point is
    /// that the prose and the validator cannot drift apart again, which is the
    /// failure that actually happened.
    #[test]
    fn guidance_never_recommends_a_rejected_relationship_name() {
        use nodespace_core::models::schema::BUILTIN_RELATIONSHIP_NAMES;

        // Names a reader could mistake for universal but which the validator
        // refuses on a type that does not declare them. `related_to` is the one
        // that actually shipped; the others are plausible near-misses that
        // would fail the same way.
        //
        // Matched as a bare substring, which cannot tell a recommendation from
        // a prohibition — guidance reading "never use related_to" would fail
        // this, reporting it as an offer. Kept deliberately: the guard's job is
        // to keep these names OUT of model-facing text, and prose telling a
        // small model what not to do is a weaker instrument than not naming it
        // (the resident-prompt ablation is the standing evidence). If a name
        // here later becomes legal — a universal, or one a seeded schema
        // declares — remove it from NOT_UNIVERSAL rather than loosening the
        // match; the constant pre-check below already forces that for builtins.
        const NOT_UNIVERSAL: [&str; 4] =
            ["related_to", "relates_to", "links_to", "associated_with"];

        for name in NOT_UNIVERSAL {
            assert!(
                !BUILTIN_RELATIONSHIP_NAMES.contains(&name),
                "{name} is now a built-in — remove it from this test's list rather \
                 than weakening the assertion below"
            );
        }

        let mut violations: Vec<String> = Vec::new();
        for (channel, text) in guidance_corpus() {
            for name in NOT_UNIVERSAL {
                if text.contains(name) {
                    violations.push(format!(
                        "  {channel} offers {name:?}, which create_relationship rejects \
                         unless the source node's schema declares it"
                    ));
                }
            }
        }

        assert!(
            violations.is_empty(),
            "model-facing guidance recommends a relationship name the validator \
             refuses — a model that follows it has its write rejected:\n{}\n\n\
             Legal names are a relation declared on the source node's own type, \
             or one of {BUILTIN_RELATIONSHIP_NAMES:?}. Prefer \"mentions\" as the \
             generic fallback.",
            violations.join("\n")
        );
    }
}
