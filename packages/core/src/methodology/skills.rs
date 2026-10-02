//! Seeded Playbook skills: one table row per skill, its guidance in a
//! Markdown file.
//!
//! # Adding skills to a Playbook
//!
//! A skill is a [`PlaybookSkill`] row in its Playbook's table: a fixed id, the
//! title and description retrieval ranks it by, its tool whitelist, the
//! schemas it is about, and its guidance body. The body is one plain `.md`
//! file under `skills/<playbook-id>/`, pulled in with `include_str!` and
//! seeded verbatim: what the file says is what the skill holds. The file
//! carries no metadata; that lives in the row. Files are compiled into the
//! binary, so nothing is read from disk at runtime.
//!
//! # Why files live here and not in `packages/skill/`
//!
//! These are seeded into the graph by this crate when a Playbook is installed;
//! they are not shipped to PTY agents. `packages/skill/` is that install
//! payload, so keeping Playbook skills beside the code that seeds them avoids
//! conflating the two.
//!
//! # Writing the description
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
//! So write it as a retrieval target, not a summary of the body. A description
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
//! # Linking a skill to its schemas
//!
//! `applies_to` names the schemas the skill is about, by the ids the Playbook
//! asks for. The install creates an `applies_to` edge to each one, under the
//! id it landed on, and skill search then carries exactly those schemas'
//! definitions (and their subtypes') with the skill, instead of guessing from
//! the request.
//!
//! # Shape
//!
//! Keep skills narrow and task-scoped, mirroring the built-in skills' own shape
//! rather than one broad "<Playbook> methodology" skill. They carry the
//! cross-schema narrative no per-schema description can — how types relate,
//! what the Plays do, why a write was rejected.
//!
//! The one bundle-level skill ([`crate::methodology::MethodologyPlaybook::overview`])
//! is narrow too, in its own way: it answers "what workflow is this workspace
//! using?" and points at the task-scoped skills rather than restating them.

use crate::markdown::{NodeTemplate, SeedTier};
use crate::models::SkillFields;

/// The tool whitelist of a skill that needs no more than reading and writing
/// nodes. A skill that links nodes or changes a task's status names its own.
pub const DEFAULT_TOOLS: &[&str] = &["create_node", "update_node", "search_nodes", "get_node"];

/// One Playbook skill, as its table row.
#[derive(Debug, Clone, Copy)]
pub struct PlaybookSkill {
    /// The skill node's fixed id.
    pub id: &'static str,
    /// The skill's name, embedded for retrieval with its description.
    pub title: &'static str,
    /// What the skill is for, in the words a request arrives in.
    pub description: &'static str,
    /// What the skill is not for; see *Writing the description*.
    pub exclusion: Option<&'static str>,
    /// Tools a turn that selects this skill may call.
    pub tools: &'static [&'static str],
    /// Schema ids this skill is about, as the Playbook names them.
    pub applies_to: &'static [&'static str],
    /// The guidance, as plain Markdown.
    pub body: &'static str,
}

impl PlaybookSkill {
    /// The seed this row installs.
    pub fn template(&self) -> NodeTemplate {
        let mut skill = SkillFields::new(self.description, self.tools, 3);
        if let Some(exclusion) = self.exclusion {
            skill = skill.with_exclusion(exclusion);
        }
        NodeTemplate {
            tier: SeedTier::Starter,
            ..NodeTemplate::skill(self.id, self.title, skill, self.body)
        }
    }

    /// The seed for a Playbook's bundle-level skill, with an "Installed in
    /// this workspace" section naming what `installed` records.
    ///
    /// The section is written for every install, not only a re-keyed one: the
    /// point is that the answer comes from what happened here, not from what
    /// the Playbook would have done in an empty graph.
    pub fn overview_template(&self, installed: &InstalledIds) -> NodeTemplate {
        let mut template = self.template();
        template
            .markdown_content
            .push_str(&render_installed(installed));
        template
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

#[cfg(test)]
mod tests {
    use super::*;

    const ROW: PlaybookSkill = PlaybookSkill {
        id: "4e7a2c90-1b63-4f58-8d0a-9c3e5b7f1a20",
        title: "T",
        description: "D",
        exclusion: Some("Add a task."),
        tools: &["get_node", "create_relationship"],
        applies_to: &["issue"],
        body: "# T\n\nbody\n",
    };

    #[test]
    fn a_row_seeds_a_starter_skill_under_its_id_with_its_body_verbatim() {
        let template = ROW.template();
        assert_eq!(template.id, ROW.id);
        assert_eq!(template.title, "T");
        assert_eq!(template.root_node_type, "skill");
        assert_eq!(template.markdown_content, ROW.body);
        assert!(matches!(template.tier, SeedTier::Starter));
        // ADR-057: guidance children are ordinary markdown nodes.
        assert!(template.child_node_type.is_none());

        let skill = SkillFields::from_properties(&template.root_properties)
            .expect("seed decodes as a skill");
        assert_eq!(skill.description, "D");
        assert_eq!(skill.exclusion.as_deref(), Some("Add a task."));
        assert_eq!(skill.tool_whitelist, ["get_node", "create_relationship"]);
    }

    #[test]
    fn the_overview_appends_what_the_install_landed() {
        let installed = InstalledIds {
            schemas: vec![("issue".into(), "issue".into())],
            ..Default::default()
        };
        let template = ROW.overview_template(&installed);
        assert!(template.markdown_content.starts_with(ROW.body));
        assert!(
            template
                .markdown_content
                .contains("## Installed in this workspace"),
            "{}",
            template.markdown_content
        );
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
}
