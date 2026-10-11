//! `nodespace wait` — block until a node or a saved query's result changes in
//! a stated way, then print it and exit (ADR-093 §1, "A blocking read").
//!
//! A session that hands work to someone else (a review by another session, a
//! question for a person) starts a wait in the background and is resumed by
//! its harness when the command exits. The exit status is the contract:
//! [`EXIT_TIMEOUT`], [`EXIT_UNREACHABLE`] and [`EXIT_DAEMON_UNAVAILABLE`] are
//! told apart from each other and from an ordinary failure.
//!
//! The wait is driven by the daemon's `WatchNodes` stream and never by a
//! timer. The stream says only that something changed; whether the condition
//! holds is asked with the reads the CLI already has, so a condition is
//! written as the filters `nodespace query` takes and there is no second
//! condition language. The order is fixed: open the stream, check, then wait
//! for events, so a change made between the check and the first event is
//! seen.
//!
//! Nothing is kept between runs. The node and the queue are the record, and
//! running a wait again after any exit resumes waiting.

use std::collections::BTreeSet;
use std::io::Write;
use std::path::Path;
use std::time::Duration;

use anyhow::{Context, Result};
use clap::{Args, Subcommand, ValueEnum};
use nodespace_daemon::nodespace::{
    ExecuteQueryRequest, GetNodeContextRequest, GetNodeContextResponse, GetNodeRequest, NodeEvent,
    RunSavedQueryRequest, RunSavedQueryResponse, WatchRequest,
};
use serde_json::{json, Value};
use tokio::time::Instant;
use tokio_stream::StreamExt;
use tonic::{Code, Status, Streaming};

use crate::output;
use crate::{connect, connect_database, resolve_routing, DatabaseIdInterceptor, NodeClient};

/// Exit status when `--timeout` passed before the condition held.
pub const EXIT_TIMEOUT: i32 = 3;
/// Exit status when the condition can no longer be reached: the node or the
/// saved query is gone, or the node matches `--unless`.
pub const EXIT_UNREACHABLE: i32 = 4;
/// Exit status when the daemon was not running, or went away and did not
/// come back.
pub const EXIT_DAEMON_UNAVAILABLE: i32 = 5;

#[derive(Subcommand, Debug)]
pub enum WaitAction {
    /// Wait until a node matches a condition, then print it with its context,
    /// as `node context` reads it. A condition that already holds returns at
    /// once. Exits 3 on timeout, 4 when the node no longer exists or matches
    /// `--unless`, 5 when the daemon is unavailable.
    Node(NodeWaitArgs),
    /// Wait until a saved query's result changes, then print the run, as
    /// `query run` prints it, with what it gained and lost. Exits 3 on
    /// timeout, 4 when the saved query or the `--item` node no longer exists,
    /// 5 when the daemon is unavailable.
    Query(QueryWaitArgs),
}

#[derive(Args, Debug, Clone)]
pub struct TimingArgs {
    /// Give up after this long and exit 3: a number of seconds, or a number
    /// with `s`, `m` or `h` (`90s`, `30m`, `3h`).
    #[arg(long, default_value = "3h", value_parser = parse_duration)]
    pub timeout: Duration,
    /// Print a status line to stderr this often while waiting, in the form
    /// `--timeout` takes.
    #[arg(long, default_value = "1m", value_parser = parse_duration)]
    pub interval: Duration,
}

#[derive(Args, Debug)]
pub struct NodeWaitArgs {
    /// Node ID (UUID).
    pub id: String,
    /// The condition: a JSON array of filter conditions in the shape
    /// `nodespace query --filters` takes, all of which the node must match,
    /// e.g. `[{"type":"property","operator":"equals","property":"status","value":"in_review","negate":true}]`
    /// for "its status is no longer in_review".
    #[arg(long)]
    pub until: String,
    /// A condition that ends the wait with exit 4 when the node matches it,
    /// in the shape `--until` takes: the state from which `--until` will not
    /// be reached, e.g. a cancelled task. It is checked before `--until`.
    #[arg(long)]
    pub unless: Option<String>,
    #[command(flatten)]
    pub timing: TimingArgs,
}

/// What a query wait waits for, compared with the result the query had when
/// the wait started.
#[derive(ValueEnum, Debug, Clone, Copy, PartialEq, Eq)]
pub enum QueryChange {
    /// The result gained or lost an item.
    Change,
    /// The result gained an item.
    Gain,
    /// The result lost an item.
    Loss,
    /// The result has at least one item. Holds at once when it already does.
    NotEmpty,
}

