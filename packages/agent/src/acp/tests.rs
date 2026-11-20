//! The session engine against a scripted fake agent (a Python program that
//! speaks the protocol on its standard input and output), hostile cases
//! included. No real agent, model or network is involved.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use nodespace_core::secrets::{InMemorySecretBackend, SecretStore};
use nodespace_types::EnvSecret;
use serde_json::{json, Value};
use tokio::sync::mpsc::Receiver;

use super::*;

const FAKE: &str = r#"
import sys, json, os, time
mode, log = sys.argv[1], sys.argv[2]
SID = "sess-1"
def out(o):
    sys.stdout.write(json.dumps(o) + "\n"); sys.stdout.flush()
def raw(s):
    sys.stdout.write(s + "\n"); sys.stdout.flush()
def note(k, v):
    with open(log, "a") as f: f.write(json.dumps({k: v}) + "\n")
def reply(i, r): out({"jsonrpc": "2.0", "id": i, "result": r})
def error(i, c, m): out({"jsonrpc": "2.0", "id": i, "error": {"code": c, "message": m}})
def upd(u, sid=SID): out({"jsonrpc": "2.0", "method": "session/update", "params": {"sessionId": sid, "update": u}})
def chunk(t, sid=SID): upd({"sessionUpdate": "agent_message_chunk", "content": {"type": "text", "text": t}}, sid)
def perm(i, tid="tc1"):
    out({"jsonrpc": "2.0", "id": i, "method": "session/request_permission", "params": {
        "sessionId": SID, "toolCall": {"toolCallId": tid, "title": "Run ls", "kind": "execute"},
        "options": [
            {"optionId": "always", "name": "Always", "kind": "allow_always"},
            {"optionId": "once", "name": "Allow once", "kind": "allow_once"},
            {"optionId": "never", "name": "Never", "kind": "reject_always"},
            {"optionId": "no", "name": "Deny", "kind": "reject_once"}]}})
def readmsg():
    l = sys.stdin.readline()
    if not l: sys.exit(0)
    m = json.loads(l); note("recv", m); return m
note("boot", {"pid": os.getpid(), "cwd": os.getcwd(), "cwdmode": os.stat(".").st_mode & 0o777,
    "env": sorted(k for k in os.environ if k in ("ACP_TOKEN", "NS_ACP_INHERITED", "DECLARED", "PATH", "HOME"))})
if mode == "stderrflood": sys.stderr.write("x" * 5000000); sys.stderr.flush()
prompt_id = None
def turn(i):
    global prompt_id
    prompt_id = i
    if mode == "ok":
        chunk("Hel"); chunk("lo")
        upd({"sessionUpdate": "agent_thought_chunk", "content": {"type": "text", "text": "hmm"}})
        upd({"sessionUpdate": "user_message_chunk", "content": {"type": "text", "text": "echo"}})
        upd({"sessionUpdate": "tool_call", "toolCallId": "t1", "title": "Read", "kind": "read", "status": "pending", "rawInput": {"p": "/secret"}})
        upd({"sessionUpdate": "tool_call_update", "toolCallId": "t1", "status": "completed", "rawOutput": "BIGSECRET"})
        upd({"sessionUpdate": "plan", "entries": [{"content": "a", "priority": "high", "status": "pending"}]})
        upd({"sessionUpdate": "usage_update", "used": 3, "size": 9})
        upd({"sessionUpdate": "available_commands_update", "availableCommands": [{"name": "nodespace", "description": "d"}]})
        reply(i, {"stopReason": "end_turn"})
    elif mode == "requests":
        for n, method in enumerate(["fs/read_text_file", "terminal/create", "_ext/thing", "session/set_mode"]):
            out({"jsonrpc": "2.0", "id": 200 + n, "method": method, "params": {"path": "/etc/passwd"}})
            readmsg()
        chunk("still here"); reply(i, {"stopReason": "end_turn"})
    elif mode == "perm":
        perm(100); readmsg(); chunk("after"); reply(i, {"stopReason": "end_turn"})
    elif mode == "permflood":
        for n in range(20): perm(100 + n, "tc%d" % n)
        got = 0
        while got < 20:
            readmsg(); got += 1
        reply(i, {"stopReason": "end_turn"})
    elif mode == "permhuge":
        for n in range(1000): perm(100 + n, "tc%d" % n)
        time.sleep(30)
    elif mode == "permcancel":
        perm(100); readmsg(); readmsg(); reply(i, {"stopReason": "cancelled"})
    elif mode in ("hold", "cancelignore", "cancelok"):
        chunk("working")
    elif mode == "hugemsg":
        raw(json.dumps({"jsonrpc": "2.0", "method": "session/update", "params": {"pad": "z" * 2000000}}))
        chunk("after huge"); reply(i, {"stopReason": "end_turn"})
    elif mode == "manyhuge":
        for n in range(20): raw("y" * 200000)
        time.sleep(30)
    elif mode == "endless":
        while True: chunk("x")
    elif mode == "endlessslow":
        while True: chunk("x"); time.sleep(0.01)
    elif mode == "silent":
        time.sleep(30)
    elif mode == "wrongid":
        reply(77, {"stopReason": "end_turn"}); reply("x", {}); reply(i, {"stopReason": "end_turn"})
    elif mode == "manywrongid":
        for n in range(1000): reply(1000 + n, {})
        time.sleep(30)
    elif mode == "noise":
        for n in range(10): raw("log: not json %d" % n)
        raw("[1,2,3]"); raw('{"jsonrpc":"1.0","id":5,"result":{}}')
        chunk("after noise"); reply(i, {"stopReason": "end_turn"})
    elif mode == "manynoise":
        for n in range(1000): raw("log: not json %d" % n)
        time.sleep(30)
    elif mode == "deep":
        raw("[" * 5000 + "]" * 5000); chunk("after deep"); reply(i, {"stopReason": "end_turn"})
    elif mode == "die":
        chunk("partial"); os._exit(3)
    elif mode == "echosecret":
        chunk("token is " + os.environ.get("ACP_TOKEN", "")); error(i, -32001, "bad " + os.environ.get("ACP_TOKEN", ""))
    elif mode == "foreign":
        chunk("not yours", "other-session"); chunk("yours"); reply(i, {"stopReason": "end_turn"})
    elif mode == "escaped":
        tok = os.environ.get("ACP_TOKEN", "")
        e = "".join("\\u%04x" % ord(c) for c in tok)
        def rawupd(body):
            raw('{"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"sess-1","update":' + body + '}}')
        rawupd('{"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":"a ' + e + ' b"},"messageId":"m' + e + '"}')
        rawupd('{"sessionUpdate":"agent_thought_chunk","content":{"type":"text","text":"' + e + '"}}')
        rawupd('{"sessionUpdate":"plan","entries":[{"content":"' + e + '","priority":"high","status":"pending"}]}')
        rawupd('{"sessionUpdate":"session_info_update","title":"' + e + '"}')
        rawupd('{"sessionUpdate":"tool_call","toolCallId":"t' + e + '","title":"' + e + '","kind":"read"}')
        rawupd('{"sessionUpdate":"available_commands_update","availableCommands":[{"name":"' + e + '"}]}')
        raw('{"jsonrpc":"2.0","id":100,"method":"session/request_permission","params":{"sessionId":"sess-1","toolCall":{"toolCallId":"tc","title":"' + e + '"},"options":[{"optionId":"o' + e + '","name":"' + e + '","kind":"allow_once"}]}}')
        readmsg(); reply(i, {"stopReason": "end_turn"})
    elif mode == "turnerror":
        error(i, -32000, "Authentication required")
    elif mode == "badstop":
        reply(i, {})
    elif mode == "weirdstop":
        reply(i, {"stopReason": "max_tokens"})
