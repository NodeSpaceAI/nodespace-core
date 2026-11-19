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
//!   A credential too short to redact safely is refused.
//! - **Limits.** A timeout kills the program and everything it started (its
//!   process group); output is read as a stream and capped, and the program is
//!   killed when the cap is reached, rather than buffered and cut afterwards.
//!
//! Limits of the redaction: the exact value is replaced. A program that prints
//! a transformed form of a credential (base64, URL-encoded, split across
//! lines) is not caught. The child's environment copy of the value is held by
//! the process machinery and is not wiped.

use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use nodespace_core::secrets::{resolve_credential_with, SecretStore};
use nodespace_types::{
    CredentialRef, ToolCliFields, DEFAULT_RESPONSE_BYTES, DEFAULT_TIMEOUT_SECONDS,
};
use serde_json::Value;
use tokio::io::AsyncReadExt;
use tokio::process::Command;
use zeroize::Zeroizing;

use super::remote_tools::{
    RemoteCallError, RemoteOutput, RemoteTool, RemoteToolKind, RemoteToolRunner,
};

/// The daemon's variables a program is given, when they are set: enough to find
/// other programs and the user's home, and nothing that carries a credential.
pub const BASE_ENV: &[&str] = &["PATH", "HOME", "LANG", "USER", "TMPDIR"];

/// A credential shorter than this is refused: redacting it would mangle ordinary
/// output, and it is not a credential.
const MIN_REDACTABLE_BYTES: usize = 4;
/// Read size for the program's output streams.
const READ_CHUNK: usize = 8192;
/// How much of a failing program's output an error message carries.
const ERROR_OUTPUT_BYTES: usize = 1024;
/// What a redacted value is replaced with.
const REDACTED: &str = "[redacted]";
/// Longest part of the argument list a confirmation question shows.
const MAX_DESTINATION_BYTES: usize = 2048;

type EnvLookup = Arc<dyn Fn(&str) -> Option<String> + Send + Sync>;

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

/// Replaces every credential value in `text`, longest first.
fn redact(text: &str, needles: &[Zeroizing<String>]) -> String {
    let mut ordered: Vec<&str> = needles.iter().map(|n| n.as_str()).collect();
    ordered.sort_by_key(|n| std::cmp::Reverse(n.len()));
    let mut out = text.to_string();
    for needle in ordered {
        out = out.replace(needle, REDACTED);
    }
    out
}

/// `text` cut to at most `max` bytes at a character boundary.
fn cut(text: &mut String, max: usize) {
    if text.len() > max {
        let mut end = max;
        while !text.is_char_boundary(end) {
            end -= 1;
        }
        text.truncate(end);
    }
}

/// How long to wait for a killed program to be reaped.
const REAP_BOUND: Duration = Duration::from_secs(2);

/// Kills the program and everything it started. The child leads its own process
/// group, so the group is killed; the child itself is killed as well, so a
/// failure to signal the group cannot leave it running.
fn stop(child: &mut tokio::process::Child) {
    #[cfg(unix)]
    if let Some(pid) = child.id() {
        unsafe {
            libc::killpg(pid as i32, libc::SIGKILL);
        }
    }
    let _ = child.start_kill();
}

/// Waits a bounded time for a program that has been told to stop.
async fn reap(child: &mut tokio::process::Child) {
    let _ = tokio::time::timeout(REAP_BOUND, child.wait()).await;
}

