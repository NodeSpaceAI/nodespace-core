//! `nodespace seed ...` — review the shipped changes to built-in items the
//! user has edited (ADR-094 §8).
//!
//! NodeSpace ships skills, plays, saved queries and other items as seeded
//! nodes, and never overwrites one a user edited. When a newer version of
//! such an item ships, it is held back as pending. These commands list what is
//! pending, show the shipped version beside the user's, and settle one item:
//! keep the user's version, or take the shipped one. Nothing is replaced
//! without `take`.

use anyhow::{Context, Result};
use clap::{Args, Subcommand};
use nodespace_daemon::nodespace::{
    ListPendingSeedUpdatesRequest, PendingSeedUpdate, PendingSeedUpdateDetail,
    PendingSeedUpdateRef, ResolvePendingSeedUpdateRequest, SeedUpdateChoice,
};
use serde_json::json;
use std::io::{IsTerminal, Write};

use super::skill::sanitize_for_terminal;
use crate::NodeClient;

#[derive(Subcommand, Debug)]
pub enum SeedAction {
    /// List the built-in items you have edited that have a newer shipped
    /// version: kind, title, which part (config or guidance), and when you
    /// last edited it.
    Pending,
    /// Show one pending item's shipped version and your version.
    Show(ItemArgs),
    /// Replace your version of one part of one item with the shipped version.
    /// Discards your edit to that part; asks for confirmation unless `--yes`
    /// is passed.
    Take(TakeArgs),
    /// Keep your version of one part of one item. It stops being pending
    /// until the shipped version changes again.
    Keep(ItemArgs),
}

#[derive(Args, Debug)]
pub struct ItemArgs {
    /// The item's node id, or its exact title as `nodespace seed pending`
    /// lists it.
    pub item: String,

    #[command(flatten)]
    pub aspect: AspectArgs,
}

#[derive(Args, Debug)]
pub struct TakeArgs {
    #[command(flatten)]
    pub item: ItemArgs,

    /// Take the shipped version without prompting. Required when there is no
    /// interactive terminal: taking discards an edit, so it is never done
    /// unattended without this flag.
    #[arg(long)]
    pub yes: bool,
}

/// Which part of the item. Needed only when both parts of it are pending.
#[derive(Args, Debug, Default)]
#[group(multiple = false)]
pub struct AspectArgs {
    /// The item's config: its name and its fields.
    #[arg(long)]
    pub config: bool,

    /// The item's guidance: its body.
    #[arg(long)]
    pub guidance: bool,
}

impl AspectArgs {
    fn named(&self) -> Option<&'static str> {
        if self.config {
            Some("config")
        } else if self.guidance {
            Some("guidance")
        } else {
            None
        }
    }
}

pub async fn run(client: &mut NodeClient, action: SeedAction, json_out: bool) -> Result<()> {
    match action {
        SeedAction::Pending => {
            let updates = list(client).await?;
            print_pending(&mut std::io::stdout(), &updates, json_out)
        }
        SeedAction::Show(args) => {
            let update = find(client, &args).await?;
            let detail = fetch_detail(client, &update).await?;
            print_detail(&mut std::io::stdout(), &detail, json_out)
        }
        SeedAction::Take(args) => {
            let update = find(client, &args.item).await?;
            if !args.yes {
                let detail = fetch_detail(client, &update).await?;
                if !confirm_take(&take_summary(&detail))? {
                    println!("Skipped.");
                    return Ok(());
                }
            }
            resolve(client, &update, SeedUpdateChoice::TakeShipped).await?;
            print_resolved(&mut std::io::stdout(), &update, "took_shipped", json_out)
        }
        SeedAction::Keep(args) => {
            let update = find(client, &args).await?;
            resolve(client, &update, SeedUpdateChoice::KeepMine).await?;
            print_resolved(&mut std::io::stdout(), &update, "kept_mine", json_out)
        }
    }
}

async fn list(client: &mut NodeClient) -> Result<Vec<PendingSeedUpdate>> {
    Ok(client
        .list_pending_seed_updates(ListPendingSeedUpdatesRequest {})
        .await
        .context("ListPendingSeedUpdates RPC failed")?
        .into_inner()
        .updates)
}

