//! The runner of `tool-cli` tools: runs the program a binding names, with the
//! arguments the model chose, and returns what it printed.
//!
//! The rules it keeps (each is tested against fake programs):
//!
//! - **The binding is read through the typed reader** ([`ToolCliFields`]) and
//!   must be complete; anything malformed fails closed before a process starts.
//! - **No shell.** The program is spawned directly with an argv vector. Each
//!   `{name}` placeholder in the binding is one whole argv element, whatever
//!   the model's value holds (spaces, `;`, `$(...)`, quotes, a leading `-`).
//!   A value is a string, number or boolean; anything else, or a string with a
//!   NUL, is refused. A placeholder whose argument the model left out is
//!   dropped.
//! - **`--` before model-supplied values.** When at least one model-supplied
//!   element is present, a `--` element is inserted before the first one, so a
//!   value that looks like an option is a positional argument.
//! - **The environment is built, not inherited.** The child gets a short fixed
//!   list of the daemon's variables ([`BASE_ENV`]) and the entries the binding
//!   declares; nothing else.
//! - **Credentials.** A declared `env` entry's `keychain:` or `env:` reference
//!   is resolved here, at the point the child's environment is built, and goes
//!   to that one child only. It is never an argument, is held in memory that is
//!   wiped when dropped, and is redacted from everything this runner returns.
//!   A credential too short to redact safely is refused. When output is cut
//!   (the program is stopped at the cap) each stream is redacted whole and any
//!   trailing part of a credential the read stopped inside of is removed, so no
//!   prefix of a credential is ever returned.
//! - **Limits.** A timeout kills the program and everything it started (its
//!   process group); output is read as a stream and capped, and the program is
//!   killed when the cap is reached, rather than buffered and cut afterwards.
//!
//! What `--` does and does not do: it goes before the first model-supplied
//! element, so the binding's own fixed arguments that follow a placeholder also
//! come after it (`["log", "{q}"]` runs `git log -- main`, which names paths,
//! not a revision; `["{q}", "--out", "x"]` passes `--out` as a positional
//! argument). A program that does not honour `--` receives a literal `--`
//! argument. Option injection through a value is therefore prevented only for
//! programs that honour `--` (git, ssh, curl and most others do). A binding
//! that is itself dangerous (an interpreter run with `-c`, say) is within what
//! the user approved: the approval covers the binding, not what it can do.
//!
//! Process handling: the program runs in its own process group, which is
//! killed on a timeout, at the output cap, and when the call is cancelled
//! (the future is dropped). A background process the program leaves running
//! after it exits normally, and one that leaves the group (`setsid`), are not
//! stopped. The program starts in a fresh private (`0700`) empty directory that
//! is removed afterwards. It inherits the daemon's `PATH` (so bare program
//! names resolve as they do for the user, Homebrew paths included), the file
//! descriptors the platform does not mark close-on-exec, the umask and the
//! signal mask; another process of the same user can read its environment.
//!
//! Limits of the redaction: the exact value is replaced. A program that prints
//! a transformed form of a credential (base64, URL-encoded, split across
//! lines) is not caught. The child's environment copy of the value is held by
//! the process machinery and is not wiped.

use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use nodespace_core::secrets::SecretStore;
use nodespace_types::{ToolCliFields, DEFAULT_RESPONSE_BYTES, DEFAULT_TIMEOUT_SECONDS};
use serde_json::Value;
use tokio::io::AsyncReadExt;
use zeroize::Zeroizing;

pub use super::process::BASE_ENV;
use super::process::{
    environment_note, program_note, reap, resolve_env_credentials, spawn_isolated, stop, EnvLookup,
    Isolated,
};
use super::redaction::{cut, sanitize};
use super::remote_tools::{
    RemoteCallError, RemoteOutput, RemoteTool, RemoteToolKind, RemoteToolRunner,
};

/// Read size for the program's output streams.
const READ_CHUNK: usize = 8192;
/// How much of a failing program's output an error message carries.
const ERROR_OUTPUT_BYTES: usize = 1024;
/// Longest confirmation text.
const MAX_DESTINATION_BYTES: usize = 4096;

/// Runs `tool-cli` tools.
pub struct CliRunner {
    secrets: Arc<SecretStore>,
    daemon_env: EnvLookup,
}

impl CliRunner {
    /// A runner whose credentials come from `secrets` and whose base
    /// environment comes from this process.
    pub fn new(secrets: Arc<SecretStore>) -> Self {
        Self {
            secrets,
            daemon_env: Arc::new(|name| std::env::var(name).ok()),
        }
    }

    /// A runner reading the daemon's variables (for `env:` references and the
    /// base environment) through `lookup`, for tests.
    pub fn with_env_lookup(
        secrets: Arc<SecretStore>,
        lookup: impl Fn(&str) -> Option<String> + Send + Sync + 'static,
    ) -> Self {
        Self {
            secrets,
            daemon_env: Arc::new(lookup),
        }
    }
}

/// What to run: the program and its argv.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Invocation {
    program: String,
    argv: Vec<String>,
}

