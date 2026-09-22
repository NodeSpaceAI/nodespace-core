//! The emitter half of the `[decision ...]` marker contract, captured as a
//! golden file that the TypeScript scrape and parser are tested against.
//!
//! ## Why this test exists
//!
//! The decision marker format is a contract spanning three files in two
//! languages: `local_agent::decisions` and `agent_loop.rs` emit it,
//! `scripts/aichat.ts` scrapes the daemon log into a `[decision ...]` marker,
//! and `scripts/eval/runner.ts` parses that marker back into a record the eval
//! scores. Each site had its own tests against its own hand-written fixture
//! strings, so all three could drift while every test stayed green.
//!
//! That is not a hypothetical. It has already happened twice in this exact
//! format family, both times with passing tests on both sides:
//!
//! - `aichat.ts`'s own comments record a scrape that matched nothing for a
//!   prolonged period — it looked for a `scoped tool list` line carrying
//!   `selected_tools=`, and no such line has ever existed in the daemon. Every
//!   scored turn in the trace recorded an empty tool list.
//! - `decision_selected` was matched as a non-whitespace run, silently
//!   truncating `"Schema Creation"` to `"Schema"`. Invisible for tool names and
//!   type ids, which contain no spaces; wrong for every skill. Caught by eye on
//!   a probe run, not by a test.
//!
//! "A human notices the drift" is precisely the mechanism that failed both
//! times. This file replaces it with a mechanical one: the golden holds log
//! lines the **real** tracing layer actually emitted, and
//! `scripts/eval/decision-roundtrip.test.ts` reads that same file and asserts
//! what the real scrape and the real parser recover from it. A change to the
//! emission regenerates the golden, and the TypeScript test fails on the diff.
//!
//! ## Why the lines are captured, not written
//!
//! The golden is produced by installing a `tracing_subscriber` and running the
//! actual `tracing::info!` calls, not by formatting the expected text by hand.
//! A hand-built fixture pins the file to itself: it would keep passing after a
//! field rename, a quoting change, or a reordering, which is the entire class
//! of failure this is meant to catch.
//!
//! Time is the one thing suppressed (`.without_time()`, with a fixed stamp
//! prepended), because a wall-clock timestamp would make the golden differ on
//! every run. Everything the contract actually depends on — field names, field
//! order, quoting, escaping — is real subscriber output.
//!
//! ## Updating the golden
//!
//! Never written by a bare test failure. After a deliberate change to the
//! emission:
//!
//! ```text
//! UPDATE_GOLDEN=1 cargo test -p nodespace-agent --test decision_marker_golden
//! ```
//!
//! Then review the `git diff` and run the TypeScript side, which asserts what
//! the new lines parse back to.

use std::io::Write;
use std::sync::{Arc, Mutex};

use nodespace_agent::local_agent::decisions::{DecisionKind, DecisionRecord};

/// A `tracing` writer that accumulates into a shared buffer.
#[derive(Clone)]
struct CaptureWriter(Arc<Mutex<Vec<u8>>>);