#[derive(Args, Debug)]
pub struct QueryWaitArgs {
    /// The saved query's id, or its title (quoted when it has spaces). A
    /// title must name exactly one saved query.
    pub query: String,
    /// What to wait for. `change`, `gain` and `loss` compare the result with
    /// the one the query had when the wait started; `not-empty` holds as soon
    /// as the result has an item.
    #[arg(long, value_enum, default_value = "change", conflicts_with = "item")]
    pub until: QueryChange,
    /// Wait until this node is in the result, in place of `--until`. Holds
    /// at once when it already is.
    #[arg(long, value_name = "NODE_ID")]
    pub item: Option<String>,
    /// JSON array of filter conditions ANDed with the stored ones for every
    /// run of the wait, in the shape `nodespace query --filters` takes.
    #[arg(long)]
    pub filters: Option<String>,
    #[command(flatten)]
    pub timing: TimingArgs,
}

/// A duration as `--timeout` and `--interval` take it: seconds, or a number
/// with a unit.
fn parse_duration(s: &str) -> Result<Duration, String> {
    let (digits, unit) = match s.char_indices().find(|(_, c)| !c.is_ascii_digit()) {
        Some((at, _)) => s.split_at(at),
        None => (s, "s"),
    };
    let scale = match unit {
        "s" => 1,
        "m" => 60,
        "h" => 3600,
        _ => return Err(format!("'{s}' is not a duration: use 90s, 30m or 3h")),
    };
    let seconds = digits
        .parse::<u64>()
        .ok()
        .and_then(|n| n.checked_mul(scale))
        .ok_or_else(|| format!("'{s}' is not a duration: use 90s, 30m or 3h"))?;
    if seconds == 0 {
        return Err("a duration must be greater than zero".to_string());
    }
    Ok(Duration::from_secs(seconds))
}

/// How long a wait lasts and how it paces what it prints and retries.
#[derive(Debug, Clone, Copy)]
pub struct Timing {
    pub timeout: Duration,
    pub interval: Duration,
    /// How long a daemon that went away is retried before the wait gives up.
    pub reconnect_for: Duration,
    /// The pause between two reconnect attempts.
    pub reconnect_every: Duration,
}

impl From<&TimingArgs> for Timing {
    fn from(args: &TimingArgs) -> Self {
        Self {
            timeout: args.timeout,
            interval: args.interval,
            reconnect_for: Duration::from_secs(120),
            reconnect_every: Duration::from_secs(2),
        }
    }
}

/// What a wait watches and the condition it checks.
#[derive(Debug)]
pub enum Target {
    Node {
        id: String,
        /// The `--until` filters, as a JSON array.
        until: Vec<Value>,
        /// The `--unless` filters, when given.
        unless: Option<Vec<Value>>,
    },
    Query {
        query: String,
        filters: Option<String>,
        until: QueryChange,
        item: Option<String>,
        /// The ids the result held at the first check: what `change`, `gain`
        /// and `loss` are compared with.
        baseline: Option<BTreeSet<String>>,
    },
}

/// What a wait returns when its condition holds.
#[derive(Debug)]
pub enum Met {
    Node(Box<GetNodeContextResponse>),
    Query {
        run: Box<RunSavedQueryResponse>,
        /// The ids in the result now that were not there when the wait
        /// started, and the ones there then that are gone. Both empty for
        /// `not-empty` and `--item`, which compare with nothing.
        gained: Vec<String>,
        lost: Vec<String>,
    },
}

/// How a wait ended.
#[derive(Debug)]
pub enum Outcome {
    Met(Met),
    TimedOut,
    /// The condition can no longer be reached, and why.
    Unreachable(String),
    /// The daemon could not be reached, and what the attempt reported.
    DaemonUnavailable(String),
}

/// A wait that ended without its condition: carried to `main`, which exits
/// with `code` after printing `message`.
#[derive(Debug)]
pub struct WaitEnded {
    pub code: i32,
    pub message: String,
}

impl std::fmt::Display for WaitEnded {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for WaitEnded {}

/// One check of a target's condition.
enum Check {
    Met(Met),
    Pending,
    Unreachable(String),
}

fn parse_filters(flag: &str, raw: &str) -> Result<Vec<Value>> {
    serde_json::from_str::<Vec<Value>>(raw)
        .with_context(|| format!("{flag} must be a JSON array of filter conditions"))
}

impl Target {
    pub fn node(args: &NodeWaitArgs) -> Result<Self> {
        Ok(Self::Node {
            id: args.id.clone(),
            until: parse_filters("--until", &args.until)?,
            unless: args
                .unless
                .as_deref()
                .map(|raw| parse_filters("--unless", raw))
                .transpose()?,
        })
    }

