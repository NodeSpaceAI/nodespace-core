//! `nodespace logs` — read the daemon's log.
//!
//! Play execution errors (a failed action, a cycle-limit breach, a rule that
//! would not compile) are operational telemetry rather than knowledge, so they
//! go to the daemon log rather than becoming nodes. This is how you read them
//! back.
//!
//! Local-only: the log is a file on this machine, written by whichever
//! supervisor started the daemon, so there is nothing to ask the daemon for —
//! and asking would fail in exactly the case you most want the log, namely a
//! daemon that will not start.
//!
//! The path differs by install method, which is the whole reason this verb
//! exists: telling a user to `grep ~/.nodespace/logs/...` is wrong for a
//! Homebrew install, and a caller has no reliable way to know which they have.
//!
//! # Bounded-memory reading
//!
//! A headless (Homebrew/systemd) install never rotates its log — rotation is
//! implemented only in the Tauri desktop app's supervisor (see
//! `daemon_setup::rotate_daemon_logs`'s own doc comment for why: on every
//! platform the log files are the daemon's inherited stdio, owned by the
//! service manager, not something the daemon itself opens or could safely
//! reopen). A long-lived headless daemon can therefore accumulate a log file
//! of unbounded size, so this command must never load the whole file into
//! memory: [`tail_matching_lines`] scans backward from the end in fixed-size
//! chunks, stopping as soon as it has found `--lines` matches, and
//! [`count_matching_lines`] streams forward one line at a time to report the
//! total match count. Neither ever holds more than a small, bounded amount of
//! the file in memory at once.