for line in sys.stdin:
    m = json.loads(line); note("recv", m)
    method, i = m.get("method"), m.get("id")
    if method == "initialize":
        if mode == "ignoreinit": continue
        if mode == "badversion": reply(i, {"protocolVersion": 2}); continue
        if mode == "initwrongid": reply(99, {"protocolVersion": 1})
        if mode == "initrequests":
            out({"jsonrpc": "2.0", "id": 300, "method": "fs/read_text_file", "params": {}}); readmsg()
        reply(i, {"protocolVersion": 1, "agentCapabilities": {"loadSession": True, "sessionCapabilities": {"resume": {}}},
                  "authMethods": [{"id": "login", "name": "Login"}], "agentInfo": {"name": "fake", "version": "1.2"}})
    elif method == "session/new":
        if mode == "needauth": error(i, -32000, "Authentication required"); continue
        if mode == "nosid": reply(i, {}); continue
        reply(i, {"sessionId": SID})
    elif method == "session/prompt":
        turn(i)
    elif method == "session/cancel":
        if mode == "cancelok": reply(prompt_id, {"stopReason": "cancelled"})
"#;

struct Fixture {
    _dir: tempfile::TempDir,
    script: PathBuf,
    log: PathBuf,
}

fn fixture() -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let script = dir.path().join("fake.py");
    std::fs::write(&script, FAKE).unwrap();
    let log = dir.path().join("log.jsonl");
    Fixture {
        _dir: dir,
        script,
        log,
    }
}

fn store() -> Arc<SecretStore> {
    Arc::new(SecretStore::new(Arc::new(InMemorySecretBackend::default())))
}

fn daemon_env(name: &str) -> Option<String> {
    match name {
        "PATH" | "HOME" => std::env::var(name).ok(),
        "FROM_DAEMON" => Some("daemon-value".to_string()),
        _ => None,
    }
}

/// Small limits so the hostile cases finish quickly.
fn limits() -> AcpLimits {
    AcpLimits {
        message_bytes: 64 * 1024,
        startup: Duration::from_secs(10),
        turn: Duration::from_secs(20),
        idle: Duration::from_secs(10),
        permission: Duration::from_secs(10),
        cancel_grace: Duration::from_secs(5),
        ..AcpLimits::default()
    }
}

impl Fixture {
    fn launch(&self, mode: &str) -> AcpLaunch {
        AcpLaunch {
            program: "python3".into(),
            args: vec![
                self.script.display().to_string(),
                mode.into(),
                self.log.display().to_string(),
            ],
            literal_env: vec![("DECLARED".into(), "yes".into())],
            ..AcpLaunch::default()
        }
    }

    async fn start(
        &self,
        mode: &str,
        limits: AcpLimits,
    ) -> Result<(AcpSession, Receiver<AcpEvent>), AcpError> {
        AcpSession::start(&self.launch(mode), &store(), daemon_env, limits).await
    }