impl CliRunner {
    /// Resolves the binding's declared credentials: the (name, value) pairs for
    /// the child's environment. Runs the platform store off the async threads.
    async fn credentials(
        &self,
        fields: &ToolCliFields,
    ) -> Result<Vec<(String, Zeroizing<String>)>, RemoteCallError> {
        let mut out = Vec::new();
        for entry in fields.env.iter().flatten() {
            let reference = CredentialRef::parse(&entry.secret).map_err(|e| {
                RemoteCallError(format!("the tool's binding is not valid: {}", e.0))
            })?;
            let secrets = Arc::clone(&self.secrets);
            let lookup = Arc::clone(&self.daemon_env);
            let value = tokio::task::spawn_blocking(move || {
                resolve_credential_with(&secrets, &reference, |n| lookup(n))
            })
            .await
            .map_err(|_| RemoteCallError("the credential store task failed".to_string()))?
            .map_err(|e| RemoteCallError(e.to_string()))?;
            // The one place a value is read: it goes into the child's environment.
            let text = Zeroizing::new(value.expose().to_string());
            if text.len() < MIN_REDACTABLE_BYTES {
                return fail(format!(
                    "the credential for `{}` is too short to be redacted safely",
                    entry.name
                ));
            }
            out.push((entry.name.clone(), text));
        }
        Ok(out)
    }
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
        let mut shown = match invocation(&fields, args) {
            Ok(call) => {
                let argv = serde_json::to_string(&call.argv).unwrap_or_default();
                format!("run `{}` with arguments {argv}", call.program)
            }
            Err(e) => format!("(unusable call: {})", e.0),
        };
        let credentials: Vec<String> = fields
            .env
            .iter()
            .flatten()
            .map(|e| format!("{} from {}", e.name, e.secret))
            .collect();
        if !credentials.is_empty() {
            shown.push_str(&format!("; environment: {}", credentials.join(", ")));
        }
        cut(&mut shown, MAX_DESTINATION_BYTES);
        shown
    }

    async fn run(&self, tool: &RemoteTool, args: &Value) -> Result<RemoteOutput, RemoteCallError> {
        let fields = read_binding(tool)?;
        let call = invocation(&fields, args)?;
        let credentials = self.credentials(&fields).await?;
        let needles: Vec<Zeroizing<String>> = credentials.iter().map(|(_, v)| v.clone()).collect();
        let longest = needles.iter().map(|n| n.len()).max().unwrap_or(0);
        let cap = fields.max_output_bytes.unwrap_or(DEFAULT_RESPONSE_BYTES) as usize;
        let limit = Duration::from_secs(fields.timeout_seconds.unwrap_or(DEFAULT_TIMEOUT_SECONDS));

        let mut child = {
            let mut command = Command::new(&call.program);
            command
                .args(&call.argv)
                .env_clear()
                .stdin(Stdio::null())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .kill_on_drop(true)
                .current_dir(std::env::temp_dir());
            for name in BASE_ENV {
                if let Some(value) = (self.daemon_env)(name) {
                    command.env(name, value);
                }
            }
            for (name, value) in &credentials {
                command.env(name, value.as_str());
            }
            #[cfg(unix)]
            command.process_group(0);
            command.spawn().map_err(|e| {
                RemoteCallError(format!("the program could not be started: {}", e.kind()))
            })?
        };
        drop(credentials);

        let mut stdout = child.stdout.take().expect("stdout is piped");
        let mut stderr = child.stderr.take().expect("stderr is piped");
        // Read a little past the cap so a credential cut by the cap is still
        // whole when it is redacted; the text is cut to the cap afterwards.
        let read_limit = cap + longest;

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
                    stop(&mut child);
                    break;
                }
            }
            let status = if stopped_at_cap {
                // Killed: a bounded wait, and its status says nothing.
                reap(&mut child).await;
                child.try_wait()
            } else {
                child.wait().await.map(Some)
            };
            (out, err, stopped_at_cap, status)
        };

        let (out, err, stopped_at_cap, status) = match tokio::time::timeout(limit, work).await {
            Ok(done) => done,
            Err(_) => {
                stop(&mut child);
                reap(&mut child).await;
                return fail(format!(
                    "the program did not finish within {} seconds and was stopped",
                    limit.as_secs()
                ));
            }
        };

        let mut text = String::from_utf8_lossy(&out).into_owned();
        if !err.is_empty() {
            text.push_str("\n[stderr]\n");
            text.push_str(&String::from_utf8_lossy(&err));
        }
        let mut text = redact(&text, &needles);
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
}