use anyhow::{bail, Result};
use clap::Args;
use std::fs::File;
use std::io::{BufRead, BufReader, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

#[derive(Args, Debug)]
pub struct LogsArgs {
    /// Show only lines containing this text — a play id, a rule name, an
    /// error type. Matched literally, not as a regex.
    #[arg(long)]
    pub filter: Option<String>,

    /// How many matching lines to show, most recent last.
    #[arg(long, default_value_t = 50)]
    pub lines: usize,

    /// Print the resolved log file path and exit without reading it.
    #[arg(long)]
    pub path_only: bool,
}

/// Candidate log locations, in the order they are checked.
///
/// The desktop app's supervisor writes under the user's home; a Homebrew
/// service writes under the prefix. Both are real and neither is discoverable
/// from the other, so the resolution is "first one that exists".
///
/// The home log is under the state directory the daemon's own resolver gives,
/// so it follows `NODESPACE_HOME`. A redirected home holds all NodeSpace
/// state, so its log is then the only candidate: an isolated run must not
/// report another install's log as its own.
fn candidate_paths() -> Vec<PathBuf> {
    candidate_paths_for(
        nodespace_daemon::nodespace_dir().ok(),
        nodespace_daemon::nodespace_home_override().is_some(),
    )
}

/// [`candidate_paths`] for the given state directory (`None` when no home can
/// be found), `redirected` when `NODESPACE_HOME` chose it.
fn candidate_paths_for(state_dir: Option<PathBuf>, redirected: bool) -> Vec<PathBuf> {
    let mut candidates: Vec<PathBuf> = state_dir
        .into_iter()
        .map(|dir| dir.join("logs").join("nodespaced.log"))
        .collect();
    if redirected {
        return candidates;
    }

    // Homebrew's `var` lives under the prefix, which differs by architecture.
    for prefix in ["/opt/homebrew", "/usr/local"] {
        candidates.push(
            PathBuf::from(prefix)
                .join("var")
                .join("log")
                .join("nodespace")
                .join("nodespaced.log"),
        );
    }

    candidates
}

fn resolve_log_path() -> Option<PathBuf> {
    candidate_paths().into_iter().find(|p| p.exists())
}

/// Whether `line` satisfies `filter` — a literal substring match, or every
/// line when no filter was given. Shared by [`count_matching_lines`] and
/// [`tail_matching_lines`] so the two passes can never disagree about what
/// counts as a match.
fn matches_filter(line: &str, filter: Option<&str>) -> bool {
    match filter {
        Some(f) => line.contains(f),
        None => true,
    }
}

/// Count how many lines in `path` satisfy `filter`, streaming the file
/// forward one line at a time instead of reading it whole. Memory use is
/// bounded by a single line's length regardless of the file's total size.
///
/// Decodes each raw line lossily (`String::from_utf8_lossy`), matching
/// [`tail_matching_lines_with_chunk`]'s own decoding, rather than
/// `BufRead::lines()` (which would hard-error the whole command on the first
/// invalid UTF-8 byte anywhere in the file). A long-lived, never-rotated
/// headless log — exactly the case this command exists to serve — is exactly
/// the kind of file likely to eventually contain one.
fn count_matching_lines(path: &Path, filter: Option<&str>) -> std::io::Result<usize> {
    let mut reader = BufReader::new(File::open(path)?);
    let mut count = 0usize;
    let mut buf: Vec<u8> = Vec::new();
    loop {
        buf.clear();
        if reader.read_until(b'\n', &mut buf)? == 0 {
            break;
        }
        if buf.last() == Some(&b'\n') {
            buf.pop();
        }
        if matches_filter(&String::from_utf8_lossy(&buf), filter) {
            count += 1;
        }
    }
    Ok(count)
}

/// Bytes read per reverse-seek chunk while tailing. Large enough that a
/// typical `--lines` tail (tens of lines) resolves in one or two chunk reads
/// even against a many-megabyte log; small enough that memory stays trivial
/// and bounded by how far back a match search actually needs to go, not by
/// the file's total size.
const TAIL_CHUNK_SIZE: u64 = 256 * 1024;

/// Read the last `limit` lines satisfying `filter` from `path`, scanning
/// backward from the end in [`TAIL_CHUNK_SIZE`] chunks instead of
/// `read_to_string`-ing the whole file. See [`tail_matching_lines_with_chunk`]
/// for the algorithm.
fn tail_matching_lines(
    path: &Path,
    filter: Option<&str>,
    limit: usize,
) -> std::io::Result<Vec<String>> {
    tail_matching_lines_with_chunk(path, filter, limit, TAIL_CHUNK_SIZE)
}

/// The actual reverse-chunk tail scan, parameterized on chunk size so tests
/// can force multi-chunk boundary crossings against small inputs without
/// needing a multi-hundred-KB fixture.
///
/// Returns up to `limit` matching lines, oldest-first (the same top-to-bottom
/// order the previous whole-file-read implementation produced), stopping as
/// soon as `limit` matches are found rather than always scanning to the start
/// of the file.
///
/// # Algorithm
///
/// Reads `chunk_size`-byte chunks working backward from EOF. Each chunk is
/// split on `\n`; every piece except the first is a complete line (the first
/// piece's true start lies further back in the file, in bytes not yet read,
/// unless this chunk reaches the start of the file). The incomplete first
/// piece is carried forward and prepended — in file-byte order, appended
/// after the next chunk's freshly read bytes — to be completed by the next
/// (earlier) chunk. Complete lines are matched against `filter` and collected
/// most-recent-first (the discovery order), then reversed once at the end.
fn tail_matching_lines_with_chunk(
    path: &Path,
    filter: Option<&str>,
    limit: usize,
    chunk_size: u64,
) -> std::io::Result<Vec<String>> {
    if limit == 0 {
        return Ok(Vec::new());
    }

    let mut file = File::open(path)?;
    let file_len = file.metadata()?.len();

    // Collected in most-recent-first order (the direction we discover
    // matches, scanning backward from EOF); reversed once at the end.
    let mut collected: Vec<String> = Vec::new();
    // The as-yet-incomplete line at the start of the chunk just read; its
    // true start lies further back in the file, still unread. Appending it
    // after the next (earlier) chunk's bytes reconstructs the full line.
    let mut carry: Vec<u8> = Vec::new();
    let mut pos = file_len;
    let mut first_chunk = true;

    while pos > 0 && collected.len() < limit {
        let read_size = chunk_size.min(pos);
        pos -= read_size;
        file.seek(SeekFrom::Start(pos))?;
        let mut chunk = vec![0u8; read_size as usize];
        file.read_exact(&mut chunk)?;
        chunk.extend_from_slice(&carry);
        carry.clear();

        let mut pieces: Vec<&[u8]> = chunk.split(|&b| b == b'\n').collect();
        // Only on the tail-most (first-processed) chunk does a trailing
        // empty piece mean "nothing after the file's final newline" rather
        // than a genuine blank line — on every later chunk, a trailing
        // empty piece is a real blank line bounded by an actual newline on
        // each side (the one found by the previous iteration) and must be
        // kept.
        if first_chunk && chunk.last() == Some(&b'\n') {
            pieces.pop();
        }
        first_chunk = false;

        // Whether unread file content precedes this chunk: if so,
        // `pieces[0]` is not yet a complete line and must carry forward
        // instead of being matched now.
        let more_before = pos > 0;
        let start = usize::from(more_before);

        for piece in pieces[start..].iter().rev() {
            if collected.len() >= limit {
                break;
            }
            let line = String::from_utf8_lossy(piece).into_owned();
            if matches_filter(&line, filter) {
                collected.push(line);
            }
        }

        if more_before {
            carry = pieces[0].to_vec();
        }
    }

    collected.reverse();
    Ok(collected)
}

pub fn run(args: LogsArgs, json: bool) -> Result<()> {
    let Some(path) = resolve_log_path() else {
        let looked = candidate_paths()
            .iter()
            .map(|p| format!("  {}", p.display()))
            .collect::<Vec<_>>()
            .join("\n");
        bail!(
            "No daemon log found. Looked in:\n{looked}\n\n\
             A daemon that has never started writes no log — check `nodespace diagnostics` first."
        );
    };

    if args.path_only {
        if json {
            println!(
                "{}",
                serde_json::json!({ "path": path.display().to_string() })
            );
        } else {
            println!("{}", path.display());
        }
        return Ok(());
    }

    let matched_total = count_matching_lines(&path, args.filter.as_deref())
        .map_err(|e| anyhow::anyhow!("Could not read {}: {e}", path.display()))?;
    let shown = tail_matching_lines(&path, args.filter.as_deref(), args.lines)
        .map_err(|e| anyhow::anyhow!("Could not read {}: {e}", path.display()))?;

    if json {
        println!(
            "{}",
            serde_json::json!({
                "path": path.display().to_string(),
                "matched": matched_total,
                "shown": shown.len(),
                "lines": shown,
            })
        );
    } else {
        for line in &shown {
            println!("{line}");
        }
        if matched_total > shown.len() {
            eprintln!(
                "\n({} of {} matching lines shown — raise --lines for more)",
                shown.len(),
                matched_total
            );
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    /// Both install layouts are checked, or the verb reintroduces exactly the
    /// wrong-path problem it exists to remove.
    #[test]
    fn candidates_cover_both_install_layouts() {
        let paths: Vec<String> =
            candidate_paths_for(Some(PathBuf::from("/home/user/.nodespace")), false)
                .iter()
            .map(|p| p.display().to_string())
            .collect();
        let joined = paths.join("\n");

        assert!(
            joined.contains(".nodespace/logs/nodespaced.log"),
            "the desktop app's log path must be a candidate: {joined}"
        );
        assert!(
            joined.contains("var/log/nodespace/nodespaced.log"),
            "a Homebrew service's log path must be a candidate: {joined}"
        );
    }

    /// A redirected NodeSpace home has one log, its own: no Homebrew prefix
    /// is consulted, so an isolated run never resolves to another install's
    /// log.
    #[test]
    fn a_redirected_home_is_the_only_candidate() {
        let paths = candidate_paths_for(Some(PathBuf::from("/isolated/.nodespace")), true);
        assert_eq!(
            paths,
            vec![PathBuf::from("/isolated/.nodespace/logs/nodespaced.log")]
        );
    }

    /// Reference implementation mirroring the old `read_to_string`-based
    /// behavior exactly, used to check the bounded-memory scan against on
    /// generated fixtures rather than hand-computing expected output.
    fn naive_tail(body: &str, filter: Option<&str>, limit: usize) -> (usize, Vec<String>) {
        let matched: Vec<&str> = body
            .lines()
            .filter(|line| matches_filter(line, filter))
            .collect();
        let shown: Vec<String> = matched
            .iter()
            .rev()
            .take(limit)
            .rev()
            .map(|s| s.to_string())
            .collect();
        (matched.len(), shown)
    }

    fn write_temp_log(contents: &str) -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("nodespaced.log");
        let mut f = File::create(&path).expect("create log file");
        f.write_all(contents.as_bytes()).expect("write log file");
        (dir, path)
    }

    /// A small input scanned with a tiny chunk size, forcing many chunk
    /// boundaries mid-line, must still reconstruct every line exactly —
    /// including a genuine blank line and a final line with no trailing
    /// newline.
    #[test]
    fn tail_matching_lines_reconstructs_lines_across_tiny_chunk_boundaries() {
        let body = "line one\nline two\n\nline four\nlast line no newline";
        let (_dir, path) = write_temp_log(body);

        for chunk_size in [1u64, 2, 3, 5, 8, 13, 1024] {
            let got = tail_matching_lines_with_chunk(&path, None, 10, chunk_size)
                .unwrap_or_else(|e| panic!("chunk_size={chunk_size}: {e}"));
            let (_, expected) = naive_tail(body, None, 10);
            assert_eq!(
                got, expected,
                "chunk_size={chunk_size} must reconstruct every line identically"
            );
        }
    }

    /// The tail limit stops collection early (most-recent lines only), and a
    /// filter is applied identically to the naive whole-file reference,
    /// across chunk sizes that force the match to be found mid-chunk-scan.
    #[test]
    fn tail_matching_lines_respects_limit_and_filter_across_chunk_sizes() {
        let mut body = String::new();
        for i in 0..500 {
            if i % 7 == 0 {
                body.push_str(&format!("ERROR line {i}\n"));
            } else {
                body.push_str(&format!("info line {i}\n"));
            }
        }
        let (_dir, path) = write_temp_log(&body);

        for chunk_size in [4u64, 16, 64, 4096] {
            for (filter, limit) in [(None, 5), (Some("ERROR"), 3), (Some("ERROR"), 100)] {
                let got = tail_matching_lines_with_chunk(&path, filter, limit, chunk_size)
                    .unwrap_or_else(|e| panic!("chunk_size={chunk_size}: {e}"));
                let (_, expected) = naive_tail(&body, filter, limit);
                assert_eq!(
                    got, expected,
                    "chunk_size={chunk_size} filter={filter:?} limit={limit}"
                );
            }
        }
    }

    #[test]
    fn tail_matching_lines_limit_zero_returns_empty() {
        let (_dir, path) = write_temp_log("a\nb\nc\n");
        let got = tail_matching_lines(&path, None, 0).expect("must succeed");
        assert!(got.is_empty());
    }

    #[test]
    fn count_matching_lines_matches_naive_reference() {
        let body = "alpha\nbeta ERROR\ngamma\ndelta ERROR\nepsilon\n";
        let (_dir, path) = write_temp_log(body);

        let (expected_total, _) = naive_tail(body, Some("ERROR"), usize::MAX);
        let got = count_matching_lines(&path, Some("ERROR")).expect("must succeed");
        assert_eq!(got, expected_total);
    }

    /// A long-lived, never-rotated headless log (exactly this command's
    /// target case) can end up with a stray invalid-UTF-8 byte somewhere in
    /// it. Both passes must tolerate that (lossy decoding) rather than
    /// hard-erroring the whole command over one bad byte, and must still
    /// find every valid line around it.
    #[test]
    fn count_and_tail_tolerate_invalid_utf8_instead_of_erroring() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("nodespaced.log");
        let mut f = File::create(&path).expect("create log file");
        f.write_all(b"good line one\n").unwrap();
        f.write_all(&[b'b', b'a', b'd', 0xFF, 0xFE, b'\n']).unwrap();
        f.write_all(b"good line two\n").unwrap();
        drop(f);

        let total =
            count_matching_lines(&path, None).expect("must not error on invalid UTF-8 bytes");
        assert_eq!(total, 3);

        let shown =
            tail_matching_lines(&path, None, 10).expect("must not error on invalid UTF-8 bytes");
        assert_eq!(shown.len(), 3);
        assert_eq!(shown[0], "good line one");
        assert_eq!(shown[2], "good line two");
    }

    /// The end-to-end case the fix exists for: a large (50MB-class) log file
    /// tailed and filtered correctly without ever reading it whole. Verified
    /// against the naive whole-file reference for correctness; the bounded
    /// memory property is structural (the implementation under test never
    /// calls `read_to_string` and only ever holds one chunk plus a small
    /// collected-lines buffer).
    #[test]
    fn tail_and_filter_are_correct_against_a_50mb_generated_log() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("nodespaced.log");
        let mut f = File::create(&path).expect("create log file");

        // ~50MB: ~1M lines at ~50 bytes/line. Every 100th line carries the
        // filter marker, so a filtered tail must scan back across many
        // chunk reads to collect `--lines` matches instead of finding them
        // all in the very last chunk.
        const TOTAL_LINES: usize = 1_000_000;
        let mut expected_matched = 0usize;
        let mut last_matches: Vec<String> = Vec::new();
        for i in 0..TOTAL_LINES {
            let is_match = i % 100 == 0;
            let line = if is_match {
                format!("2026-09-28T00:00:00Z ERROR daemon: play failed id={i}\n")
            } else {
                format!("2026-09-28T00:00:00Z INFO daemon: heartbeat tick={i}\n")
            };
            f.write_all(line.as_bytes()).expect("write line");
            if is_match {
                expected_matched += 1;
                let trimmed = line.trim_end_matches('\n').to_string();
                last_matches.push(trimmed);
                if last_matches.len() > 20 {
                    last_matches.remove(0);
                }
            }
        }
        drop(f);

        let file_len = std::fs::metadata(&path).expect("stat").len();
        assert!(
            file_len > 40 * 1024 * 1024,
            "fixture must be tens of MB to exercise the multi-chunk path, got {file_len} bytes"
        );

        let total = count_matching_lines(&path, Some("ERROR")).expect("count must succeed");
        assert_eq!(total, expected_matched);

        let shown = tail_matching_lines(&path, Some("ERROR"), 20).expect("tail must succeed");
        assert_eq!(shown, last_matches);
    }
}
