//! Seeded Playbook skills, authored as markdown files.
//!
//! # Adding skills to a Playbook
//!
//! Each skill is one `.md` file under `skills/<playbook-id>/`, pulled in with
//! `include_str!` and turned into a seed template by [`playbook_skill`]:
//!
//! ```text
//! ---
//! title: "Creating an Issue"
//! description: "Report a bug, defect, crash or something broken, open a ticket, ..."
//! exclusion: "Add a task or a reminder."
//! ---
//! # Creating an Issue
//! ...
//! ```
//!
//! The frontmatter carries the skill's title and description; everything after
//! the closing `---` line is seeded verbatim as the guidance body. Files are
//! compiled into the binary, so nothing is read from disk at runtime.
//!
//! The frontmatter is a strict subset of YAML: the two required keys `title`
//! and `description`, plus two optional ones. `tools` is a comma-separated tool
//! whitelist for a skill that needs more than [`DEFAULT_TOOLS`], such as one
//! that links nodes or changes a task's status. `exclusion` is the skill's
//! `SkillNode::exclusion` — see *Writing the description* for when a
//! Playbook skill needs one. One key per line, each value
//! double-quoted with no escapes and no embedded `"`. Anything else fails every
//! test that builds the Playbook, so a malformed file cannot ship.
//!
//! # Why files live here and not in `packages/skill/`
//!
//! These are seeded into the graph by this crate when a Playbook is installed;
//! they are not shipped to PTY agents. `packages/skill/` is that install
//! payload, so keeping Playbook skills beside the code that seeds them avoids
//! conflating the two.
//!
//! # Why the description travels with the body
//!
//! Retrieval (`skill_ops::find_skills`) is pure KNN cosine over skill ROOTS,
//! limit-capped, with the threshold at 0.0 — the cosine noise floor, not a
//! confidence cutoff (ADR-038; the 0.8 floor in the superseded ADR-030 is
//! gone, so the model judges confidence from the raw score). Two consequences
//! for anything seeded here:
//!
//!   - The markdown body is NOT indexed. Only the root's title and
//!     `description` are, so the description is the entire retrieval surface.
//!   - Nothing is filtered out; a weak description is out-RANKED. These
//!     compete directly with the built-ins, so "Creating an Issue" loses to
//!     "Node Creation" on a query like "file a bug" unless its description is
//!     written in the words a request actually arrives in.
//!
//! Keeping the description in the same file as the body makes a skill's
//! retrieval surface and its content one editable unit.
//!
//! # Writing the description
//!
//! Write it as a retrieval target, not a summary of the body. A description
//! that lists what the guidance covers ("the extended status and priority
//! vocabularies, and point estimates") reads well and matches nothing a user
//! says. Follow the built-ins' convention:
//!
//!   - Lead with the user's verbs, and their synonyms: "Report a bug, defect,
//!     crash or something broken, open a ticket, or raise an issue", not "How
//!     to create an issue".
//!   - Name the trigger in the user's own framing: "Use when the user says
//!     start the sprint, …". A gate that *rejects* a write is met as "it won't
//!     let me mark this done" or "why can't I close this" — name those, not
//!     only the system's "status change was rejected".
//!   - The title is embedded too. "Working with Cycles" alone pulled "add a
//!     task to follow up next week" away from Node Creation whatever the
//!     description said; "Sprints and Cycles" does not.
//!   - Only words for what the skill does. An embedding has no negation, so
//!     "not for plain tasks" pulls the skill onto plain-task requests.
//!   - No generic noun tail ("…a node or record"): it makes the skill an
//!     attractor for anything node-shaped.
//!
//! Where a general request sits closer to the skill than any wording can fix,
//! give it an `exclusion` naming that request positively ("Add a task or a
//! reminder.") rather than widening the description's disclaimers. It lowers
//! the skill only on queries nearer the exclusion than the description.
//!
//! Then measure it rather than argue about it: add the skill's own-intent
//! queries and the general queries it must not take to
//! `packages/agent/tests/it/live_skill_retrieval_stability.rs` (see
//! `linear_playbook_skills_win_their_own_intents` and
//! `linear_playbook_skills_do_not_displace_built_ins`), which rank the real
//! registry with the locked embedding model. The Linear skills' first drafts
//! summarised their bodies and won 7 of 17 of their own intents there.
//!
//! # Shape
//!
//! Keep skills narrow and task-scoped, mirroring the built-in skills' own shape
//! rather than one broad "<Playbook> methodology" skill. They carry the
//! cross-schema narrative no per-schema description can — how types relate,
//! what the Plays do, why a write was rejected.
//!
//! The one bundle-level skill ([`playbook_overview_skill`]) is narrow too, in
//! its own way: it answers "what workflow is this workspace using?" and points
//! at the task-scoped skills rather than restating them.
//!
//! The built-in skills in `nodespace-agent`'s `skill_pipeline` stay in Rust
//! because they interpolate shared rule constants; Playbook skills are static
//! prose and have no such reason.