/// The one pending update `args` names, from the daemon's list.
async fn find(client: &mut NodeClient, args: &ItemArgs) -> Result<PendingSeedUpdate> {
    let updates = list(client).await?;
    select(&updates, &args.item, args.aspect.named()).cloned()
}

/// Pick the pending update `item` names: a node id, or an exact title. An id
/// wins over a title, so an item whose title is another's id cannot shadow it.
fn select<'a>(
    updates: &'a [PendingSeedUpdate],
    item: &str,
    aspect: Option<&str>,
) -> Result<&'a PendingSeedUpdate> {
    let by_id: Vec<&PendingSeedUpdate> = updates.iter().filter(|u| u.node_id == item).collect();
    let named = if by_id.is_empty() {
        updates.iter().filter(|u| u.title == item).collect()
    } else {
        by_id
    };
    if named.is_empty() {
        anyhow::bail!(
            "No shipped update is pending for \"{item}\". `nodespace seed pending` lists what is."
        );
    }

    let matching: Vec<&PendingSeedUpdate> = named
        .iter()
        .copied()
        .filter(|u| aspect.is_none_or(|aspect| u.aspect == aspect))
        .collect();
    match matching.as_slice() {
        [only] => Ok(only),
        [] => anyhow::bail!(
            "The {} of \"{item}\" has no shipped update pending.",
            aspect.unwrap_or_default()
        ),
        several => {
            let mut nodes: Vec<&str> = several.iter().map(|u| u.node_id.as_str()).collect();
            nodes.dedup();
            if nodes.len() > 1 {
                anyhow::bail!(
                    "More than one pending item is titled \"{item}\". Name it by node id: {}.",
                    nodes.join(", ")
                );
            }
            anyhow::bail!(
                "Both the config and the guidance of \"{item}\" are pending. Pass --config or \
                 --guidance."
            )
        }
    }
}

async fn fetch_detail(
    client: &mut NodeClient,
    update: &PendingSeedUpdate,
) -> Result<PendingSeedUpdateDetail> {
    Ok(client
        .get_pending_seed_update(PendingSeedUpdateRef {
            node_id: update.node_id.clone(),
            aspect: update.aspect.clone(),
        })
        .await
        .map_err(|status| anyhow::anyhow!("{}", status.message()))?
        .into_inner())
}

async fn resolve(
    client: &mut NodeClient,
    update: &PendingSeedUpdate,
    choice: SeedUpdateChoice,
) -> Result<()> {
    client
        .resolve_pending_seed_update(ResolvePendingSeedUpdateRequest {
            node_id: update.node_id.clone(),
            aspect: update.aspect.clone(),
            choice: choice as i32,
        })
        .await
        .map_err(|status| anyhow::anyhow!("{}", status.message()))?;
    Ok(())
}

fn update_json(update: &PendingSeedUpdate) -> serde_json::Value {
    json!({
        "node_id": update.node_id,
        "kind": update.node_type,
        "title": update.title,
        "aspect": update.aspect,
        "shipped_version": update.shipped_version,
        "recorded_at": update.recorded_at,
        "last_edited_at": update.last_edited_at,
    })
}

fn print_pending(w: &mut impl Write, updates: &[PendingSeedUpdate], json_out: bool) -> Result<()> {
    if json_out {
        let value = json!({
            "count": updates.len(),
            "updates": updates.iter().map(update_json).collect::<Vec<_>>(),
        });
        writeln!(w, "{}", serde_json::to_string_pretty(&value)?)?;
        return Ok(());
    }

    if updates.is_empty() {
        writeln!(w, "No shipped updates are pending.")?;
        return Ok(());
    }

    writeln!(
        w,
        "{} shipped update(s) to built-in items you have edited. Yours are kept until you choose:",
        updates.len()
    )?;
    for update in updates {
        writeln!(w)?;
        writeln!(w, "kind:        {}", sanitize_for_terminal(&update.node_type))?;
        writeln!(w, "title:       {}", sanitize_for_terminal(&update.title))?;
        writeln!(w, "aspect:      {}", update.aspect)?;
        writeln!(w, "last edited: {}", update.last_edited_at)?;
        writeln!(w, "node:        {}", update.node_id)?;
    }
    writeln!(w)?;
    writeln!(
        w,
        "Compare with `nodespace seed show <node>`, then `nodespace seed keep <node>` or \
         `nodespace seed take <node>`."
    )?;
    Ok(())
}