    fn log(&self) -> Vec<(String, Value)> {
        std::fs::read_to_string(&self.log)
            .unwrap_or_default()
            .lines()
            .map(|l| {
                let v: Value = serde_json::from_str(l).unwrap();
                let (k, v) = v.as_object().unwrap().iter().next().unwrap();
                (k.clone(), v.clone())
            })
            .collect()
    }

    fn boot(&self) -> Value {
        self.log().into_iter().find(|(k, _)| k == "boot").unwrap().1
    }

    /// Every message the fake received, in order.
    fn received(&self) -> Vec<Value> {
        self.log()
            .into_iter()
            .filter(|(k, _)| k == "recv")
            .map(|(_, v)| v)
            .collect()
    }

    fn pid(&self) -> i32 {
        self.boot()["pid"].as_i64().unwrap() as i32
    }
}

async fn next(rx: &mut Receiver<AcpEvent>) -> AcpEvent {
    tokio::time::timeout(Duration::from_secs(15), rx.recv())
        .await
        .expect("an event within 15 seconds")
        .expect("the stream ended without a SessionEnded")
}

/// Events up to and including the first that ends the turn or the session.
async fn until_end(rx: &mut Receiver<AcpEvent>) -> Vec<AcpEvent> {
    let mut seen = Vec::new();
    loop {
        let event = next(rx).await;
        let done = matches!(
            event,
            AcpEvent::TurnEnded { .. } | AcpEvent::TurnFailed { .. } | AcpEvent::SessionEnded(_)
        );
        seen.push(event);
        if done {
            return seen;
        }
    }
}

fn alive(pid: i32) -> bool {
    unsafe { libc::kill(pid, 0) == 0 }
}