    pub fn query(args: &QueryWaitArgs) -> Self {
        Self::Query {
            query: args.query.clone(),
            filters: args.filters.clone(),
            until: args.until,
            item: args.item.clone(),
            baseline: None,
        }
    }

    /// What is being waited on, for a status line.
    fn describe(&self) -> String {
        match self {
            Self::Node { id, .. } => format!("node {id}"),
            Self::Query { query, .. } => format!("saved query '{query}'"),
        }
    }

    async fn check(&mut self, client: &mut NodeClient) -> Result<Check, Status> {
        match self {
            Self::Node { id, until, unless } => check_node(client, id, until, unless).await,
            Self::Query {
                query,
                filters,
                until,
                item,
                baseline,
            } => check_query(client, query, filters, *until, item, baseline).await,
        }
    }
}

/// Whether `id`, a node of type `node_type`, matches every filter in
/// `filters`: the filters run as a query narrowed to that one node.
async fn node_matches(
    client: &mut NodeClient,
    id: &str,
    node_type: &str,
    filters: &[Value],
) -> Result<bool, Status> {
    let mut narrowed = vec![json!({
        "type": "metadata",
        "operator": "equals",
        "property": "id",
        "value": id,
    })];
    narrowed.extend(filters.iter().cloned());
    let response = client
        .execute_query(ExecuteQueryRequest {
            target_type: node_type.to_string(),
            filters_json: Some(Value::Array(narrowed).to_string()),
            sorting_json: None,
            limit: 1,
        })
        .await?
        .into_inner();
    Ok(response.count > 0)
}

async fn check_node(
    client: &mut NodeClient,
    id: &str,
    until: &[Value],
    unless: &Option<Vec<Value>>,
) -> Result<Check, Status> {
    let node = match client
        .get_node(GetNodeRequest {
            node_id: id.to_string(),
        })
        .await
    {
        Ok(response) => response.into_inner().node_data,
        Err(status) if status.code() == Code::NotFound => None,
        Err(status) => return Err(status),
    };
    let Some(node) = node else {
        return Ok(Check::Unreachable(format!("node {id} no longer exists")));
    };

    if let Some(unless) = unless {
        if node_matches(client, id, &node.node_type, unless).await? {
            return Ok(Check::Unreachable(format!("node {id} matches --unless")));
        }
    }
    if !node_matches(client, id, &node.node_type, until).await? {
        return Ok(Check::Pending);
    }

    match client
        .get_node_context(GetNodeContextRequest {
            node_id: id.to_string(),
            paths_json: None,
            version_only: false,
        })
        .await
    {
        Ok(response) => Ok(Check::Met(Met::Node(Box::new(response.into_inner())))),
        // Deleted between the match and the read.
        Err(status) if status.code() == Code::NotFound => {
            Ok(Check::Unreachable(format!("node {id} no longer exists")))
        }
        Err(status) => Err(status),
    }
}

async fn check_query(
    client: &mut NodeClient,
    query: &str,
    filters: &Option<String>,
    until: QueryChange,
    item: &Option<String>,
    baseline: &mut Option<BTreeSet<String>>,
) -> Result<Check, Status> {
    if let Some(item) = item {
        match client
            .get_node(GetNodeRequest {
                node_id: item.clone(),
            })
            .await
        {
            Ok(_) => {}
            Err(status) if status.code() == Code::NotFound => {
                return Ok(Check::Unreachable(format!("node {item} no longer exists")));
            }
            Err(status) => return Err(status),
        }
    }

    let run = match client
        .run_saved_query(RunSavedQueryRequest {
            query: query.to_string(),
            filters_json: filters.clone(),
            limit: 0,
            with_context: false,
        })
        .await
    {
        Ok(response) => response.into_inner(),
        Err(status) if status.code() == Code::NotFound => {
            return Ok(Check::Unreachable(format!(
                "saved query '{query}' no longer exists"
            )));
        }
        Err(status) => return Err(status),
    };

    let now: BTreeSet<String> = run.nodes.iter().map(|node| node.id.clone()).collect();
    if let Some(item) = item {
        return Ok(if now.contains(item) {
            met_query(run, Vec::new(), Vec::new())
        } else {
            Check::Pending
        });
    }
    if until == QueryChange::NotEmpty {
        return Ok(if now.is_empty() {
            Check::Pending
        } else {
            met_query(run, Vec::new(), Vec::new())
        });
    }

    let before = baseline.get_or_insert_with(|| now.clone());
    let gained: Vec<String> = now.difference(before).cloned().collect();
    let lost: Vec<String> = before.difference(&now).cloned().collect();
    let holds = match until {
        QueryChange::Gain => !gained.is_empty(),
        QueryChange::Loss => !lost.is_empty(),
        QueryChange::Change | QueryChange::NotEmpty => !gained.is_empty() || !lost.is_empty(),
    };
    Ok(if holds {
        met_query(run, gained, lost)
    } else {
        Check::Pending
    })
}

fn met_query(run: RunSavedQueryResponse, gained: Vec<String>, lost: Vec<String>) -> Check {
    Check::Met(Met::Query {
        run: Box::new(run),
        gained,
        lost,
    })
}

/// The daemon stopped answering, as opposed to answering with a refusal.
/// A request cut off when the daemon goes away surfaces as one of these.
fn is_disconnect(status: &Status) -> bool {
    matches!(
        status.code(),
        Code::Unavailable | Code::Unknown | Code::Cancelled | Code::Internal
    )
}

/// Open the change stream as a background subscriber: a wait is nobody
/// looking at the database.
async fn open_stream(client: &mut NodeClient) -> Result<Streaming<NodeEvent>, Status> {
    Ok(client
        .watch_nodes(WatchRequest {
            node_type: String::new(),
            root_id: String::new(),
            background: true,
        })
        .await?
        .into_inner())
}

/// A whole number of seconds or minutes, as a status line says it.
fn human(duration: Duration) -> String {
    let seconds = duration.as_secs();
    match seconds {
        0..=119 => format!("{seconds}s"),
        _ => format!("{}m", seconds / 60),
    }
}

/// The line printed every interval while waiting.
pub fn status_line(what: &str, waited: Duration, timeout: Duration) -> String {
    format!(
        "waiting on {what}: {} elapsed, timeout in {}",
        human(waited),
        human(timeout.saturating_sub(waited))
    )
}

/// Take every event the stream has already delivered, so a burst of writes
/// is answered with one check. Returns false when the stream ended.
async fn drain(stream: &mut Streaming<NodeEvent>) -> bool {
    loop {
        tokio::select! {
            biased;
            event = stream.next() => match event {
                Some(Ok(_)) => {}
                Some(Err(_)) | None => return false,
            },
            () = std::future::ready(()) => return true,
        }
    }
}

/// Retry the daemon until it answers, `reconnect_for` has passed or the
/// wait's own deadline has. `None` when it did not come back.
async fn reconnect(
    sock: &Path,
    interceptor: &DatabaseIdInterceptor,
    timing: &Timing,
    deadline: Instant,
) -> Option<NodeClient> {
    let give_up = (Instant::now() + timing.reconnect_for).min(deadline);
    loop {
        tokio::time::sleep(timing.reconnect_every).await;
        if let Ok(client) = connect(sock, interceptor.clone()).await {
            return Some(client);
        }
        if Instant::now() >= give_up {
            return None;
        }
    }
}

/// Wait until `target`'s condition holds, writing status lines to `status`.
///
/// `Err` is a failed command (a filter the daemon rejected); every way a
/// wait itself ends is an [`Outcome`].
pub async fn watch(
    sock: &Path,
    interceptor: &DatabaseIdInterceptor,
    target: &mut Target,
    timing: &Timing,
    status: &mut (dyn Write + Send),
) -> Result<Outcome> {
    let started = Instant::now();
    let deadline = started + timing.timeout;
    let mut ticker = tokio::time::interval_at(started + timing.interval, timing.interval);
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let what = target.describe();

    let mut client = match connect(sock, interceptor.clone()).await {
        Ok(client) => client,
        Err(error) => return Ok(Outcome::DaemonUnavailable(format!("{error:#}"))),
    };
    // A check that fails right after a connection was made is the daemon
    // refusing the request, not the daemon going away.
    let mut answered = false;

    loop {
        // Subscribe before checking: a change between the two is then seen.
        let disconnected = match open_stream(&mut client).await {
            Ok(mut stream) => loop {
                match target.check(&mut client).await {
                    Ok(Check::Met(met)) => return Ok(Outcome::Met(met)),
                    Ok(Check::Unreachable(reason)) => return Ok(Outcome::Unreachable(reason)),
                    Ok(Check::Pending) => answered = true,
                    Err(status) if answered && is_disconnect(&status) => break status,
                    Err(status) => return Err(check_failed(status)),
                }
                let ended = loop {
                    tokio::select! {
                        () = tokio::time::sleep_until(deadline) => return Ok(Outcome::TimedOut),
                        _ = ticker.tick() => {
                            let line = status_line(&what, started.elapsed(), timing.timeout);
                            writeln!(status, "{line}").ok();
                        }
                        event = stream.next() => match event {
                            Some(Ok(_)) => break !drain(&mut stream).await,
                            Some(Err(_)) | None => break true,
                        },
                    }
                };
                if ended {
                    break Status::unavailable("the change stream ended");
                }
            },
            Err(status) if answered && is_disconnect(&status) => status,
            Err(status) => return Err(check_failed(status)),
        };

        writeln!(
            status,
            "lost the daemon while waiting on {what} ({}); reconnecting for up to {}",
            crate::status_text(&disconnected),
            human(timing.reconnect_for)
        )
        .ok();
        client = match reconnect(sock, interceptor, timing, deadline).await {
            Some(client) => client,
            None if Instant::now() >= deadline => return Ok(Outcome::TimedOut),
            None => {
                return Ok(Outcome::DaemonUnavailable(format!(
                    "the daemon went away and did not come back within {}",
                    human(timing.reconnect_for)
                )))
            }
        };
        answered = false;
        writeln!(status, "reconnected; checking {what} again").ok();
    }
}

/// A check the daemon refused: its message says which filter or which query
/// is at fault.
fn check_failed(status: Status) -> anyhow::Error {
    match status.code() {
        Code::InvalidArgument | Code::NotFound | Code::FailedPrecondition => {
            anyhow::anyhow!("{}", status.message())
        }
        _ => anyhow::Error::new(status).context("the wait's check failed"),
    }
}

/// A wait's result on stdout: what `node context` or `query run` prints,
/// with what a query wait gained and lost.
fn print_met(met: &Met, json: bool) -> Result<()> {
    match met {
        Met::Node(response) => output::print_node_context(response, json),
        Met::Query { run, gained, lost } => output::print_query_wait(run, gained, lost, json),
    }
}

/// Run a wait to its end: print the result and return `Ok` when the
/// condition holds, or a [`WaitEnded`] carrying the exit status when it did
/// not.
pub async fn run(
    sock: &Path,
    selection: Option<&str>,
    action: WaitAction,
    json: bool,
) -> Result<()> {
    let (mut target, timing) = match &action {
        WaitAction::Node(args) => (Target::node(args)?, Timing::from(&args.timing)),
        WaitAction::Query(args) => (Target::query(args), Timing::from(&args.timing)),
    };

    // A daemon that is not running is this command's own exit status, told
    // apart from a database selection that names nothing.
    let outcome = match connect_database(sock).await {
        Err(error) => Outcome::DaemonUnavailable(format!("{error:#}")),
        Ok(_) => {
            let (interceptor, _) = resolve_routing(sock, selection).await?;
            watch(
                sock,
                &interceptor,
                &mut target,
                &timing,
                &mut std::io::stderr(),
            )
            .await?
        }
    };

    let (code, outcome, reason) = match outcome {
        Outcome::Met(met) => return print_met(&met, json),
        Outcome::TimedOut => (
            EXIT_TIMEOUT,
            "timeout",
            format!(
                "timed out after {} waiting on {}",
                human(timing.timeout),
                target.describe()
            ),
        ),
        Outcome::Unreachable(reason) => (EXIT_UNREACHABLE, "unreachable", reason),
        Outcome::DaemonUnavailable(reason) => {
            (EXIT_DAEMON_UNAVAILABLE, "daemon_unavailable", reason)
        }
    };
    if json {
        println!("{:#}", json!({ "outcome": outcome, "reason": reason }));
    }
    Err(anyhow::Error::new(WaitEnded {
        code,
        message: reason,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_duration_is_seconds_or_a_number_with_a_unit() {
        assert_eq!(parse_duration("90"), Ok(Duration::from_secs(90)));
        assert_eq!(parse_duration("90s"), Ok(Duration::from_secs(90)));
        assert_eq!(parse_duration("30m"), Ok(Duration::from_secs(1800)));
        assert_eq!(parse_duration("3h"), Ok(Duration::from_secs(10800)));
        for bad in ["", "0", "0m", "h", "3d", "1.5h", "-3", "3 h"] {
            assert!(parse_duration(bad).is_err(), "{bad:?} must be refused");
        }
    }

    #[test]
    fn a_status_line_says_what_is_waited_on_and_how_long_is_left() {
        assert_eq!(
            status_line(
                "node abc",
                Duration::from_secs(60),
                Duration::from_secs(3 * 3600)
            ),
            "waiting on node abc: 60s elapsed, timeout in 179m"
        );
    }

    #[test]
    fn a_condition_that_is_not_a_json_array_is_refused_naming_its_flag() {
        let error = parse_filters("--until", r#"{"operator":"equals"}"#).unwrap_err();
        assert!(format!("{error:#}").starts_with("--until must be a JSON array"));
    }
}