fn print_detail(w: &mut impl Write, detail: &PendingSeedUpdateDetail, json_out: bool) -> Result<()> {
    let update = detail
        .update
        .as_ref()
        .context("daemon returned no pending update")?;
    if json_out {
        let mut value = update_json(update);
        value["shipped"] = json!(detail.shipped);
        value["yours"] = json!(detail.yours);
        writeln!(w, "{}", serde_json::to_string_pretty(&value)?)?;
        return Ok(());
    }

    writeln!(
        w,
        "{} \"{}\" ({}), last edited {}",
        sanitize_for_terminal(&update.node_type),
        sanitize_for_terminal(&update.title),
        update.aspect,
        update.last_edited_at
    )?;
    writeln!(w)?;
    writeln!(w, "=== SHIPPED ===")?;
    writeln!(w, "{}", sanitize_for_terminal(&detail.shipped))?;
    writeln!(w)?;
    writeln!(w, "=== YOURS ===")?;
    writeln!(w, "{}", sanitize_for_terminal(&detail.yours))?;
    Ok(())
}

fn print_resolved(
    w: &mut impl Write,
    update: &PendingSeedUpdate,
    choice: &str,
    json_out: bool,
) -> Result<()> {
    if json_out {
        let mut value = update_json(update);
        value["choice"] = json!(choice);
        writeln!(w, "{}", serde_json::to_string_pretty(&value)?)?;
        return Ok(());
    }
    let title = sanitize_for_terminal(&update.title);
    if choice == "took_shipped" {
        writeln!(
            w,
            "✓ The {} of \"{title}\" is now the shipped version.",
            update.aspect
        )?;
    } else {
        writeln!(
            w,
            "✓ Kept your {} of \"{title}\". It will be listed again when the shipped version \
             next changes.",
            update.aspect
        )?;
    }
    Ok(())
}

fn take_summary(detail: &PendingSeedUpdateDetail) -> String {
    let (title, aspect) = detail
        .update
        .as_ref()
        .map(|u| (sanitize_for_terminal(&u.title), u.aspect.clone()))
        .unwrap_or_default();
    format!(
        "About to replace your {aspect} of \"{title}\" with the shipped version. Yours, which \
         will be discarded:\n\n{}\n\nThis cannot be undone.",
        sanitize_for_terminal(&detail.yours)
    )
}