use crate::markdown::{NodeTemplate, SeedTier};
use crate::models::SkillNode;

/// The tool whitelist of a skill whose frontmatter names no `tools`.
const DEFAULT_TOOLS: &[&str] = &["create_node", "update_node", "search_nodes", "get_node"];

/// A parsed skill source.
#[derive(Debug, PartialEq)]
struct Parsed<'a> {
    title: &'a str,
    description: &'a str,
    tools: Vec<&'a str>,
    exclusion: Option<&'a str>,
    body: &'a str,
}

/// Build a seed template from a Playbook skill's markdown source
/// (frontmatter + body, see the module docs).
///
/// # Panics
///
/// If the frontmatter is malformed. Sources are `include_str!` constants, so
/// this is a build-content error that the Playbook's own tests surface.
pub fn playbook_skill(source: &str) -> NodeTemplate {
    let parsed = parse(source).unwrap_or_else(|e| {
        panic!("malformed Playbook skill frontmatter: {e}");
    });
    let mut skill = SkillNode::new(parsed.title, parsed.description, &parsed.tools, 3);
    if let Some(exclusion) = parsed.exclusion {
        skill = skill.with_exclusion(exclusion);
    }
    NodeTemplate {
        tier: SeedTier::Starter,
        ..NodeTemplate::skill(skill, parsed.body)
    }
}

/// What an install created, under the ids it actually used.
///
/// Every id here is the one that landed, which differs from the Playbook's
/// own when a collision re-keyed it. Pairs are `(what the Playbook calls it,
/// id in this workspace)`: the requested schema id, a Play's name, a view's
/// name.
#[derive(Debug, Default)]
pub struct InstalledIds {
    pub schemas: Vec<(String, String)>,
    pub plays: Vec<(String, String)>,
    pub skills: Vec<String>,
    pub views: Vec<(String, String)>,
}

/// Build a Playbook's bundle-level skill (see
/// [`crate::methodology::MethodologyPlaybook::overview`]) with an
/// "Installed in this workspace" section naming what `installed` records.
///
/// The section is written for every install, not only a re-keyed one: the
/// point is that the answer comes from what happened here, not from what the
/// Playbook would have done in an empty graph.
///
/// # Panics
///
/// As [`playbook_skill`], if the source's frontmatter is malformed.
pub fn playbook_overview_skill(source: &str, installed: &InstalledIds) -> NodeTemplate {
    let mut template = playbook_skill(source);
    template
        .markdown_content
        .push_str(&render_installed(installed));
    template
}

fn render_installed(installed: &InstalledIds) -> String {
    let mut out = String::from("\n## Installed in this workspace\n");

    let types = installed.schemas.iter().map(|(requested, actual)| {
        if requested == actual {
            format!("`{actual}`")
        } else {
            format!(
                "`{actual}` — this Playbook's `{requested}`, re-keyed because `{requested}` \
                 already existed. Use `{actual}` wherever the guidance says `{requested}`."
            )
        }
    });
    push_list(&mut out, "Types", types);
    push_list(
        &mut out,
        "Plays",
        installed
            .plays
            .iter()
            .map(|(name, id)| format!("{name} (`{id}`)")),
    );
    push_list(
        &mut out,
        "Guidance skills",
        installed.skills.iter().cloned(),
    );
    push_list(
        &mut out,
        "Saved views",
        installed
            .views
            .iter()
            .map(|(name, id)| format!("{name} (`{id}`)")),
    );

    out
}

/// Append `label` and a bullet per item, or nothing when there are no items.
fn push_list(out: &mut String, label: &str, items: impl Iterator<Item = String>) {
    let mut items = items.peekable();
    if items.peek().is_none() {
        return;
    }
    out.push_str(&format!("\n{label}:\n\n"));
    for item in items {
        out.push_str(&format!("- {item}\n"));
    }
}

