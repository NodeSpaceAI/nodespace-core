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
//! description: "How to create an issue in a Linear-style workspace: ..."
//! ---
//! # Creating an Issue
//! ...
//! ```
//!
//! The frontmatter carries the skill's title and description; everything after
//! the closing `---` line is seeded verbatim as the guidance body. Files are
//! compiled into the binary, so nothing is read from disk at runtime.
//!
//! The frontmatter is a strict subset of YAML: exactly the two keys `title`
//! and `description`, one per line, each value double-quoted with no escapes
//! and no embedded `"`. Anything else fails every test that builds the Playbook,
//! so a malformed file cannot ship.
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
//! confidence cutoff (ADR-038 describes an 0.8 floor; the code deliberately
//! moved past that so the model judges confidence from the raw score). Two
//! consequences for anything seeded here:
//!
//!   - The markdown body is NOT indexed. Only the root's title and
//!     `description` are, so the description is the entire retrieval surface.
//!   - Nothing is filtered out; a weak description is out-RANKED. These
//!     compete directly with the 11 built-ins, so "Creating an Issue" loses to
//!     "Node Creation" on a query like "file a bug" unless its description is
//!     written in the words a request actually arrives in.
//!
//! Keeping the description in the same file as the body makes a skill's
//! retrieval surface and its content one editable unit.
//!
//! # Shape
//!
//! Keep skills narrow and task-scoped, mirroring the built-in skills' own shape
//! rather than one broad "<Playbook> methodology" skill. They carry the
//! cross-schema narrative no per-schema description can — how types relate,
//! what the Plays do, why a write was rejected.
//!
//! The built-in skills in `nodespace-agent`'s `skill_pipeline` stay in Rust
//! because they interpolate shared rule constants; Playbook skills are static
//! prose and have no such reason.

use crate::markdown::{NodeTemplate, SeedTier};

/// Build a seed template from a Playbook skill's markdown source
/// (frontmatter + body, see the module docs).
///
/// # Panics
///
/// If the frontmatter is malformed. Sources are `include_str!` constants, so
/// this is a build-content error that the Playbook's own tests surface.
pub fn playbook_skill(source: &str) -> NodeTemplate {
    let (title, description, body) = parse(source).unwrap_or_else(|e| {
        panic!("malformed Playbook skill frontmatter: {e}");
    });
    NodeTemplate {
        title: title.to_string(),
        content: None,
        markdown_content: body.to_string(),
        root_node_type: "skill".to_string(),
        root_properties: serde_json::json!({
            "description": description,
            "tool_whitelist": ["create_node", "update_node", "search_nodes", "get_node"],
            "max_iterations": 3,
        }),
        child_node_type: None,
        child_properties: None,
        tier: SeedTier::Starter,
    }
}

/// Split `source` into `(title, description, body)`.
fn parse(source: &str) -> Result<(&str, &str, &str), String> {
    let rest = source
        .strip_prefix("---\n")
        .ok_or("source must start with a `---` line")?;
    let end = rest
        .find("\n---\n")
        .ok_or("no closing `---` line after the frontmatter")?;
    let (front, body) = (&rest[..end], &rest[end + "\n---\n".len()..]);

    let mut title = None;
    let mut description = None;
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
            other => return Err(format!("unknown frontmatter key {other:?}")),
        };
        if slot.replace(value).is_some() {
            return Err(format!("duplicate frontmatter key {key:?}"));
        }
    }

    match (title, description) {
        (Some(t), Some(d)) if !t.is_empty() && !d.is_empty() => Ok((t, d, body)),
        _ => Err("both `title` and `description` are required and non-empty".to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_frontmatter_from_body_verbatim() {
        let src = "---\ntitle: \"T\"\ndescription: \"a: b\"\n---\n# T\n\n---\nbody\n";
        assert_eq!(parse(src), Ok(("T", "a: b", "# T\n\n---\nbody\n")));
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
        ] {
            assert!(parse(src).is_err(), "{src:?} should be rejected");
        }
    }
}