/// Prompt on a real terminal; refuse, not proceed, without one. Taking the
/// shipped version discards an edit, so it follows `skill reset`: a script
/// that means it passes `--yes`. The prompt goes to stderr so a `--json`
/// result on stdout is never interleaved with it.
fn confirm_take(summary: &str) -> Result<bool> {
    if !std::io::stdin().is_terminal() || !std::io::stdout().is_terminal() {
        anyhow::bail!(
            "No interactive terminal detected -- refusing to discard an edit without \
             confirmation. Pass --yes to take the shipped version non-interactively.\n\n{summary}"
        );
    }

    eprintln!("{summary}");
    eprint!("Proceed? [y/N] ");
    std::io::stderr().flush().ok();

    let mut reply = String::new();
    std::io::stdin()
        .read_line(&mut reply)
        .context("Failed to read confirmation from stdin")?;
    let reply = reply.trim().to_ascii_lowercase();
    Ok(reply == "y" || reply == "yes")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn update(node_id: &str, title: &str, aspect: &str) -> PendingSeedUpdate {
        PendingSeedUpdate {
            node_id: node_id.to_string(),
            node_type: "skill".to_string(),
            title: title.to_string(),
            aspect: aspect.to_string(),
            shipped_version: "abc123".to_string(),
            recorded_at: "2026-10-01T00:00:00+00:00".to_string(),
            last_edited_at: "2026-09-20T12:00:00+00:00".to_string(),
        }
    }

    #[test]
    fn pending_lists_kind_title_aspect_and_last_edit() {
        let updates = [update("n1", "Research & Search", "guidance")];

        let mut buf = Vec::new();
        print_pending(&mut buf, &updates, false).unwrap();
        let out = String::from_utf8(buf).unwrap();
        for expected in [
            "kind:        skill",
            "title:       Research & Search",
            "aspect:      guidance",
            "last edited: 2026-09-20T12:00:00+00:00",
        ] {
            assert!(out.contains(expected), "{out}");
        }

        let mut buf = Vec::new();
        print_pending(&mut buf, &updates, true).unwrap();
        let value: serde_json::Value = serde_json::from_slice(&buf).unwrap();
        assert_eq!(value["count"], 1);
        assert_eq!(
            value["updates"][0],
            json!({
                "node_id": "n1",
                "kind": "skill",
                "title": "Research & Search",
                "aspect": "guidance",
                "shipped_version": "abc123",
                "recorded_at": "2026-10-01T00:00:00+00:00",
                "last_edited_at": "2026-09-20T12:00:00+00:00",
            })
        );
    }

    #[test]
    fn pending_says_so_when_nothing_is() {
        let mut buf = Vec::new();
        print_pending(&mut buf, &[], false).unwrap();
        assert_eq!(
            String::from_utf8(buf).unwrap(),
            "No shipped updates are pending.\n"
        );
    }

    #[test]
    fn show_prints_the_shipped_version_and_the_users() {
        let detail = PendingSeedUpdateDetail {
            update: Some(update("n1", "Research & Search", "guidance")),
            shipped: "Search first.".to_string(),
            yours: "Search \u{1B}[8mlast.".to_string(),
        };

        let mut buf = Vec::new();
        print_detail(&mut buf, &detail, false).unwrap();
        let out = String::from_utf8(buf).unwrap();
        let shipped = out.find("=== SHIPPED ===\nSearch first.").expect(&out);
        let yours = out.find("=== YOURS ===\nSearch last.").expect(&out);
        assert!(shipped < yours, "{out}");
        // The user's text is graph data: an escape in it stays off the terminal.
        assert!(!out.contains('\u{1B}'), "{out:?}");

        let mut buf = Vec::new();
        print_detail(&mut buf, &detail, true).unwrap();
        let value: serde_json::Value = serde_json::from_slice(&buf).unwrap();
        assert_eq!(value["shipped"], "Search first.");
        assert_eq!(value["yours"], "Search \u{1B}[8mlast.");
        assert_eq!(value["aspect"], "guidance");
    }

    #[test]
    fn an_item_is_named_by_id_or_by_title() {
        let updates = [
            update("n1", "Research & Search", "guidance"),
            update("n2", "Node Creation", "config"),
        ];
        assert_eq!(select(&updates, "n2", None).unwrap().node_id, "n2");
        assert_eq!(
            select(&updates, "Research & Search", None).unwrap().node_id,
            "n1"
        );
        let err = select(&updates, "Nothing", None).unwrap_err().to_string();
        assert!(err.contains("nodespace seed pending"), "{err}");
    }

    #[test]
    fn an_item_with_both_parts_pending_needs_the_part_named() {
        let updates = [
            update("n1", "Research & Search", "guidance"),
            update("n1", "Research & Search", "config"),
        ];
        let err = select(&updates, "n1", None).unwrap_err().to_string();
        assert!(err.contains("--config or --guidance"), "{err}");
        assert_eq!(
            select(&updates, "n1", Some("config")).unwrap().aspect,
            "config"
        );

        let one = [update("n1", "Research & Search", "guidance")];
        let err = select(&one, "n1", Some("config")).unwrap_err().to_string();
        assert!(err.contains("config of \"n1\""), "{err}");
    }

    #[test]
    fn two_items_sharing_a_title_are_named_by_id() {
        let updates = [
            update("n1", "Daily Review", "config"),
            update("n2", "Daily Review", "config"),
        ];
        let err = select(&updates, "Daily Review", None)
            .unwrap_err()
            .to_string();
        assert!(err.contains("n1, n2"), "{err}");
    }

    #[test]
    fn the_part_flags_are_exclusive() {
        use clap::Parser;
        #[derive(Parser, Debug)]
        struct Harness {
            #[command(subcommand)]
            action: SeedAction,
        }
        assert!(Harness::try_parse_from(["seed", "take", "n1", "--config", "--guidance"]).is_err());
        assert!(Harness::try_parse_from(["seed", "take", "n1", "--config", "--yes"]).is_ok());
        assert!(Harness::try_parse_from(["seed", "keep", "n1"]).is_ok());
    }
}