async fn gone(pid: i32) -> bool {
    for _ in 0..60 {
        if !alive(pid) {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    false
}

fn fault_text(events: &[AcpEvent]) -> String {
    match events.last() {
        Some(AcpEvent::SessionEnded(EndReason::Fault(m))) => m.clone(),
        other => panic!("expected a fault, got {other:?}"),
    }
}

// -- the handshake ---------------------------------------------------------

#[tokio::test]
async fn a_turn_streams_typed_updates_and_keeps_no_tool_input_or_output() {
    let f = fixture();
    let (session, mut rx) = f.start("ok", limits()).await.unwrap();
    assert_eq!(session.info().session_id, "sess-1");
    assert_eq!(session.info().agent_name.as_deref(), Some("fake"));
    assert!(session.info().load_session && session.info().resume);
    assert_eq!(session.info().auth_methods, ["login"]);
    session.prompt("hi").await.unwrap();
    let events = until_end(&mut rx).await;
    let text = |t: &str| {
        AcpEvent::Update(AcpUpdate::AgentText {
            text: t.into(),
            message_id: None,
        })
    };
    assert_eq!(events[0], text("Hel"));
    assert_eq!(events[1], text("lo"));
    assert_eq!(
        events[2],
        AcpEvent::Update(AcpUpdate::AgentThought {
            text: "hmm".into(),
            message_id: None
        })
    );
    // The echo of the user's own message is not an event.
    assert!(matches!(
        events[3],
        AcpEvent::Update(AcpUpdate::ToolCall {
            kind: ToolKind::Read,
            status: Some(ToolStatus::Pending),
            ..
        })
    ));
    assert!(matches!(
        events[4],
        AcpEvent::Update(AcpUpdate::ToolCallUpdate {
            status: Some(ToolStatus::Completed),
            ..
        })
    ));
    assert_eq!(
        events[5],
        AcpEvent::Update(AcpUpdate::Plan {
            markdown: "- [ ] a".into()
        })
    );
    assert_eq!(
        events[6],
        AcpEvent::Update(AcpUpdate::Usage { used: 3, size: 9 })
    );
    assert_eq!(
        events[7],
        AcpEvent::Update(AcpUpdate::Commands {
            names: vec!["nodespace".into()]
        })
    );
    assert_eq!(
        events[8],
        AcpEvent::TurnEnded {
            stop_reason: StopReason::EndTurn
        }
    );
    let rendered = format!("{events:?}");
    assert!(!rendered.contains("BIGSECRET") && !rendered.contains("/secret"));
    session.close().await;
}

#[tokio::test]
async fn initialize_offers_no_capabilities_and_session_new_names_the_private_directory() {
    let f = fixture();
    let (session, _rx) = f.start("ok", limits()).await.unwrap();
    let received = f.received();
    let init = &received[0];
    assert_eq!(init["method"], "initialize");
    assert_eq!(init["params"]["protocolVersion"], 1);
    assert_eq!(
        init["params"]["clientCapabilities"],
        json!({"fs": {"readTextFile": false, "writeTextFile": false}, "terminal": false})
    );
    let new = &received[1];
    assert_eq!(new["method"], "session/new");
    assert_eq!(new["params"]["mcpServers"], json!([]));
    let boot = f.boot();
    let cwd = boot["cwd"].as_str().unwrap();
    assert_eq!(
        new["params"]["cwd"]
            .as_str()
            .map(|c| std::fs::canonicalize(c).unwrap()),
        Some(std::fs::canonicalize(cwd).unwrap())
    );
    assert_eq!(boot["cwdmode"], 0o700);
    session.close().await;
    assert!(
        !std::path::Path::new(cwd).exists(),
        "the private directory is removed"
    );
}

#[tokio::test]
async fn the_environment_is_built_not_inherited() {
    std::env::set_var("NS_ACP_INHERITED", "leaked");
    let f = fixture();
    let (session, _rx) = f.start("ok", limits()).await.unwrap();
    let env: Vec<String> = serde_json::from_value(f.boot()["env"].clone()).unwrap();
    assert!(env.contains(&"DECLARED".to_string()) && env.contains(&"PATH".to_string()));
    assert!(!env.contains(&"NS_ACP_INHERITED".to_string()), "{env:?}");
    session.close().await;
}

#[tokio::test]
async fn a_launch_that_repeats_or_misnames_an_environment_entry_is_refused() {
    let f = fixture();
    let mut launch = f.launch("ok");
    launch.daemon_env = vec!["DECLARED".into()];
    let e = AcpSession::start(&launch, &store(), daemon_env, limits()).await;
    assert!(matches!(e, Err(AcpError::Launch(_))));
    let mut launch = f.launch("ok");
    launch.literal_env.push(("PATH".into(), "/".into()));
    assert!(matches!(
        AcpSession::start(&launch, &store(), daemon_env, limits()).await,
        Err(AcpError::Launch(_))
    ));
    let mut launch = f.launch("ok");
    launch.literal_env.push(("BAD NAME".into(), "x".into()));
    assert!(matches!(
        AcpSession::start(&launch, &store(), daemon_env, limits()).await,
        Err(AcpError::Launch(_))
    ));
    assert!(!f.log.exists(), "nothing was started");
}

#[tokio::test]
async fn an_agent_that_ignores_initialize_is_stopped_at_the_startup_limit() {
    let f = fixture();
    let started = Instant::now();
    let result = f
        .start(
            "ignoreinit",
            AcpLimits {
                startup: Duration::from_secs(1),
                ..limits()
            },
        )
        .await;
    assert!(matches!(result, Err(AcpError::Timeout("starting", 1))));
    assert!(started.elapsed() < Duration::from_millis(3500));
    assert!(gone(f.pid()).await, "the agent was killed");
}

#[tokio::test]
async fn a_protocol_version_other_than_one_and_a_missing_session_id_end_startup() {
    let f = fixture();
    assert!(matches!(
        f.start("badversion", limits()).await,
        Err(AcpError::Protocol(_))
    ));
    assert!(gone(f.pid()).await);
    let f = fixture();
    assert!(matches!(
        f.start("nosid", limits()).await,
        Err(AcpError::Protocol(_))
    ));
}

#[tokio::test]
async fn a_sign_in_error_at_startup_is_typed() {
    let f = fixture();
    assert!(matches!(
        f.start("needauth", limits()).await,
        Err(AcpError::AuthRequired(_))
    ));
    assert!(gone(f.pid()).await);
}

#[tokio::test]
async fn requests_during_startup_are_refused_and_a_wrong_id_answer_is_not_taken() {
    let f = fixture();
    let (session, _rx) = f.start("initrequests", limits()).await.unwrap();
    let refusal = f.received().into_iter().find(|m| m["id"] == 300).unwrap();
    assert_eq!(refusal["error"]["code"], -32601);
    session.close().await;
    let f = fixture();
    let (session, _rx) = f.start("initwrongid", limits()).await.unwrap();
    session.close().await;
}

#[tokio::test]
async fn a_chatty_stderr_does_not_stall_the_agent() {
    let f = fixture();
    let (session, mut rx) = f.start("stderrflood", limits()).await.unwrap();
    session.prompt("hi").await.unwrap();
    // The fake answers no prompt in this mode; the session is simply up.
    session.cancel().await.unwrap();
    drop(rx.try_recv());
    session.close().await;
}

// -- what the agent asks of the client -------------------------------------

#[tokio::test]
async fn fs_terminal_and_unknown_requests_are_refused_and_the_turn_continues() {
    let f = fixture();
    let (session, mut rx) = f.start("requests", limits()).await.unwrap();
    session.prompt("hi").await.unwrap();
    let events = until_end(&mut rx).await;
    assert_eq!(
        events.last(),
        Some(&AcpEvent::TurnEnded {
            stop_reason: StopReason::EndTurn
        })
    );
    let answers: Vec<Value> = f
        .received()
        .into_iter()
        .filter(|m| (200..204).contains(&m["id"].as_i64().unwrap_or(0)))
        .collect();
    assert_eq!(answers.len(), 4);
    for a in answers {
        assert_eq!(a["error"]["code"], -32601, "{a}");
        assert!(a.get("result").is_none());
    }
    session.close().await;
}

#[tokio::test]
async fn a_permission_request_is_surfaced_with_only_once_choices_and_never_auto_answered() {
    let f = fixture();
    let (session, mut rx) = f.start("perm", limits()).await.unwrap();
    session.prompt("hi").await.unwrap();
    let AcpEvent::PermissionRequest(request) = next(&mut rx).await else {
        panic!("expected a permission request")
    };
    assert_eq!(request.title, "Run ls");
    let ids: Vec<&str> = request
        .options
        .iter()
        .map(|o| o.option_id.as_str())
        .collect();
    assert_eq!(ids, ["once", "no"]);
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(
        !f.received().iter().any(|m| m["id"] == 100),
        "nothing answers the request until the user does"
    );
    // An "always" choice the agent offered is not accepted, nor is an invented one.
    for bad in ["always", "never", "made-up"] {
        assert_eq!(
            session
                .answer_permission(request.request, PermissionChoice::Select(bad.into()))
                .await,
            Err(AcpError::InvalidOption)
        );
    }
    assert_eq!(
        session
            .answer_permission(999, PermissionChoice::Cancel)
            .await,
        Err(AcpError::UnknownPermission)
    );
    session
        .answer_permission(request.request, PermissionChoice::Select("once".into()))
        .await
        .unwrap();
    assert_eq!(
        next(&mut rx).await,
        AcpEvent::PermissionResolved {
            request: request.request,
            resolution: PermissionResolution::Answered
        }
    );
    let events = until_end(&mut rx).await;
    assert_eq!(
        events.last(),
        Some(&AcpEvent::TurnEnded {
            stop_reason: StopReason::EndTurn
        })
    );
    let answer = f.received().into_iter().find(|m| m["id"] == 100).unwrap();
    assert_eq!(
        answer["result"],
        json!({"outcome": {"outcome": "selected", "optionId": "once"}})
    );
    // Answered twice is unknown the second time.
    assert_eq!(
        session
            .answer_permission(request.request, PermissionChoice::Cancel)
            .await,
        Err(AcpError::UnknownPermission)
    );
    session.close().await;
}

#[tokio::test]
async fn a_user_cancel_choice_and_a_timeout_both_answer_cancelled() {
    let f = fixture();
    let (session, mut rx) = f.start("perm", limits()).await.unwrap();
    session.prompt("hi").await.unwrap();
    let AcpEvent::PermissionRequest(request) = next(&mut rx).await else {
        panic!()
    };
    session
        .answer_permission(request.request, PermissionChoice::Cancel)
        .await
        .unwrap();
    until_end(&mut rx).await;
    let answer = f.received().into_iter().find(|m| m["id"] == 100).unwrap();
    assert_eq!(
        answer["result"],
        json!({"outcome": {"outcome": "cancelled"}})
    );
    session.close().await;

    let f = fixture();
    let (session, mut rx) = f
        .start(
            "perm",
            AcpLimits {
                permission: Duration::from_millis(300),
                ..limits()
            },
        )
        .await
        .unwrap();
    session.prompt("hi").await.unwrap();
    let AcpEvent::PermissionRequest(request) = next(&mut rx).await else {
        panic!()
    };
    assert_eq!(
        next(&mut rx).await,
        AcpEvent::PermissionResolved {
            request: request.request,
            resolution: PermissionResolution::TimedOut
        }
    );
    until_end(&mut rx).await;
    let answer = f.received().into_iter().find(|m| m["id"] == 100).unwrap();
    assert_eq!(
        answer["result"],
        json!({"outcome": {"outcome": "cancelled"}})
    );
    session.close().await;
}

#[tokio::test]
async fn a_permission_flood_is_capped_at_the_pending_limit_and_the_rest_answered_cancelled() {
    let f = fixture();
    let (session, mut rx) = f
        .start(
            "permflood",
            AcpLimits {
                pending_permissions: 3,
                ..limits()
            },
        )
        .await
        .unwrap();
    session.prompt("hi").await.unwrap();
    let mut surfaced = Vec::new();
    for _ in 0..3 {
        let AcpEvent::PermissionRequest(r) = next(&mut rx).await else {
            panic!()
        };
        surfaced.push(r.request);
    }
    // The other seventeen were answered cancelled at once.
    for _ in 0..40 {
        if f.received()
            .iter()
            .filter(|m| m["result"]["outcome"]["outcome"] == "cancelled")
            .count()
            >= 17
        {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert_eq!(
        f.received()
            .iter()
            .filter(|m| m["result"]["outcome"]["outcome"] == "cancelled")
            .count(),
        17
    );
    for request in surfaced {
        session
            .answer_permission(request, PermissionChoice::Cancel)
            .await
            .unwrap();
    }
    loop {
        match next(&mut rx).await {
            AcpEvent::TurnEnded { .. } => break,
            AcpEvent::PermissionResolved { .. } => {}
            other => panic!("{other:?}"),
        }
    }
    session.close().await;
}

#[tokio::test]
async fn too_many_permission_requests_in_one_turn_end_the_session() {
    let f = fixture();
    let (session, mut rx) = f
        .start(
            "permhuge",
            AcpLimits {
                turn_permissions: 5,
                pending_permissions: 2,
                ..limits()
            },
        )
        .await
        .unwrap();
    session.prompt("hi").await.unwrap();
    let mut events = Vec::new();
    loop {
        let e = next(&mut rx).await;
        let end = matches!(e, AcpEvent::SessionEnded(_));
        events.push(e);
        if end {
            break;
        }
    }
    assert!(
        fault_text(&events).contains("permission requests"),
        "{events:?}"
    );
    assert!(gone(f.pid()).await);
}

// -- cancel ----------------------------------------------------------------

#[tokio::test]
async fn cancel_answers_waiting_permissions_cancelled_and_ends_with_the_cancelled_reason() {
    let f = fixture();
    let (session, mut rx) = f.start("permcancel", limits()).await.unwrap();
    session.prompt("hi").await.unwrap();
    let AcpEvent::PermissionRequest(request) = next(&mut rx).await else {
        panic!()
    };
    session.cancel().await.unwrap();
    assert_eq!(
        next(&mut rx).await,
        AcpEvent::PermissionResolved {
            request: request.request,
            resolution: PermissionResolution::Cancelled
        }
    );
    assert_eq!(
        next(&mut rx).await,
        AcpEvent::TurnEnded {
            stop_reason: StopReason::Cancelled
        }
    );
    let received = f.received();
    assert!(received
        .iter()
        .any(|m| m["method"] == "session/cancel" && m["params"]["sessionId"] == "sess-1"));
    let answer = received.iter().find(|m| m["id"] == 100).unwrap();
    assert_eq!(
        answer["result"],
        json!({"outcome": {"outcome": "cancelled"}})
    );
    session.close().await;
}

#[tokio::test]
async fn cancel_ends_a_running_turn_and_the_session_is_ready_for_another() {
    let f = fixture();
    let (session, mut rx) = f.start("cancelok", limits()).await.unwrap();
    session.prompt("hi").await.unwrap();
    assert!(matches!(next(&mut rx).await, AcpEvent::Update(_)));
    assert_eq!(session.prompt("again").await, Err(AcpError::Busy));
    session.cancel().await.unwrap();
    assert_eq!(
        next(&mut rx).await,
        AcpEvent::TurnEnded {
            stop_reason: StopReason::Cancelled
        }
    );
    session.prompt("again").await.unwrap();
    session.close().await;
}

#[tokio::test]
async fn an_agent_that_ignores_cancel_is_stopped_after_the_grace_period() {
    let f = fixture();
    let (session, mut rx) = f
        .start(
            "cancelignore",
            AcpLimits {
                cancel_grace: Duration::from_millis(400),
                ..limits()
            },
        )
        .await
        .unwrap();
    session.prompt("hi").await.unwrap();
    assert!(matches!(next(&mut rx).await, AcpEvent::Update(_)));
    session.cancel().await.unwrap();
    let events = until_end(&mut rx).await;
    assert!(fault_text(&events).contains("after it was cancelled"));
    assert!(gone(f.pid()).await);
    assert_eq!(session.prompt("x").await, Err(AcpError::Closed));
}

// -- bounds ----------------------------------------------------------------

#[tokio::test]
async fn one_oversize_message_is_dropped_without_being_held_and_the_turn_continues() {
    let f = fixture();
    let (session, mut rx) = f.start("hugemsg", limits()).await.unwrap();
    session.prompt("hi").await.unwrap();
    let events = until_end(&mut rx).await;
    assert_eq!(events.len(), 2, "{events:?}");
    assert_eq!(
        events[1],
        AcpEvent::TurnEnded {
            stop_reason: StopReason::EndTurn
        }
    );
    session.close().await;
}

#[tokio::test]
async fn repeated_oversize_messages_end_the_session() {
    let f = fixture();
    let (session, mut rx) = f
        .start(
            "manyhuge",
            AcpLimits {
                max_oversize: 3,
                ..limits()
            },
        )
        .await
        .unwrap();
    session.prompt("hi").await.unwrap();
    let events = until_end(&mut rx).await;
    assert!(fault_text(&events).contains("over the size limit"));
    assert!(gone(f.pid()).await);
}

#[tokio::test]
async fn never_ending_updates_hit_the_event_cap() {
    let f = fixture();
    let (session, mut rx) = f
        .start(
            "endless",
            AcpLimits {
                turn_events: 500,
                turn_bytes: u64::MAX / 2,
                ..limits()
            },
        )
        .await
        .unwrap();
    session.prompt("hi").await.unwrap();
    let events = until_end(&mut rx).await;
    assert!(fault_text(&events).contains("more updates"));
    assert!(gone(f.pid()).await);
}

#[tokio::test]
async fn never_ending_updates_hit_the_byte_cap() {
    let f = fixture();
    let (session, mut rx) = f
        .start(
            "endless",
            AcpLimits {
                turn_bytes: 100_000,
                turn_events: u32::MAX,
                ..limits()
            },
        )
        .await
        .unwrap();
    session.prompt("hi").await.unwrap();
    let events = until_end(&mut rx).await;
    assert!(fault_text(&events).contains("more than the limit"));
    assert!(gone(f.pid()).await);
}

#[tokio::test]
async fn a_slow_endless_turn_hits_the_turn_timeout_and_a_silent_one_the_idle_timeout() {
    let f = fixture();
    let (session, mut rx) = f
        .start(
            "endlessslow",
            AcpLimits {
                turn: Duration::from_millis(600),
                turn_events: u32::MAX,
                ..limits()
            },
        )
        .await
        .unwrap();
    session.prompt("hi").await.unwrap();
    let events = until_end(&mut rx).await;
    assert!(fault_text(&events).contains("the turn"), "{events:?}");
    assert!(gone(f.pid()).await);

    let f = fixture();
    let (session, mut rx) = f
        .start(
            "silent",
            AcpLimits {
                idle: Duration::from_millis(500),
                ..limits()
            },
        )
        .await
        .unwrap();
    session.prompt("hi").await.unwrap();
    let events = until_end(&mut rx).await;
    assert!(
        fault_text(&events).contains("anything during the turn"),
        "{events:?}"
    );
    assert!(gone(f.pid()).await);
}

#[tokio::test]
async fn answers_with_the_wrong_id_are_ignored_and_too_many_end_the_session() {
    let f = fixture();
    let (session, mut rx) = f.start("wrongid", limits()).await.unwrap();
    session.prompt("hi").await.unwrap();
    let events = until_end(&mut rx).await;
    assert_eq!(
        events,
        vec![AcpEvent::TurnEnded {
            stop_reason: StopReason::EndTurn
        }]
    );
    session.close().await;

    let f = fixture();
    let (session, mut rx) = f
        .start(
            "manywrongid",
            AcpLimits {
                agent_requests: 10,
                ..limits()
            },
        )
        .await
        .unwrap();
    session.prompt("hi").await.unwrap();
    let events = until_end(&mut rx).await;
    assert!(fault_text(&events).contains("not wanted"));
    assert!(gone(f.pid()).await);
}

#[tokio::test]
async fn some_noise_is_tolerated_and_a_flood_of_it_ends_the_session() {
    let f = fixture();
    let (session, mut rx) = f.start("noise", limits()).await.unwrap();
    session.prompt("hi").await.unwrap();
    let events = until_end(&mut rx).await;
    assert_eq!(events.len(), 2, "{events:?}");
    session.close().await;

    let f = fixture();
    let (session, mut rx) = f
        .start(
            "manynoise",
            AcpLimits {
                noise_lines: 20,
                ..limits()
            },
        )
        .await
        .unwrap();
    session.prompt("hi").await.unwrap();
    let events = until_end(&mut rx).await;
    assert!(fault_text(&events).contains("not JSON-RPC"));
    assert!(gone(f.pid()).await);
}

#[tokio::test]
async fn a_deeply_nested_line_is_noise_not_a_crash() {
    let f = fixture();
    let (session, mut rx) = f.start("deep", limits()).await.unwrap();
    session.prompt("hi").await.unwrap();
    let events = until_end(&mut rx).await;
    assert_eq!(events.len(), 2, "{events:?}");
    session.close().await;
}

#[tokio::test]
async fn updates_for_another_session_are_ignored() {
    let f = fixture();
    let (session, mut rx) = f.start("foreign", limits()).await.unwrap();
    session.prompt("hi").await.unwrap();
    let events = until_end(&mut rx).await;
    assert_eq!(events.len(), 2, "{events:?}");
    assert!(
        matches!(&events[0], AcpEvent::Update(AcpUpdate::AgentText { text, .. }) if text == "yours")
    );
    session.close().await;
}

#[tokio::test]
async fn a_prompt_over_the_limit_is_refused_before_it_is_sent() {
    let f = fixture();
    let (session, _rx) = f
        .start(
            "ok",
            AcpLimits {
                prompt_bytes: 10,
                ..limits()
            },
        )
        .await
        .unwrap();
    assert_eq!(
        session.prompt("0123456789ab").await,
        Err(AcpError::PromptTooLarge)
    );
    assert!(!f.received().iter().any(|m| m["method"] == "session/prompt"));
    session.close().await;
}

// -- ends ------------------------------------------------------------------

#[tokio::test]
async fn an_agent_that_dies_mid_turn_ends_the_session_with_agent_exited() {
    let f = fixture();
    let (session, mut rx) = f.start("die", limits()).await.unwrap();
    session.prompt("hi").await.unwrap();
    let events = until_end(&mut rx).await;
    assert_eq!(
        events.last(),
        Some(&AcpEvent::SessionEnded(EndReason::AgentExited))
    );
    assert_eq!(session.prompt("x").await, Err(AcpError::Closed));
}

#[tokio::test]
async fn a_prompt_error_is_a_failed_turn_and_a_missing_stop_reason_ends_the_session() {
    let f = fixture();
    let (session, mut rx) = f.start("turnerror", limits()).await.unwrap();
    session.prompt("hi").await.unwrap();
    let events = until_end(&mut rx).await;
    assert!(matches!(
        &events[0],
        AcpEvent::TurnFailed {
            code: -32000,
            auth_required: true,
            ..
        }
    ));
    // The session stays up.
    session.prompt("again").await.unwrap();
    session.close().await;

    let f = fixture();
    let (_session, mut rx) = f.start("badstop", limits()).await.unwrap();
    _session.prompt("hi").await.unwrap();
    let events = until_end(&mut rx).await;
    assert!(fault_text(&events).contains("no stop reason"));
}

#[tokio::test]
async fn other_stop_reasons_are_typed() {
    let f = fixture();
    let (session, mut rx) = f.start("weirdstop", limits()).await.unwrap();
    session.prompt("hi").await.unwrap();
    assert_eq!(
        next(&mut rx).await,
        AcpEvent::TurnEnded {
            stop_reason: StopReason::MaxTokens
        }
    );
    session.close().await;
}

#[tokio::test]
async fn dropping_the_handle_kills_the_agent() {
    let f = fixture();
    let (session, _rx) = f.start("hold", limits()).await.unwrap();
    let pid = f.pid();
    assert!(alive(pid));
    drop(session);
    assert!(gone(pid).await);
}

#[tokio::test]
async fn close_stops_the_agent_and_ends_the_stream() {
    let f = fixture();
    let (session, mut rx) = f.start("hold", limits()).await.unwrap();
    let pid = f.pid();
    session.close().await;
    assert!(gone(pid).await);
    assert_eq!(
        next(&mut rx).await,
        AcpEvent::SessionEnded(EndReason::Closed)
    );
}

#[tokio::test]
async fn a_reader_that_stops_reading_events_ends_the_session() {
    let f = fixture();
    let (session, rx) = f.start("ok", limits()).await.unwrap();
    drop(rx);
    session.prompt("hi").await.unwrap();
    for _ in 0..60 {
        if session.prompt("x").await == Err(AcpError::Closed) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(gone(f.pid()).await);
}

// -- credentials -----------------------------------------------------------

#[tokio::test]
async fn a_credential_is_removed_from_the_raw_bytes_before_anything_is_parsed() {
    let f = fixture();
    let secrets = store();
    secrets.set("tok", "TOKENVALUE-123").unwrap();
    let mut launch = f.launch("echosecret");
    launch.secrets = vec![EnvSecret {
        name: "ACP_TOKEN".into(),
        secret: "keychain:tok".into(),
    }];
    let (session, mut rx) = AcpSession::start(&launch, &secrets, daemon_env, limits())
        .await
        .unwrap();
    assert!(f.boot()["env"]
        .as_array()
        .unwrap()
        .iter()
        .any(|v| v == "ACP_TOKEN"));
    session.prompt("hi").await.unwrap();
    let events = until_end(&mut rx).await;
    let rendered = format!("{events:?}");
    assert!(rendered.contains("[redacted]"), "{rendered}");
    assert!(!rendered.contains("TOKENVALUE"), "{rendered}");
    assert!(
        matches!(&events[1], AcpEvent::TurnFailed { code: -32001, message, .. } if message == "bad [redacted]")
    );
    assert_eq!(session.redact("a TOKENVALUE-123 b"), "a [redacted] b");
    session.close().await;
}

#[tokio::test]
async fn a_hung_credential_store_ends_startup_at_the_limit_before_anything_runs() {
    let f = fixture();
    let secrets = Arc::new(SecretStore::new(Arc::new(
        crate::local_agent::test_http::SlowBackend(Duration::from_secs(3)),
    )));
    let mut launch = f.launch("ok");
    launch.secrets = vec![EnvSecret {
        name: "ACP_TOKEN".into(),
        secret: "keychain:tok".into(),
    }];
    let started = Instant::now();
    let result = AcpSession::start(
        &launch,
        &secrets,
        daemon_env,
        AcpLimits {
            startup: Duration::from_secs(1),
            ..limits()
        },
    )
    .await;
    assert!(matches!(result, Err(AcpError::Credential(m)) if m.contains("within 1 seconds")));
    assert!(started.elapsed() < Duration::from_millis(2500));
    assert!(
        !f.log.exists(),
        "the agent was started before the credential was read"
    );
}

#[tokio::test]
async fn a_credential_sent_as_json_escapes_is_removed_from_every_surfaced_string() {
    let f = fixture();
    let secrets = store();
    let token = "TOK\"EN\\/VALUE-\u{e9}9";
    secrets.set("tok", token).unwrap();
    let mut launch = f.launch("escaped");
    launch.secrets = vec![EnvSecret {
        name: "ACP_TOKEN".into(),
        secret: "keychain:tok".into(),
    }];
    let (session, mut rx) = AcpSession::start(&launch, &secrets, daemon_env, limits())
        .await
        .unwrap();
    session.prompt("hi").await.unwrap();
    let mut events = Vec::new();
    loop {
        let event = next(&mut rx).await;
        if let AcpEvent::PermissionRequest(r) = &event {
            session
                .answer_permission(r.request, PermissionChoice::Cancel)
                .await
                .unwrap();
        }
        let done = matches!(
            event,
            AcpEvent::TurnEnded { .. } | AcpEvent::SessionEnded(_)
        );
        events.push(event);
        if done {
            break;
        }
    }
    // Text, thought, plan, title, tool call, command name, permission option name.
    assert!(events.len() >= 8, "{events:?}");
    let shown = format!("{events:?}");
    let chars: Vec<char> = token.chars().collect();
    for window in chars.windows(4) {
        let piece: String = window.iter().collect();
        assert!(
            !shown.contains(&piece),
            "a prefix of the credential leaked: {piece}\n{shown}"
        );
        assert!(
            !shown.contains(&piece.escape_debug().to_string()),
            "{piece}"
        );
    }
    assert!(shown.contains("[redacted]"));
    session.close().await;
}

#[tokio::test]
async fn the_idle_clock_does_not_run_while_a_permission_waits_and_restarts_when_it_resolves() {
    let f = fixture();
    let (session, mut rx) = f
        .start(
            "perm",
            AcpLimits {
                idle: Duration::from_millis(400),
                permission: Duration::from_millis(1500),
                ..limits()
            },
        )
        .await
        .unwrap();
    session.prompt("hi").await.unwrap();
    let AcpEvent::PermissionRequest(request) = next(&mut rx).await else {
        panic!()
    };
    // Past the idle limit, the session is still up and nothing has ended it.
    let quiet = tokio::time::timeout(Duration::from_millis(900), rx.recv()).await;
    assert!(
        quiet.is_err(),
        "the session ended while a permission waited: {quiet:?}"
    );
    // It ends on the permission timeout: answered cancelled, and the agent then
    // gets the idle period to finish the turn.
    assert_eq!(
        next(&mut rx).await,
        AcpEvent::PermissionResolved {
            request: request.request,
            resolution: PermissionResolution::TimedOut
        }
    );
    let events = until_end(&mut rx).await;
    assert_eq!(
        events.last(),
        Some(&AcpEvent::TurnEnded {
            stop_reason: StopReason::EndTurn
        })
    );
    session.close().await;
}
