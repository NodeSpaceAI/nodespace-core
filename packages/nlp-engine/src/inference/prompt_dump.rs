//! Dev-only prompt/response dump for every inference engine.
//!
//! One module, one writer and one sequence counter serve both engines, so the
//! records of a local turn and of a remote turn form a single numbered
//! timeline in one file, and a provider swap mid-conversation reads as one
//! continuous sequence.
//!
//! When `NODESPACE_PROMPT_DUMP` is set to a file path, every call appends what
//! was put on the wire, and the raw (pre-normalization) response, to that file
//! as line-delimited JSON. When the env var is unset this is a zero-cost
//! no-op.
//!
//! A record has the same shape whichever engine wrote it, tagged with the
//! engine. What differs is only what was put on the wire: the rendered prompt
//! text for the local engine, the request body for a remote one.
//!
//! Usage:
//! ```sh
//! NODESPACE_PROMPT_DUMP=/tmp/dump.jsonl <run the daemon>
//! # then inspect /tmp/dump.jsonl — one JSON object per line:
//! #   {"kind":"prompt","engine":"native","seq":0,"target":null,
//! #    "sent":"<full rendered text>","sent_len":123}
//! #   {"kind":"prompt","engine":"openai_compat","seq":1,
//! #    "target":"http://host/v1","sent":{<request body>},"sent_len":456}
//! #   {"kind":"response","engine":"openai_compat","seq":1,
//! #    "raw_response":"<full>","raw_response_len":789}
//! ```

use std::io::Write;
use std::sync::atomic::{AtomicU64, Ordering};

/// Env var holding the dump file path. Unset → no-op.
const ENV_DUMP_PATH: &str = "NODESPACE_PROMPT_DUMP";

/// Monotonic counter correlating a prompt record with its response record.
/// Shared by every engine, so a `seq` is unique across the whole file and
/// call order is the timeline. A request carries no session or turn id at
/// this layer, so call order is the only correlation key available here.
static SEQ: AtomicU64 = AtomicU64::new(0);

/// Which engine wrote a record.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Engine {
    /// The local engine: the model runs in this process under llama.cpp.
    Native,
    /// A remote client: an OpenAI-compatible HTTP endpoint.
    OpenAiCompat,
}

impl Engine {
    /// The tag written to a record's `engine` field.
    pub const fn tag(self) -> &'static str {
        match self {
            Engine::Native => "native",
            Engine::OpenAiCompat => "openai_compat",
        }
    }
}