fn fail<T>(message: impl Into<String>) -> Result<T, RemoteCallError> {
    Err(RemoteCallError(message.into()))
}

/// Decodes the binding and requires it complete. Fails closed.
fn read_binding(tool: &RemoteTool) -> Result<ToolCliFields, RemoteCallError> {
    let fields = ToolCliFields::from_bucket(tool.binding())
        .map_err(|e| RemoteCallError(format!("the tool's binding is not valid: {}", e.0)))?;
    fields
        .check_complete(Some(&tool.parameter_schema))
        .map_err(|e| RemoteCallError(format!("the tool's binding is not complete: {}", e.0)))?;
    Ok(fields)
}

/// One model-supplied value as an argv element.
fn element(name: &str, value: &Value) -> Result<String, RemoteCallError> {
    let text = match value {
        Value::String(s) => s.clone(),
        Value::Number(n) => n.to_string(),
        Value::Bool(b) => b.to_string(),
        _ => return fail(format!("`{name}` must be a string, number or boolean")),
    };
    if text.contains('\0') {
        return fail(format!("`{name}` cannot contain a NUL character"));
    }
    Ok(text)
}

/// The program and argv for a call: literals as written, each placeholder one
/// whole element, `--` before the first model-supplied one.
fn invocation(fields: &ToolCliFields, args: &Value) -> Result<Invocation, RemoteCallError> {
    let program = fields
        .program
        .clone()
        .ok_or_else(|| RemoteCallError("the tool's binding names no program".to_string()))?;
    let mut argv = Vec::new();
    let mut separated = false;
    for item in fields.args.iter().flatten() {
        match whole_placeholder(item) {
            None => argv.push(item.clone()),
            Some(name) => {
                let Some(value) = args.get(name) else {
                    continue;
                };
                if !separated {
                    argv.push("--".to_string());
                    separated = true;
                }
                argv.push(element(name, value)?);
            }
        }
    }
    Ok(Invocation { program, argv })
}

/// The parameter a whole-element `{name}` item names.
fn whole_placeholder(item: &str) -> Option<&str> {
    item.strip_prefix('{')
        .and_then(|rest| rest.strip_suffix('}'))
}

#[async_trait]
impl RemoteToolRunner for CliRunner {
    fn kind(&self) -> RemoteToolKind {
        RemoteToolKind::Cli
    }

    fn destination(&self, tool: &RemoteTool, args: &Value) -> String {
        let fields = match read_binding(tool) {
            Ok(fields) => fields,
            Err(e) => return format!("(unusable binding: {})", e.0),
        };
        // The credentials come first and are bounded (see `environment_note`).
        let environment = environment_note(fields.env.as_deref().unwrap_or_default());
        let call = match invocation(&fields, args) {
            Ok(call) => call,
            Err(e) => return format!("(unusable call: {}){environment}", e.0),
        };
        let head = format!("run {}", program_note(&call.program, &self.daemon_env));
        let argv = serde_json::to_string(&call.argv).unwrap_or_default();
        let prefix = "; arguments: ";
        let budget =
            MAX_DESTINATION_BYTES.saturating_sub(head.len() + environment.len() + prefix.len() + 3);
        let mut shown_argv = argv.clone();
        if argv.len() > budget {
            cut(&mut shown_argv, budget);
            shown_argv.push('\u{2026}');
        }
        format!("{head}{environment}{prefix}{shown_argv}")
    }