impl Write for CaptureWriter {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0
            .lock()
            .expect("capture buffer poisoned")
            .extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for CaptureWriter {
    type Writer = CaptureWriter;

    fn make_writer(&'a self) -> Self::Writer {
        self.clone()
    }
}

/// The timestamp stamped onto every captured line.
///
/// Fixed rather than real so the golden does not differ on every run. The
/// daemon's own `fmt()` default writes an RFC-3339 stamp in this position;
/// nothing in the scrape parses it, but the golden carries one so the fixture
/// is shaped like a real log slice rather than a stripped-down variant of one.
const FIXED_STAMP: &str = "2026-09-22T10:00:00.000000Z";

/// Emit `records` through the real tracing layer and return the captured lines.
///
/// Mirrors `agent_loop.rs`'s three emission sites: same field names, same
/// order, same `%` Display formatter on the payload. The message text varies
/// per kind exactly as it does there, because `aichat.ts` filters on the
/// literal substring `"Agent decision:"`.
fn capture(records: &[(u32, DecisionRecord)]) -> String {
    let buf = Arc::new(Mutex::new(Vec::new()));
    let writer = CaptureWriter(Arc::clone(&buf));

    let subscriber = tracing_subscriber::fmt()
        .with_writer(writer)
        .with_ansi(false)
        // See FIXED_STAMP: a real clock would make this golden unstable.
        .without_time()
        .with_target(true)
        .finish();

    tracing::subscriber::with_default(subscriber, || {
        for (iteration, rec) in records {
            let iteration = *iteration;
            // One call per kind rather than a loop over a formatted message,
            // because `tracing::info!`'s message is a literal at the macro site
            // in `agent_loop.rs` too. Keeping that shape means the captured
            // line differs from production only in its timestamp.
            match rec.kind {
                DecisionKind::Skill => tracing::info!(
                    iteration,
                    decision = rec.kind.as_str(),
                    decision_payload = %rec.payload_field(),
                    "Agent decision: skill selected"
                ),
                DecisionKind::Operation => tracing::info!(
                    iteration,
                    decision = rec.kind.as_str(),
                    decision_payload = %rec.payload_field(),
                    "Agent decision: operation selected"
                ),
                DecisionKind::Schema => tracing::info!(
                    iteration,
                    decision = rec.kind.as_str(),
                    decision_payload = %rec.payload_field(),
                    "Agent decision: schema selected"
                ),
            }
        }
    });

    let captured = buf.lock().expect("capture buffer poisoned").clone();
    let text = String::from_utf8(captured).expect("tracing wrote non-UTF-8");

    // Prepend the fixed stamp and rewrite the target so each line matches the
    // daemon's shape. The target is the test binary's name here but
    // `nodespace_agent` in production; nothing in the scrape parses it, and
    // normalising it keeps the fixture readable as the real log slice it
    // stands in for rather than a test artefact.
    text.lines()
        .map(|l| {
            format!(
                "{FIXED_STAMP} {}",
                l.replacen("decision_marker_golden:", "nodespace_agent:", 1)
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
        + "\n"
}

fn rec(kind: DecisionKind, candidates: &[&str], selected: Option<&str>) -> DecisionRecord {
    DecisionRecord {
        kind,
        candidates: candidates.iter().map(|c| c.to_string()).collect(),
        selected: selected.map(str::to_string),
    }
}

/// The golden set: one line per decision shape the contract has to carry.
///
/// Deliberately includes the adversarial names the delimited format could not
/// represent. Each is reachable rather than contrived — `create_schema`
/// derives type ids from the model's own phrasing, and skill names are
/// user-authorable in principle.
#[test]
fn emitted_decision_lines_match_golden() {
    let lines = capture(&[
        // The ordinary shapes, so a regression in the common path is visible
        // in the same diff as one in the adversarial path.
        (
            0,
            rec(
                DecisionKind::Skill,
                &["Schema Creation", "Node Creation"],
                Some("Schema Creation"),
            ),
        ),
        (
            0,
            rec(
                DecisionKind::Operation,
                &["search_nodes", "create_node"],
                Some("create_node"),
            ),
        ),
        // Off-menu: the most diagnostic signal the record carries.
        (
            0,
            rec(
                DecisionKind::Schema,
                &["invoice", "customer"],
                Some("album"),
            ),
        ),
        // A name carrying the old join string. Under the delimited format this
        // became two candidates and the selection read as off-menu.
        (
            1,
            rec(
                DecisionKind::Schema,
                &["Company, Sold To", "invoice"],
                Some("Company, Sold To"),
            ),
        ),
        // A name carrying a quote. tracing quoted a field only when it held a
        // space, so neither of the scrape's two patterns survived this.
        (
            1,
            rec(
                DecisionKind::Skill,
                &["a \" quote", "plain"],
                Some("a \" quote"),
            ),
        ),
        // A name carrying a newline. Verbatim Display would have split the log
        // line in two and truncated the record at the break.
        (
            1,
            rec(
                DecisionKind::Schema,
                &["a \n newline", "invoice"],
                Some("a \n newline"),
            ),
        ),
        // No selection: the model was offered tools and called none. Encodes
        // as JSON null.
        (2, rec(DecisionKind::Operation, &["search_nodes"], None)),
        // An EMPTY selection, which is not the same outcome as no selection
        // and must not decode back to null.
        (2, rec(DecisionKind::Schema, &["invoice"], Some(""))),
        // Nothing on offer at all.
        (2, rec(DecisionKind::Operation, &[], None)),
    ]);

    // One line per record, asserted before the golden comparison rather than
    // left implicit in the fixture's shape. The newline-bearing name above is
    // the case that matters: `split("\n")` is the first thing the scrape does
    // to a log slice, so a verbatim newline would truncate that record at the
    // break and silently drop the rest of the line.
    assert_eq!(
        lines.trim_end().lines().count(),
        9,
        "each decision must emit exactly one line, including the record whose \
         candidate name contains a newline:\n{lines}"
    );

    golden::assert_matches("decision_markers.golden", &lines);
}

// ---------------------------------------------------------------------------
// Golden comparison harness
// ---------------------------------------------------------------------------
//
// Mirrors `prompt_assembly_snapshot.rs`'s harness, which is the established
// convention in this crate: an explicit env-gated write, a hard failure on a
// missing file, and a line diff on mismatch.

mod golden {
    use std::path::{Path, PathBuf};

    fn dir() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/golden/decision_markers")
    }

    /// Gated on an env var read at the write site — never inferred from "the
    /// assertion is about to fail". A bare `cargo test` never writes.
    fn update_requested() -> bool {
        std::env::var("UPDATE_GOLDEN").ok().as_deref() == Some("1")
    }

    pub fn assert_matches(name: &str, actual: &str) {
        let file = dir().join(name);

        if update_requested() {
            std::fs::create_dir_all(dir()).expect("create tests/golden/decision_markers");
            std::fs::write(&file, actual)
                .unwrap_or_else(|e| panic!("failed to write golden {}: {e}", file.display()));
            eprintln!(
                "UPDATE_GOLDEN=1: wrote {} ({} bytes) — review with `git diff`, then run \
                 the TypeScript side (bun run test:scripts) which parses this file",
                file.display(),
                actual.len()
            );
            return;
        }

        let expected = std::fs::read_to_string(&file).unwrap_or_else(|e| {
            panic!(
                "golden file missing or unreadable at {} ({e}).\n\n\
                 Goldens are never auto-created. If this change to the decision \
                 emission is deliberate, regenerate it explicitly:\n\n  \
                 UPDATE_GOLDEN=1 cargo test -p nodespace-agent --test decision_marker_golden\n\n\
                 then review the diff AND run the TypeScript round-trip test, which \
                 asserts what these lines parse back to.",
                file.display()
            )
        });

        if actual == expected {
            return;
        }

        panic!("{}", render_diff(&file, &expected, actual));
    }

    fn render_diff(file: &Path, expected: &str, actual: &str) -> String {
        use similar::ChangeTag;

        let diff = similar::TextDiff::from_lines(expected, actual);
        let mut out = format!(
            "decision marker emission drift detected ({})\n\n\
             This format is a contract with scripts/aichat.ts and \
             scripts/eval/runner.ts. If this change is deliberate, regenerate \
             with UPDATE_GOLDEN=1 and check the TypeScript round-trip test still \
             passes — it parses this exact file.\n\n",
            file.display()
        );
        for group in diff.grouped_ops(3) {
            for op in group {
                for change in diff.iter_changes(&op) {
                    let sign = match change.tag() {
                        ChangeTag::Delete => '-',
                        ChangeTag::Insert => '+',
                        ChangeTag::Equal => ' ',
                    };
                    out.push_str(&format!("{sign}{change}"));
                }
            }
        }
        out
    }
}