/// What an engine put on the wire for one request.
#[derive(Debug, Clone, Copy)]
pub enum Sent<'a> {
    /// The rendered prompt text handed to the tokenizer.
    Text(&'a str),
    /// The request body POSTed to a remote endpoint.
    Body(&'a serde_json::Value),
}

fn dump_path() -> Option<String> {
    match std::env::var(ENV_DUMP_PATH) {
        Ok(p) if !p.trim().is_empty() => Some(p),
        _ => None,
    }
}

/// True when dumping is enabled — lets callers skip building the payload.
pub fn enabled() -> bool {
    dump_path().is_some()
}

fn append(value: &serde_json::Value) {
    let Some(path) = dump_path() else { return };
    let line = match serde_json::to_string(value) {
        Ok(s) => s,
        Err(e) => {
            tracing::debug!(error = %e, "nlp-engine prompt_dump: serialize failed");
            return;
        }
    };
    match std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
    {
        Ok(mut f) => {
            if let Err(e) = writeln!(f, "{line}") {
                tracing::debug!(error = %e, path = %path, "nlp-engine prompt_dump: write failed");
            }
        }
        Err(e) => tracing::debug!(error = %e, path = %path, "nlp-engine prompt_dump: open failed"),
    }
}

/// Dump what `engine` put on the wire for this call. `target` names where a
/// remote request went (its base URL); a local engine has none. Returns the
/// sequence number to pass to [`dump_response`] so the two records can be
/// joined.
pub fn dump_prompt(engine: Engine, target: Option<&str>, sent: Sent<'_>) -> u64 {
    let seq = SEQ.fetch_add(1, Ordering::Relaxed);
    if enabled() {
        let (sent_value, sent_len) = match sent {
            Sent::Text(text) => (serde_json::Value::from(text), text.len()),
            Sent::Body(body) => (
                body.clone(),
                serde_json::to_string(body).map(|s| s.len()).unwrap_or(0),
            ),
        };
        append(&serde_json::json!({
            "kind": "prompt",
            "engine": engine.tag(),
            "seq": seq,
            "target": target,
            "sent": sent_value,
            "sent_len": sent_len,
        }));
    }
    seq
}

/// Dump the raw (pre-normalization) model response for the call identified
/// by `seq` (the value [`dump_prompt`] returned for this same call).
pub fn dump_response(engine: Engine, seq: u64, raw_response: &str) {
    if !enabled() {
        return;
    }
    append(&serde_json::json!({
        "kind": "response",
        "engine": engine.tag(),
        "seq": seq,
        "raw_response": raw_response,
        "raw_response_len": raw_response.len(),
    }));
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The env var is process-global, so tests that touch it share this lock.
    static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    /// Reads every record `NODESPACE_PROMPT_DUMP` produced during `f`.
    fn with_dump<F: FnOnce()>(f: F) -> Vec<serde_json::Value> {
        let _guard = LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("dump.jsonl");
        std::env::set_var(ENV_DUMP_PATH, &path);
        f();
        std::env::remove_var(ENV_DUMP_PATH);
        std::fs::read_to_string(&path)
            .unwrap_or_default()
            .lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect()
    }

    #[test]
    fn both_engines_share_one_numbered_timeline() {
        let body = serde_json::json!({"model": "m", "messages": []});
        let records = with_dump(|| {
            let local = dump_prompt(Engine::Native, None, Sent::Text("rendered"));
            dump_response(Engine::Native, local, "local answer");
            let remote = dump_prompt(Engine::OpenAiCompat, Some("http://h/v1"), Sent::Body(&body));
            dump_response(Engine::OpenAiCompat, remote, "remote answer");
        });

        assert_eq!(records.len(), 4);
        let seqs: Vec<u64> = records.iter().map(|r| r["seq"].as_u64().unwrap()).collect();
        assert_eq!(seqs[0], seqs[1], "a response carries its prompt's seq");
        assert_eq!(seqs[2], seqs[3], "a response carries its prompt's seq");
        assert_eq!(
            seqs[2],
            seqs[0] + 1,
            "the two engines draw from one counter, so seq never repeats across them"
        );
        assert_eq!(records[0]["engine"], "native");
        assert_eq!(records[2]["engine"], "openai_compat");
    }

    #[test]
    fn prompt_records_have_one_shape_whichever_engine_wrote_them() {
        let body = serde_json::json!({"model": "m"});
        let records = with_dump(|| {
            dump_prompt(Engine::Native, None, Sent::Text("rendered"));
            dump_prompt(Engine::OpenAiCompat, Some("http://h/v1"), Sent::Body(&body));
        });

        let keys = |r: &serde_json::Value| -> Vec<String> {
            let mut k: Vec<String> = r.as_object().unwrap().keys().cloned().collect();
            k.sort();
            k
        };
        assert_eq!(keys(&records[0]), keys(&records[1]));
        assert_eq!(records[0]["sent"], "rendered");
        assert_eq!(records[0]["sent_len"], 8);
        assert!(records[0]["target"].is_null());
        assert_eq!(records[1]["sent"], body);
        assert_eq!(records[1]["target"], "http://h/v1");
    }

    #[test]
    fn numbering_stays_monotonic_with_the_dump_off() {
        let _guard = LOCK.lock().unwrap_or_else(|e| e.into_inner());
        std::env::remove_var(ENV_DUMP_PATH);
        let a = dump_prompt(Engine::Native, None, Sent::Text("x"));
        let b = dump_prompt(Engine::Native, None, Sent::Text("y"));
        assert!(b > a);
    }
}
