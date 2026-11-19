//! `nodespace secret ...` through the compiled binary against an in-process
//! `SecretService` over an in-memory store: the value comes from standard
//! input, and nothing the command prints or returns carries it.

use std::process::{Output, Stdio};
use std::sync::Arc;
use std::time::Duration;

use nodespace_core::secrets::{InMemorySecretBackend, SecretStore};
use nodespace_daemon::{SecretServiceImpl, SecretServiceServer};
use tempfile::TempDir;
use tokio::io::AsyncWriteExt;
use tokio::net::UnixListener;
use tokio::process::Command;
use tokio::sync::oneshot;
use tokio_stream::wrappers::UnixListenerStream;
use tonic::transport::Server;

const TOKEN: &str = "ghp_SUPERSECRETVALUE123";

struct Daemon {
    home: TempDir,
    store: Arc<SecretStore>,
    shutdown: Option<oneshot::Sender<()>>,
}

impl Drop for Daemon {
    fn drop(&mut self) {
        if let Some(tx) = self.shutdown.take() {
            let _ = tx.send(());
        }
    }
}

async fn spawn() -> Daemon {
    let home = TempDir::new().unwrap();
    let store = Arc::new(SecretStore::new(Arc::new(InMemorySecretBackend::default())));
    let sock = home.path().join("d.sock");
    let incoming = UnixListenerStream::new(UnixListener::bind(&sock).unwrap());
    let (tx, rx) = oneshot::channel::<()>();
    let service = SecretServiceImpl::new(store.clone());
    tokio::spawn(async move {
        Server::builder()
            .add_service(SecretServiceServer::new(service))
            .serve_with_incoming_shutdown(incoming, async move {
                let _ = rx.await;
            })
            .await
            .expect("server crashed");
    });
    for _ in 0..50 {
        if nodespace_cli::connect_secrets(&sock).await.is_ok() {
            return Daemon {
                home,
                store,
                shutdown: Some(tx),
            };
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    panic!("daemon did not start");
}

async fn nodespace(daemon: &Daemon, args: &[&str], stdin: &str) -> Output {
    let mut child = Command::new(env!("CARGO_BIN_EXE_nodespace"))
        .arg("--socket")
        .arg(daemon.home.path().join("d.sock"))
        .args(args)
        .env("NODESPACE_HOME", daemon.home.path())
        .env("HOME", daemon.home.path())
        .env_remove("NODESPACE_DATABASE")
        .env_remove("NODESPACED_SOCKET")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .expect("nodespace ran");
    let mut pipe = child.stdin.take().unwrap();
    pipe.write_all(stdin.as_bytes()).await.unwrap();
    drop(pipe);
    tokio::time::timeout(Duration::from_secs(30), child.wait_with_output())
        .await
        .expect("nodespace finished")
        .expect("nodespace output")
}

fn all_output(o: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&o.stdout),
        String::from_utf8_lossy(&o.stderr)
    )
}

#[tokio::test]
async fn set_reads_the_value_from_stdin_and_stores_it_without_printing_it() {
    let d = spawn().await;
    let out = nodespace(&d, &["secret", "set", "gh"], &format!("{TOKEN}\n")).await;
    assert!(out.status.success(), "{}", all_output(&out));
    assert!(!all_output(&out).contains(TOKEN));
    assert_eq!(d.store.get("gh").unwrap().unwrap().expose(), TOKEN);
}

#[tokio::test]
async fn list_names_prints_names_only_in_both_formats() {
    let d = spawn().await;
    d.store.set("b-name", TOKEN).unwrap();
    d.store.set("a-name", TOKEN).unwrap();
    let out = nodespace(&d, &["secret", "list-names"], "").await;
    assert!(out.status.success());
    assert_eq!(String::from_utf8_lossy(&out.stdout), "a-name\nb-name\n");
    let out = nodespace(&d, &["--json", "secret", "list-names"], "").await;
    assert!(!all_output(&out).contains(TOKEN));
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["names"], serde_json::json!(["a-name", "b-name"]));
}

#[tokio::test]
async fn delete_removes_the_value_and_the_name() {
    let d = spawn().await;
    d.store.set("gh", TOKEN).unwrap();
    let out = nodespace(&d, &["secret", "delete", "gh"], "").await;
    assert!(out.status.success());
    assert!(d.store.get("gh").unwrap().is_none());
    assert!(d.store.names().unwrap().is_empty());
}

#[tokio::test]
async fn a_bad_name_or_empty_value_fails_and_nothing_is_stored_or_echoed() {
    let d = spawn().await;
    let out = nodespace(&d, &["secret", "set", "bad name"], &format!("{TOKEN}\n")).await;
    assert!(!out.status.success());
    assert!(!all_output(&out).contains(TOKEN));
    let out = nodespace(&d, &["secret", "set", "gh"], "\n").await;
    assert!(!out.status.success());
    assert!(d.store.names().unwrap().is_empty());
}

#[tokio::test]
async fn a_value_on_the_command_line_is_refused_before_anything_is_stored() {
    let d = spawn().await;
    let out = nodespace(&d, &["secret", "set", "gh", TOKEN], "").await;
    assert!(!out.status.success());
    assert!(d.store.names().unwrap().is_empty());
}