/// Split `source` into its frontmatter values and body.
fn parse(source: &str) -> Result<Parsed<'_>, String> {
    let rest = source
        .strip_prefix("---\n")
        .ok_or("source must start with a `---` line")?;
    let end = rest
        .find("\n---\n")
        .ok_or("no closing `---` line after the frontmatter")?;
    let (front, body) = (&rest[..end], &rest[end + "\n---\n".len()..]);

    let mut title = None;
    let mut description = None;
    let mut tools = None;
    let mut exclusion = None;
    for line in front.lines() {
        let (key, raw) = line
            .split_once(": ")
            .ok_or_else(|| format!("expected `key: \"value\"`, got {line:?}"))?;
        let value = raw
            .strip_prefix('"')
            .and_then(|v| v.strip_suffix('"'))
            .filter(|v| !v.contains(['"', '\\']))
            .ok_or_else(|| {
                format!("{key}: value must be double-quoted with no escapes or inner quotes")
            })?;
        let slot = match key {
            "title" => &mut title,
            "description" => &mut description,
            "tools" => &mut tools,
            "exclusion" => &mut exclusion,
            other => return Err(format!("unknown frontmatter key {other:?}")),
        };
        if slot.replace(value).is_some() {
            return Err(format!("duplicate frontmatter key {key:?}"));
        }
    }

    let tools = match tools {
        None => DEFAULT_TOOLS.to_vec(),
        Some(list) => {
            let names: Vec<&str> = list.split(',').map(str::trim).collect();
            if names.iter().any(|n| n.is_empty()) {
                return Err(format!("tools: empty entry in {list:?}"));
            }
            names
        }
    };

    match (title, description) {
        (Some(title), Some(description)) if !title.is_empty() && !description.is_empty() => {
            Ok(Parsed {
                title,
                description,
                tools,
                exclusion,
                body,
            })
        }
        _ => Err("both `title` and `description` are required and non-empty".to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_frontmatter_from_body_verbatim() {
        let src = "---\ntitle: \"T\"\ndescription: \"a: b\"\n---\n# T\n\n---\nbody\n";
        assert_eq!(
            parse(src),
            Ok(Parsed {
                title: "T",
                description: "a: b",
                tools: DEFAULT_TOOLS.to_vec(),
                exclusion: None,
                body: "# T\n\n---\nbody\n",
            })
        );
    }

    #[test]
    fn an_explicit_tools_list_replaces_the_default() {
        let src = "---\ntitle: \"T\"\ndescription: \"D\"\ntools: \"get_node, create_relationship\"\n---\nb";
        assert_eq!(
            parse(src).map(|p| p.tools),
            Ok(vec!["get_node", "create_relationship"])
        );
    }

    #[test]
    fn an_exclusion_reaches_the_seeded_skill() {
        let src = "---\ntitle: \"T\"\ndescription: \"D\"\nexclusion: \"Add a task.\"\n---\nb";
        let template = playbook_skill(src);
        let skill = SkillNode::from_properties(&template.title, &template.root_properties)
            .expect("seed decodes as a skill");
        assert_eq!(skill.exclusion.as_deref(), Some("Add a task."));
    }

    #[test]
    fn the_installed_section_names_the_ids_that_landed() {
        let installed = InstalledIds {
            schemas: vec![
                ("issue".into(), "issue".into()),
                ("cycle".into(), "cycle__2".into()),
            ],
            plays: vec![("Gate".into(), "gate__2".into())],
            skills: vec!["Creating an Issue".into()],
            views: vec![("Board".into(), "board".into())],
        };
        let section = render_installed(&installed);

        assert!(section.contains("- `issue`\n"), "{section}");
        assert!(
            section.contains("`cycle__2` — this Playbook's `cycle`"),
            "{section}"
        );
        assert!(section.contains("- Gate (`gate__2`)"), "{section}");
        assert!(section.contains("- Creating an Issue"), "{section}");
        assert!(section.contains("- Board (`board`)"), "{section}");
    }

    #[test]
    fn an_empty_category_is_omitted_rather_than_left_as_a_bare_label() {
        let installed = InstalledIds {
            schemas: vec![("widget".into(), "widget".into())],
            ..Default::default()
        };
        let section = render_installed(&installed);
        assert!(section.contains("Types:"), "{section}");
        assert!(!section.contains("Plays:"), "{section}");
        assert!(!section.contains("Saved views:"), "{section}");
    }

    #[test]
    fn rejects_malformed_frontmatter() {
        for src in [
            "# no frontmatter\n",
            "---\ntitle: \"T\"\ndescription: \"D\"\n# unterminated\n",
            "---\ntitle: T\ndescription: \"D\"\n---\n",
            "---\ntitle: \"T\"\n---\n",
            "---\ntitle: \"T\"\ndescription: \"D\"\nextra: \"x\"\n---\n",
            "---\ntitle: \"T\"\ntitle: \"U\"\ndescription: \"D\"\n---\n",
            "---\ntitle: \"T\"\ndescription: \"say \\\"hi\\\"\"\n---\n",
            "---\ntitle: \"T\"\ndescription: \"D\"\ntools: \"get_node,,x\"\n---\n",
        ] {
            assert!(parse(src).is_err(), "{src:?} should be rejected");
        }
    }
}