    async fn run(&self, tool: &RemoteTool, args: &Value) -> Result<RemoteOutput, RemoteCallError> {
        let fields = read_binding(tool)?;
        let call = invocation(&fields, args)?;
        let credentials = resolve_env_credentials(
            &self.secrets,
            &self.daemon_env,
            fields.env.as_deref().unwrap_or_default(),
        )
        .await?;
        let needles: Vec<Zeroizing<String>> = credentials.iter().map(|(_, v)| v.clone()).collect();
        let cap = fields.max_output_bytes.unwrap_or(DEFAULT_RESPONSE_BYTES) as usize;
        let limit = Duration::from_secs(fields.timeout_seconds.unwrap_or(DEFAULT_TIMEOUT_SECONDS));

        let mut process = spawn_isolated(
            &call.program,
            &call.argv,
            &self.daemon_env,
            &credentials,
            Stdio::null(),
        )?;
        drop(credentials);
        let Isolated { guard, child, .. } = &mut process;

        let mut stdout = child.stdout.take().expect("stdout is piped");
        let mut stderr = child.stderr.take().expect("stderr is piped");
        // Reading stops, and the program is killed, once this much has been read.
        let read_limit = cap;

        let work = async {
            let mut out = Vec::new();
            let mut err = Vec::new();
            let (mut out_open, mut err_open) = (true, true);
            let mut chunk_out = vec![0u8; READ_CHUNK];
            let mut chunk_err = vec![0u8; READ_CHUNK];
            let mut stopped_at_cap = false;
            while out_open || err_open {
                tokio::select! {
                    read = stdout.read(&mut chunk_out), if out_open => match read {
                        Ok(n) if n > 0 => out.extend_from_slice(&chunk_out[..n]),
                        _ => out_open = false,
                    },
                    read = stderr.read(&mut chunk_err), if err_open => match read {
                        Ok(n) if n > 0 => err.extend_from_slice(&chunk_err[..n]),
                        _ => err_open = false,
                    },
                }
                if out.len() + err.len() >= read_limit {
                    stopped_at_cap = true;
                    stop(child);
                    break;
                }
            }
            let status = if stopped_at_cap {
                // Killed: a bounded wait, and its status says nothing.
                reap(child).await;
                child.try_wait()
            } else {
                child.wait().await.map(Some)
            };
            (out, err, stopped_at_cap, status)
        };

        let (out, err, stopped_at_cap, status) = match tokio::time::timeout(limit, work).await {
            Ok(done) => done,
            Err(_) => {
                stop(child);
                reap(child).await;
                if matches!(child.try_wait(), Ok(Some(_))) {
                    guard.disarm();
                }
                return fail(format!(
                    "the program did not finish within {} seconds and was stopped",
                    limit.as_secs()
                ));
            }
        };

        if matches!(status, Ok(Some(_))) {
            guard.disarm();
        }
        // Reading stopped at an arbitrary byte when the program was stopped at
        // the cap, so a stream can end part way through a credential (or a
        // character of one): each stream is sanitized as raw bytes.
        let out_text = sanitize(&out, &needles, stopped_at_cap);
        let err_text = sanitize(&err, &needles, stopped_at_cap);
        let mut text = out_text;
        if !err_text.is_empty() {
            text.push_str("\n[stderr]\n");
            text.push_str(&err_text);
        }
        let truncated = stopped_at_cap || text.len() > cap;
        cut(&mut text, cap);

        match status {
            // The program was stopped because it had said enough: what it said
            // is the result, marked cut, whatever its status was when killed.
            _ if stopped_at_cap => Ok(RemoteOutput { text, truncated }),
            Ok(Some(status)) if status.success() => Ok(RemoteOutput { text, truncated }),
            Ok(None) => fail("the program had not finished when its output ended"),
            Ok(Some(status)) => {
                let code = status
                    .code()
                    .map(|c| c.to_string())
                    .unwrap_or_else(|| "a signal".to_string());
                cut(&mut text, ERROR_OUTPUT_BYTES);
                fail(format!("the program exited with status {code}: {text}"))
            }
            Err(e) => fail(format!("the program could not be waited for: {}", e.kind())),
        }
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use crate::local_agent::process::ScratchDir;
    use nodespace_core::secrets::InMemorySecretBackend;
    use serde_json::json;
    use std::path::{Path, PathBuf};
    use std::time::Instant;
    use tempfile::TempDir;

    /// A fake program: an executable shell script in `dir`.
    fn script(dir: &Path, name: &str, body: &str) -> String {
        use std::os::unix::fs::PermissionsExt;
        let path = dir.join(name);
        std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        path.to_string_lossy().into_owned()
    }

    fn schema() -> Value {
        json!({
            "type": "object",
            "additionalProperties": false,
            "properties": {
                "q": {"type": "string"},
                "r": {"type": "string"},
                "n": {"type": "integer"},
                "flag": {"type": "boolean"},
                "o": {"type": "object"},
            },
        })
    }

    fn tool(binding: Value) -> RemoteTool {
        RemoteTool {
            node_id: "node-1".into(),
            name: "c_tool".into(),
            kind: RemoteToolKind::Cli,
            description: "d".into(),
            parameter_schema: schema(),
            properties: json!({ "tool-cli": binding }),
            approval_tag: String::new(),
        }
    }

    fn store() -> Arc<SecretStore> {
        Arc::new(SecretStore::new(Arc::new(InMemorySecretBackend::default())))
    }

    /// A runner whose view of the daemon's environment is fixed.
    fn runner(secrets: Arc<SecretStore>) -> CliRunner {
        CliRunner::with_env_lookup(secrets, |name| match name {
            "PATH" => Some("/usr/bin:/bin".to_string()),
            "HOME" => Some("/tmp".to_string()),
            "HOST_TOKEN" => Some("ENVVALUE-456".to_string()),
            _ => None,
        })
    }

    async fn run(
        runner: &CliRunner,
        binding: Value,
        args: Value,
    ) -> Result<RemoteOutput, RemoteCallError> {
        runner.run(&tool(binding), &args).await
    }

    const PRINT_ARGS: &str = "for a in \"$@\"; do printf '[%s]\\n' \"$a\"; done";

    #[tokio::test]
    async fn each_placeholder_is_one_whole_element_and_nothing_goes_through_a_shell() {
        let dir = TempDir::new().unwrap();
        let program = script(dir.path(), "args.sh", PRINT_ARGS);
        let flag: PathBuf = dir.path().join("injected");
        let nasty = format!(
            "a b; touch {0} $(touch {0}) `touch {0}` * 'q\" && touch {0}",
            flag.display()
        );
        let out = run(
            &runner(store()),
            json!({"program": program, "args": ["status", "{q}"]}),
            json!({ "q": nasty }),
        )
        .await
        .unwrap();
        assert_eq!(out.text, format!("[status]\n[--]\n[{nasty}]\n"));
        assert!(!flag.exists(), "a shell ran the model's text");
    }

    #[tokio::test]
    async fn the_separator_goes_before_the_first_model_value_and_only_when_there_is_one() {
        let dir = TempDir::new().unwrap();
        let program = script(dir.path(), "args.sh", PRINT_ARGS);
        let binding = json!({"program": program, "args": ["-x", "{q}", "-y", "{r}"]});
        let r = runner(store());
        // No value: no separator.
        let none = run(&r, binding.clone(), json!({})).await.unwrap();
        assert_eq!(none.text, "[-x]\n[-y]\n");
        // Two values: one separator, before the first, and the literal between
        // them keeps its place.
        let two = run(&r, binding.clone(), json!({"q": "1", "r": "2"}))
            .await
            .unwrap();
        assert_eq!(two.text, "[-x]\n[--]\n[1]\n[-y]\n[2]\n");
        // Only the second: the separator is before it.
        let second = run(&r, binding.clone(), json!({"r": "2"})).await.unwrap();
        assert_eq!(second.text, "[-x]\n[-y]\n[--]\n[2]\n");
        // A value that looks like an option follows the separator.
        let option = run(&r, binding, json!({"q": "--version"})).await.unwrap();
        assert_eq!(option.text, "[-x]\n[--]\n[--version]\n[-y]\n");
    }

    #[tokio::test]
    async fn values_are_strings_numbers_or_booleans_and_nothing_else() {
        let dir = TempDir::new().unwrap();
        let marker = dir.path().join("ran");
        let program = script(
            dir.path(),
            "args.sh",
            &format!("touch {}\n{PRINT_ARGS}", marker.display()),
        );
        let binding = json!({"program": program, "args": ["{n}", "{flag}", "{q}"]});
        let r = runner(store());
        let ok = run(&r, binding.clone(), json!({"n": 5, "flag": true, "q": "s"}))
            .await
            .unwrap();
        assert_eq!(ok.text, "[--]\n[5]\n[true]\n[s]\n");
        std::fs::remove_file(&marker).unwrap();
        for bad in [
            json!({"q": {"a": 1}}),
            json!({"q": [1]}),
            json!({"q": null}),
            json!({"q": "a\u{0}b"}),
        ] {
            let err = run(&r, binding.clone(), bad.clone()).await.unwrap_err();
            assert!(err.0.contains("`q`"), "{bad}: {}", err.0);
            assert!(!marker.exists(), "the program ran for {bad}");
        }
    }

    #[tokio::test]
    async fn the_environment_is_built_from_the_allow_list_and_the_binding_only() {
        let dir = TempDir::new().unwrap();
        let program = script(dir.path(), "env.sh", "env | cut -d= -f1 | sort");
        std::env::set_var("NS_CLI_TEST_INHERITED", "leaked");
        let secrets = store();
        secrets.set("tok", "SECRETVALUE-123").unwrap();
        let r = CliRunner::new(secrets);
        let out = run(
            &r,
            json!({"program": program, "env": [{"name": "API_TOKEN", "secret": "keychain:tok"}]}),
            json!({}),
        )
        .await
        .unwrap();
        let names: Vec<&str> = out.text.lines().collect();
        assert!(!names.contains(&"NS_CLI_TEST_INHERITED"), "{names:?}");
        assert!(names.contains(&"API_TOKEN"));
        // What a shell adds itself, and the base list when set.
        for name in &names {
            assert!(
                BASE_ENV.contains(name)
                    || [
                        "API_TOKEN",
                        "PWD",
                        "SHLVL",
                        "_",
                        "OLDPWD",
                        "__CF_USER_TEXT_ENCODING"
                    ]
                    .contains(name),
                "unexpected variable {name}"
            );
        }
    }

    #[tokio::test]
    async fn a_credential_reaches_only_the_programs_environment_and_is_redacted_everywhere() {
        let dir = TempDir::new().unwrap();
        // Prints the credential on both streams and its own argv.
        let program = script(
            dir.path(),
            "echo.sh",
            "echo \"out:$API_TOKEN:$HOST_TOKEN\"\necho \"err:$API_TOKEN\" >&2\nfor a in \"$@\"; do echo \"arg:$a\"; done",
        );
        let secrets = store();
        secrets.set("tok", "SECRETVALUE-123").unwrap();
        let r = runner(secrets);
        let out = run(
            &r,
            json!({"program": program, "args": ["{q}"], "env": [
                {"name": "API_TOKEN", "secret": "keychain:tok"},
                {"name": "HOST_TOKEN", "secret": "env:HOST_TOKEN"},
            ]}),
            json!({"q": "hello"}),
        )
        .await
        .unwrap();
        assert!(
            out.text.contains("out:[redacted]:[redacted]"),
            "{}",
            out.text
        );
        assert!(out.text.contains("err:[redacted]"), "{}", out.text);
        for value in ["SECRETVALUE-123", "ENVVALUE-456"] {
            assert!(!out.text.contains(value), "{}", out.text);
        }
        // Never an argument.
        assert!(out.text.contains("arg:--") && out.text.contains("arg:hello"));
    }

    #[tokio::test]
    async fn a_missing_or_too_short_credential_stops_the_call_before_the_program_runs() {
        let dir = TempDir::new().unwrap();
        let marker = dir.path().join("ran");
        let program = script(dir.path(), "p.sh", &format!("touch {}", marker.display()));
        let secrets = store();
        secrets.set("short", "abc").unwrap();
        let r = runner(secrets);
        for (reference, expect) in [
            ("keychain:absent", "not set on this device"),
            ("env:ABSENT_VAR", "not set in the environment"),
            ("keychain:short", "too short to be redacted"),
        ] {
            let err = run(
                &r,
                json!({"program": program, "env": [{"name": "API_TOKEN", "secret": reference}]}),
                json!({}),
            )
            .await
            .unwrap_err();
            assert!(err.0.contains(expect), "{reference}: {}", err.0);
            assert!(!marker.exists(), "{reference}: the program ran");
        }
    }

    #[tokio::test]
    async fn a_failing_program_reports_its_status_and_redacted_bounded_output() {
        let dir = TempDir::new().unwrap();
        let program = script(
            dir.path(),
            "fail.sh",
            "echo \"bad $API_TOKEN\" >&2\nhead -c 5000 /dev/zero | tr '\\0' x >&2\nexit 3",
        );
        let secrets = store();
        secrets.set("tok", "SECRETVALUE-123").unwrap();
        let err = run(
            &runner(secrets),
            json!({"program": program, "env": [{"name": "API_TOKEN", "secret": "keychain:tok"}]}),
            json!({}),
        )
        .await
        .unwrap_err();
        assert!(err.0.contains("status 3"), "{}", err.0);
        assert!(err.0.contains("[redacted]"));
        assert!(!err.0.contains("SECRETVALUE-123"));
        assert!(err.0.len() < ERROR_OUTPUT_BYTES + 200, "{}", err.0.len());
    }

    #[tokio::test]
    async fn the_timeout_stops_the_program_and_everything_it_started() {
        let dir = TempDir::new().unwrap();
        let pidfile = dir.path().join("pid");
        let program = script(
            dir.path(),
            "slow.sh",
            &format!("sleep 30 &\necho $! > {}\nwait", pidfile.display()),
        );
        let started = Instant::now();
        let err = run(
            &runner(store()),
            json!({"program": program, "timeout_seconds": 1}),
            json!({}),
        )
        .await
        .unwrap_err();
        assert!(
            err.0.contains("did not finish within 1 seconds"),
            "{}",
            err.0
        );
        assert!(started.elapsed() < Duration::from_secs(10));
        let pid: i32 = std::fs::read_to_string(&pidfile)
            .unwrap()
            .trim()
            .parse()
            .unwrap();
        // The grandchild is gone too (allow the kernel a moment).
        let mut alive = true;
        for _ in 0..50 {
            alive = unsafe { libc::kill(pid, 0) } == 0;
            if !alive {
                break;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        assert!(!alive, "the program's own child outlived the timeout");
    }

    #[tokio::test]
    async fn output_is_read_as_a_stream_capped_and_the_program_is_stopped_at_the_cap() {
        let dir = TempDir::new().unwrap();
        let program = script(
            dir.path(),
            "endless.sh",
            "while :; do echo xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx; done",
        );
        let started = Instant::now();
        let out = run(
            &runner(store()),
            json!({"program": program, "max_output_bytes": 4096}),
            json!({}),
        )
        .await
        .expect("the program was stopped at the cap and its output returned");
        assert!(
            started.elapsed() < Duration::from_secs(10),
            "it was not stopped at the cap"
        );
        assert!(out.truncated);
        assert!(out.text.len() <= 4096, "{}", out.text.len());
        assert!(out.text.len() > 4000);
    }

    #[tokio::test]
    async fn a_credential_cut_by_the_cap_leaves_no_part_of_itself_in_the_output() {
        let dir = TempDir::new().unwrap();
        // The credential arrives in two writes, the cap falling inside the first
        // half: the reader must keep reading until the whole value is in hand
        // to redact it, then cut.
        let program = script(
            dir.path(),
            "edge.sh",
            "printf 'AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA%s' \"$(printf %s \"$API_TOKEN\" | cut -c1-7)\"\nsleep 1\nprintf '%s' \"$(printf %s \"$API_TOKEN\" | cut -c8-)\"\nsleep 1\nprintf 'BBBBBBBBBBBBBBBBBBBBBBBB'",
        );
        let secrets = store();
        secrets.set("tok", "SECRETVALUE-123").unwrap();
        let out = run(
            &runner(secrets),
            json!({"program": program, "max_output_bytes": 64, "env": [{"name": "API_TOKEN", "secret": "keychain:tok"}]}),
            json!({}),
        )
        .await
        .unwrap();
        assert!(out.truncated);
        assert!(out.text.len() <= 64);
        for part in ["SECR", "ECRET", "VALUE", "-123"] {
            assert!(!out.text.contains(part), "{}", out.text);
        }
    }

    #[tokio::test]
    async fn a_malformed_or_incomplete_binding_fails_closed_before_anything_runs() {
        let dir = TempDir::new().unwrap();
        let marker = dir.path().join("ran");
        let program = script(dir.path(), "p.sh", &format!("touch {}", marker.display()));
        let r = runner(store());
        for (why, binding) in [
            ("no program", json!({"args": ["x"]})),
            (
                "a partial placeholder",
                json!({"program": program, "args": ["a{q}"]}),
            ),
            (
                "an undeclared placeholder",
                json!({"program": program, "args": ["{nope}"]}),
            ),
            ("a shell line", json!({"program": "git status"})),
            (
                "a denied variable",
                json!({"program": program, "env": [{"name": "PATH", "secret": "env:HOST_TOKEN"}]}),
            ),
            (
                "a literal secret",
                json!({"program": program, "env": [{"name": "TOKEN", "secret": "ghp_literal"}]}),
            ),
            ("an unknown key", json!({"program": program, "unknown": 1})),
            (
                "a timeout over the maximum",
                json!({"program": program, "timeout_seconds": 100000}),
            ),
        ] {
            let err = run(&r, binding, json!({})).await.unwrap_err();
            assert!(err.0.contains("binding"), "{why}: {}", err.0);
            assert!(!marker.exists(), "{why}: the program ran");
        }
        assert!(run(&r, json!(null), json!({})).await.is_err());
    }

    #[tokio::test]
    async fn a_program_that_cannot_start_is_an_error() {
        let err = run(
            &runner(store()),
            json!({"program": "/nonexistent/program"}),
            json!({}),
        )
        .await
        .unwrap_err();
        assert!(err.0.contains("could not be started"), "{}", err.0);
    }

    #[test]
    fn the_confirmation_names_the_program_the_arguments_and_the_credential_references() {
        let secrets = store();
        secrets.set("tok", "SECRETVALUE-123").unwrap();
        let r = runner(secrets);
        let t = tool(json!({
            "program": "/usr/bin/git",
            "args": ["log", "{q}"],
            "env": [{"name": "API_TOKEN", "secret": "keychain:tok"}],
        }));
        let shown = r.destination(&t, &json!({"q": "main"}));
        assert!(shown.contains("/usr/bin/git"), "{shown}");
        assert!(shown.contains(r#"["log","--","main"]"#), "{shown}");
        assert!(shown.contains("API_TOKEN from keychain:tok"), "{shown}");
        assert!(!shown.contains("SECRETVALUE-123"));
        // A long argument list is cut.
        let long = r.destination(&t, &json!({"q": "x".repeat(10_000)}));
        assert!(long.len() <= MAX_DESTINATION_BYTES);
        // An unusable binding is said so, not described as a command.
        let bad = r.destination(&tool(json!({"args": ["x"]})), &json!({}));
        assert!(bad.contains("unusable binding"), "{bad}");
    }

    /// The raw output ends part way through a credential after an earlier
    /// occurrence was redacted (and so shortened the text): no prefix of the
    /// last, cut-off occurrence may be returned.
    #[tokio::test]
    async fn a_credential_cut_at_the_end_of_what_was_read_leaves_no_prefix() {
        let dir = TempDir::new().unwrap();
        let program = script(
            dir.path(),
            "tail.sh",
            "printf '%s%s%s%s' FFFFFFFFFFFFFFFFFFFFFFFFFFFFFF \"$API_TOKEN\" FFFFFFFFFFFFFFFFFFFF \"$(printf %s \"$API_TOKEN\" | cut -c1-14)\"",
        );
        let secrets = store();
        secrets
            .set("tok", "SECRETSECRETSECRETSECRETSECRETSECRETABCD")
            .unwrap();
        let out = run(
            &runner(secrets),
            json!({"program": program, "max_output_bytes": 64, "env": [{"name": "API_TOKEN", "secret": "keychain:tok"}]}),
            json!({}),
        )
        .await
        .unwrap();
        assert!(out.truncated);
        assert!(out.text.len() <= 64);
        assert!(out.text.contains("[redacted]"), "{}", out.text);
        for part in ["SECR", "ECRET", "ABCD", "RETS"] {
            assert!(!out.text.contains(part), "{}", out.text);
        }
    }

    /// Two credentials, the cut-off one being the other of them, on either
    /// stream.
    #[tokio::test]
    async fn several_credentials_and_either_stream_leave_no_prefix_when_cut() {
        for stream in ["", ">&2"] {
            let dir = TempDir::new().unwrap();
            let program = script(
                dir.path(),
                "tail2.sh",
                &format!(
                    "printf '%s%s%s%s%s%s' FFFFFFFFFF \"$B\" FFFFFFFFFF \"$A\" FFFFFFFFFF BRAVOSEC {stream}"
                ),
            );
            let secrets = store();
            secrets
                .set("a", "ALPHAKEYALPHAKEYALPHAKEYALPHAKEYALPHAKEY")
                .unwrap();
            secrets.set("b", "BRAVOSECRETBRAVOSECRET").unwrap();
            let out = run(
                &runner(secrets),
                json!({"program": program, "max_output_bytes": 100, "env": [
                    {"name": "A", "secret": "keychain:a"},
                    {"name": "B", "secret": "keychain:b"},
                ]}),
                json!({}),
            )
            .await
            .unwrap();
            assert!(out.truncated, "{stream:?}");
            for part in ["ALPHA", "BRAVO", "KEYAL", "SECRET"] {
                assert!(!out.text.contains(part), "{stream:?}: {}", out.text);
            }
        }
    }

    #[tokio::test]
    async fn output_on_stderr_alone_counts_toward_the_cap() {
        let dir = TempDir::new().unwrap();
        let program = script(
            dir.path(),
            "err.sh",
            "while :; do echo xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx >&2; done",
        );
        let started = Instant::now();
        let out = run(
            &runner(store()),
            json!({"program": program, "max_output_bytes": 4096}),
            json!({}),
        )
        .await
        .expect("stopped at the cap and returned");
        assert!(started.elapsed() < Duration::from_secs(10));
        assert!(out.truncated);
        assert!(out.text.len() <= 4096);
        assert!(out.text.contains("[stderr]"));
    }

    /// A cancelled call (its future dropped) stops the program and what it
    /// started, and removes its working directory.
    #[tokio::test]
    async fn a_cancelled_call_stops_the_program_and_its_children() {
        let dir = TempDir::new().unwrap();
        let (p1, p2, cwd) = (
            dir.path().join("p1"),
            dir.path().join("p2"),
            dir.path().join("cwd"),
        );
        let program = script(
            dir.path(),
            "bg.sh",
            &format!(
                "pwd > {}\necho $$ > {}\nsleep 30 &\necho $! > {}\nwait",
                cwd.display(),
                p1.display(),
                p2.display()
            ),
        );
        let r = runner(store());
        let t = tool(json!({"program": program}));
        let no_args = json!({});
        let call = r.run(&t, &no_args);
        // Drop the future part way through.
        let _ = tokio::time::timeout(Duration::from_millis(1500), call).await;
        let pids: Vec<i32> = [&p1, &p2]
            .iter()
            .map(|p| std::fs::read_to_string(p).unwrap().trim().parse().unwrap())
            .collect();
        for pid in pids {
            let mut alive = true;
            for _ in 0..50 {
                alive = unsafe { libc::kill(pid, 0) } == 0;
                if !alive {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
            assert!(!alive, "process {pid} outlived the cancelled call");
        }
        let used = PathBuf::from(std::fs::read_to_string(&cwd).unwrap().trim());
        assert!(!used.exists(), "the working directory was left behind");
    }

    /// The program starts in a fresh private directory, which is removed.
    #[tokio::test]
    async fn the_program_runs_in_a_fresh_private_directory_that_is_removed() {
        use std::os::unix::fs::PermissionsExt;
        let dir = TempDir::new().unwrap();
        let seen = dir.path().join("seen");
        let program = script(
            dir.path(),
            "cwd.sh",
            &format!(
                "pwd > {0}\nls -A | wc -l >> {0}\necho x > scratch-file",
                seen.display()
            ),
        );
        let out = run(&runner(store()), json!({"program": program}), json!({}))
            .await
            .unwrap();
        assert_eq!(out.text, "");
        let text = std::fs::read_to_string(&seen).unwrap();
        let mut lines = text.lines();
        let used = PathBuf::from(lines.next().unwrap());
        assert_eq!(lines.next().unwrap().trim(), "0", "it started empty");
        let name = used.file_name().unwrap().to_string_lossy().into_owned();
        assert!(name.starts_with("nodespace-run-"), "{used:?}");
        assert!(!used.exists(), "the directory was left behind");
        // Made private: checked on a directory made the same way.
        let made = ScratchDir::create().unwrap();
        let mode = std::fs::metadata(&made.0).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o700);
        assert_ne!(made.0, std::env::temp_dir());
    }

    /// Credentials are listed first and bounded, so no argument can hide them.
    #[test]
    fn a_long_argument_cannot_push_the_credentials_out_of_the_confirmation() {
        let r = runner(store());
        let t = tool(json!({
            "program": "/usr/bin/git",
            "args": ["log", "{q}"],
            "env": [{"name": "API_TOKEN", "secret": "keychain:tok"}],
        }));
        let shown = r.destination(&t, &json!({"q": "x".repeat(10_000)}));
        assert!(shown.contains("API_TOKEN from keychain:tok"), "{shown}");
        assert!(shown.len() <= MAX_DESTINATION_BYTES, "{}", shown.len());
        // The cut argument list says so.
        assert!(
            shown.ends_with('\u{2026}'),
            "{}",
            &shown[shown.len() - 20..]
        );
        // Many references: eight shown, the rest counted, none hidden silently.
        let env: Vec<Value> = (0..11)
            .map(|i| json!({"name": format!("ENV_{i}"), "secret": "env:SOME_VAR"}))
            .collect();
        let many = tool(json!({"program": "/usr/bin/git", "env": env}));
        let shown = r.destination(&many, &json!({}));
        assert!(shown.contains("ENV_0 from env:SOME_VAR"));
        assert!(shown.contains("ENV_7 from env:SOME_VAR"));
        assert!(!shown.contains("ENV_8"));
        assert!(shown.contains("and 3 more"), "{shown}");
    }

    #[test]
    fn a_bare_program_name_is_shown_with_where_it_resolves() {
        let r = runner(store());
        let found = r.destination(&tool(json!({"program": "sh"})), &json!({}));
        assert!(found.contains("found on the daemon's PATH as"), "{found}");
        let missing = r.destination(
            &tool(json!({"program": "definitely-not-a-program-xyz"})),
            &json!({}),
        );
        assert!(
            missing.contains("not found on the daemon's PATH"),
            "{missing}"
        );
        // An absolute path is shown as given.
        let absolute = r.destination(&tool(json!({"program": "/bin/sh"})), &json!({}));
        assert!(
            absolute.contains("`/bin/sh`") && !absolute.contains("found on"),
            "{absolute}"
        );
    }

    /// The same, through `run`: the read ends in the middle of a character of the
    /// credential.
    #[tokio::test]
    async fn a_credential_cut_in_the_middle_of_a_character_leaves_no_prefix() {
        for stream in ["", ">&2"] {
            let dir = TempDir::new().unwrap();
            let program = script(
                dir.path(),
                "mid.sh",
                &format!("printf 'xxxx%s' \"$(printf %s \"$API_TOKEN\" | head -c 5)\" {stream}"),
            );
            let secrets = store();
            secrets.set("tok", "ABCD\u{e9}\u{20ac}FGHIJ").unwrap();
            let out = run(
                &runner(secrets),
                json!({"program": program, "max_output_bytes": 8, "env": [{"name": "API_TOKEN", "secret": "keychain:tok"}]}),
                json!({}),
            )
            .await
            .unwrap();
            assert!(out.truncated, "{stream:?}");
            assert!(!out.text.contains("ABCD"), "{stream:?}: {:?}", out.text);
        }
    }

    /// The cut one is on stderr, and a complete occurrence on stdout shrank the
    /// text so that the stderr tail falls inside the cap.
    #[tokio::test]
    async fn a_credential_cut_mid_character_on_stderr_leaves_no_prefix() {
        let dir = TempDir::new().unwrap();
        let program = script(
            dir.path(),
            "mid_err.sh",
            "printf %s \"$API_TOKEN\"\nprintf %s \"$API_TOKEN\" | head -c 5 >&2",
        );
        let secrets = store();
        secrets
            .set(
                "tok",
                "ABCD\u{e9}\u{20ac}FGHIJFGHIJFGHIJFGHIJFGHIJFGHIJFGHIJ",
            )
            .unwrap();
        let out = run(
            &runner(secrets),
            json!({"program": program, "max_output_bytes": 49, "env": [{"name": "API_TOKEN", "secret": "keychain:tok"}]}),
            json!({}),
        )
        .await
        .unwrap();
        assert!(out.truncated);
        assert!(out.text.starts_with("[redacted]"), "{:?}", out.text);
        assert!(!out.text.contains("ABCD"), "{:?}", out.text);
    }

    #[test]
    fn a_long_resolved_path_does_not_squeeze_out_the_credentials_or_arguments() {
        let root = TempDir::new().unwrap();
        let mut deep = root.path().to_path_buf();
        for _ in 0..4 {
            deep.push("d".repeat(200));
        }
        std::fs::create_dir_all(&deep).unwrap();
        script(&deep, "longprog", "true");
        let path_var = deep.to_string_lossy().into_owned();
        let r =
            CliRunner::with_env_lookup(store(), move |n| (n == "PATH").then(|| path_var.clone()));
        let t = tool(json!({
            "program": "longprog",
            "args": ["{q}"],
            "env": [{"name": "API_TOKEN", "secret": "keychain:tok"}],
        }));
        let shown = r.destination(&t, &json!({"q": "main"}));
        assert!(shown.contains("API_TOKEN from keychain:tok"), "{shown}");
        assert!(shown.contains(r#"["--","main"]"#), "{shown}");
        assert!(
            shown.contains('\u{2026}'),
            "the long path was not cut: {shown}"
        );
        assert!(shown.len() < 1500, "{}", shown.len());
    }

    #[test]
    fn an_unset_path_is_said_to_be_unset_not_reported_as_not_found() {
        let r = CliRunner::with_env_lookup(store(), |_| None);
        let shown = r.destination(&tool(json!({"program": "git"})), &json!({}));
        assert!(shown.contains("PATH is unset"), "{shown}");
        assert!(!shown.contains("not found"), "{shown}");
    }
}
