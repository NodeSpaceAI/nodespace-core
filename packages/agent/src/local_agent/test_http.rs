//! A local HTTP server and a scripted resolver for the tests of the runners that
//! speak HTTP. Nothing here touches the real network.

use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

use super::net_policy::HostResolver;

// -- a local HTTP server ----------------------------------------------

#[derive(Debug, Clone)]
pub(crate) struct Request {
    pub(crate) head: String,
    pub(crate) body: Vec<u8>,
}

pub(crate) enum Reply {
    Whole(Vec<u8>),
    /// The head, then `chunk` every `every`, until the client goes away.
    Stream {
        head: Vec<u8>,
        chunk: Vec<u8>,
        every: Duration,
    },
}

pub(crate) fn http(status: &str, headers: &[(&str, &str)], body: &[u8]) -> Vec<u8> {
    let mut out = format!(
        "HTTP/1.1 {status}\r\nConnection: close\r\nContent-Length: {}\r\n",
        body.len()
    );
    for (name, value) in headers {
        out.push_str(&format!("{name}: {value}\r\n"));
    }
    out.push_str("\r\n");
    let mut bytes = out.into_bytes();
    bytes.extend_from_slice(body);
    bytes
}

pub(crate) fn ok_json(body: &str) -> Reply {
    Reply::Whole(http(
        "200 OK",
        &[("Content-Type", "application/json")],
        body.as_bytes(),
    ))
}

pub(crate) struct Server {
    pub(crate) port: u16,
    pub(crate) seen: Arc<Mutex<Vec<Request>>>,
    /// Set when a streaming client went away.
    pub(crate) closed: Arc<std::sync::atomic::AtomicBool>,
}

impl Server {
    pub(crate) fn hits(&self) -> usize {
        self.seen.lock().unwrap().len()
    }
    pub(crate) fn requests(&self) -> Vec<Request> {
        self.seen.lock().unwrap().clone()
    }
}

pub(crate) async fn serve(handler: impl Fn(&Request) -> Reply + Send + Sync + 'static) -> Server {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let seen: Arc<Mutex<Vec<Request>>> = Arc::default();
    let closed = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let handler = Arc::new(handler);
    let (seen_in, closed_in) = (Arc::clone(&seen), Arc::clone(&closed));
    tokio::spawn(async move {
        loop {
            let Ok((mut stream, _)) = listener.accept().await else {
                return;
            };
            let (handler, seen, closed) = (
                Arc::clone(&handler),
                Arc::clone(&seen_in),
                Arc::clone(&closed_in),
            );
            tokio::spawn(async move {
                let mut buffer = Vec::new();
                let mut chunk = [0u8; 4096];
                let head_end = loop {
                    let n = stream.read(&mut chunk).await.unwrap_or(0);
                    if n == 0 {
                        return;
                    }
                    buffer.extend_from_slice(&chunk[..n]);
                    if let Some(at) = buffer.windows(4).position(|w| w == b"\r\n\r\n") {
                        break at + 4;
                    }
                };
                let head = String::from_utf8_lossy(&buffer[..head_end]).into_owned();
                let length = head
                    .lines()
                    .find_map(|l| {
                        l.to_ascii_lowercase()
                            .strip_prefix("content-length:")
                            .map(|v| v.trim().parse::<usize>().unwrap_or(0))
                    })
                    .unwrap_or(0);
                while buffer.len() < head_end + length {
                    let n = stream.read(&mut chunk).await.unwrap_or(0);
                    if n == 0 {
                        break;
                    }
                    buffer.extend_from_slice(&chunk[..n]);
                }
                let request = Request {
                    head,
                    body: buffer[head_end..].to_vec(),
                };
                let reply = handler(&request);
                seen.lock().unwrap().push(request);
                match reply {
                    Reply::Whole(bytes) => {
                        let _ = stream.write_all(&bytes).await;
                        let _ = stream.shutdown().await;
                    }
                    Reply::Stream { head, chunk, every } => {
                        if stream.write_all(&head).await.is_err() {
                            closed.store(true, Ordering::SeqCst);
                            return;
                        }
                        loop {
                            if stream.write_all(&chunk).await.is_err()
                                || stream.flush().await.is_err()
                            {
                                closed.store(true, Ordering::SeqCst);
                                return;
                            }
                            tokio::time::sleep(every).await;
                        }
                    }
                }
            });
        }
    });
    Server { port, seen, closed }
}

/// A resolver with scripted answers; counts its calls.
#[derive(Default)]
pub(crate) struct FakeResolver {
    pub(crate) answers: Mutex<HashMap<String, Vec<Vec<IpAddr>>>>,
    pub(crate) calls: AtomicUsize,
}

impl FakeResolver {
    pub(crate) fn with(host: &str, answers: Vec<Vec<IpAddr>>) -> Arc<Self> {
        let r = Self::default();
        r.answers.lock().unwrap().insert(host.to_string(), answers);
        Arc::new(r)
    }
}

#[async_trait]
impl HostResolver for FakeResolver {
    async fn resolve(&self, host: &str, _port: u16) -> std::io::Result<Vec<IpAddr>> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        let mut answers = self.answers.lock().unwrap();
        let list = answers
            .get_mut(host)
            .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::NotFound, "no such host"))?;
        Ok(if list.len() > 1 {
            list.remove(0)
        } else {
            list[0].clone()
        })
    }
}

pub(crate) fn ip(text: &str) -> IpAddr {
    text.parse().unwrap()
}

/// A credential store that answers only after the given time.
pub(crate) struct SlowBackend(pub(crate) Duration);

impl nodespace_core::secrets::SecretBackend for SlowBackend {
    fn set(&self, _a: &str, _v: &str) -> Result<(), nodespace_core::secrets::SecretError> {
        Ok(())
    }
    fn get(&self, _a: &str) -> Result<Option<String>, nodespace_core::secrets::SecretError> {
        std::thread::sleep(self.0);
        Ok(Some("SECRETVALUE-123".to_string()))
    }
    fn delete(&self, _a: &str) -> Result<bool, nodespace_core::secrets::SecretError> {
        Ok(false)
    }
}
