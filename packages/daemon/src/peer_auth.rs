//! Same-user peer check for the daemon's Unix socket (ADR-052 section 2).
//!
//! The socket's `0o600` mode is the first layer; this is the second. Every
//! accepted connection has its peer's user id read from the kernel
//! (`SO_PEERCRED` on Linux, `LOCAL_PEERCRED` on macOS, through tokio's
//! `UnixStream::peer_cred`) and compared with the daemon's own effective uid.
//! A connection from any other uid is closed before it reaches the gRPC layer.
//!
//! A refusal never surfaces as a stream error: tonic treats an `Err` from the
//! incoming stream as fatal and stops serving, so one foreign connection would
//! take the daemon down. A refused peer is dropped and the accept loop goes on.

use std::io;
use std::pin::Pin;
use std::task::{Context, Poll};

use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio::net::unix::UCred;
use tokio::net::{UnixListener, UnixStream};
use tokio_stream::Stream;
use tonic::transport::server::Connected;

pub use crate::peer_signature::AuditToken;

/// What the daemon learned about a peer when it accepted the connection. It is
/// read once, at accept time, and travels with the connection into every
/// request as the request's connect info.
#[derive(Clone, Debug)]
pub struct PeerInfo {
    /// The peer's credentials as the kernel reports them (uid, pid).
    pub cred: Option<UCred>,
    /// The peer's audit token (macOS `LOCAL_PEERTOKEN`); `None` elsewhere and
    /// when the kernel did not give one.
    pub audit_token: Option<AuditToken>,
}

/// The peer's audit token, read from the connected socket.
#[cfg(target_os = "macos")]
fn read_audit_token(stream: &UnixStream) -> Option<AuditToken> {
    use std::os::fd::AsRawFd;
    /// `SOL_LOCAL`.
    const SOL_LOCAL: libc::c_int = 0;
    /// `LOCAL_PEERTOKEN`: the audit token of the process that connected.
    const LOCAL_PEERTOKEN: libc::c_int = 0x006;
    let mut token = [0u32; 8];
    let mut len = std::mem::size_of_val(&token) as libc::socklen_t;
    // SAFETY: `token` is a live, writable buffer of `len` bytes, which is the
    // size of an `audit_token_t` that LOCAL_PEERTOKEN fills in.
    let status = unsafe {
        libc::getsockopt(
            stream.as_raw_fd(),
            SOL_LOCAL,
            LOCAL_PEERTOKEN,
            token.as_mut_ptr().cast(),
            &mut len,
        )
    };
    (status == 0 && len as usize == std::mem::size_of_val(&token)).then_some(AuditToken(token))
}

#[cfg(not(target_os = "macos"))]
fn read_audit_token(_stream: &UnixStream) -> Option<AuditToken> {
    None
}

/// A connection the daemon accepted, carrying what was read about its peer at
/// accept time. Reads and writes go straight to the socket.
pub struct PeerStream {
    inner: UnixStream,
    info: PeerInfo,
}

impl PeerStream {
    /// Read the peer's credentials and audit token off `stream`.
    pub fn new(stream: UnixStream) -> Self {
        let info = PeerInfo {
            cred: stream.peer_cred().ok(),
            audit_token: read_audit_token(&stream),
        };
        Self {
            inner: stream,
            info,
        }
    }

    /// The socket, for the accept check.
    pub fn stream(&self) -> &UnixStream {
        &self.inner
    }

    pub fn info(&self) -> &PeerInfo {
        &self.info
    }
}

impl Connected for PeerStream {
    type ConnectInfo = PeerInfo;

    fn connect_info(&self) -> Self::ConnectInfo {
        self.info.clone()
    }
}

impl AsyncRead for PeerStream {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        Pin::new(&mut self.inner).poll_read(cx, buf)
    }
}

impl AsyncWrite for PeerStream {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        Pin::new(&mut self.inner).poll_write(cx, buf)
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.inner).poll_flush(cx)
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.inner).poll_shutdown(cx)
    }

    fn poll_write_vectored(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        bufs: &[io::IoSlice<'_>],
    ) -> Poll<io::Result<usize>> {
        Pin::new(&mut self.inner).poll_write_vectored(cx, bufs)
    }

    fn is_write_vectored(&self) -> bool {
        self.inner.is_write_vectored()
    }
}

/// The boxed accept stream tonic serves from.
pub type IncomingStream = Pin<Box<dyn Stream<Item = io::Result<PeerStream>> + Send>>;

/// Whether a peer with credentials `cred` may be served by a daemon running as
/// effective uid `our_euid`.
///
/// Strict equality, and it fails closed: a peer whose credentials could not be
/// read is refused. Root is not special-cased: a root peer is refused when the
/// daemon is not root, exactly like any other foreign uid, so the daemon never
/// serves another account's process on the strength of that account's power.
pub fn peer_allowed(cred: io::Result<UCred>, our_euid: u32) -> bool {
    match cred {
        Ok(cred) => uid_matches(cred.uid(), our_euid),
        Err(_) => false,
    }
}

fn uid_matches(peer_uid: u32, our_euid: u32) -> bool {
    peer_uid == our_euid
}

/// The daemon's own effective uid.
fn our_euid() -> u32 {
    // SAFETY: geteuid has no preconditions and cannot fail.
    unsafe { libc::geteuid() }
}

/// Accept connections from `listener`, serving only peers with the daemon's
/// own effective uid.
pub fn owner_only_incoming(listener: UnixListener) -> IncomingStream {
    owner_only_incoming_for_euid(listener, our_euid())
}

/// [`owner_only_incoming`] for a daemon running as `euid`. The production
/// function calls this with the real effective uid; a test passes another uid
/// to drive the real check (credential read, comparison, logging) against a
/// real same-user connection, which must then be refused.
pub fn owner_only_incoming_for_euid(listener: UnixListener, euid: u32) -> IncomingStream {
    owner_only_incoming_with(listener, move |stream| {
        let cred = stream.stream().peer_cred();
        let (uid, pid) = match &cred {
            Ok(c) => (Some(c.uid()), c.pid()),
            Err(_) => (None, None),
        };
        let allowed = peer_allowed(cred, euid);
        if !allowed {
            tracing::warn!(?uid, ?pid, "refused a connection from another user");
        }
        allowed
    })
}

/// [`owner_only_incoming`] with the per-connection decision supplied by
/// `check`, so a test can drive the refusal path without a second user account.
/// A stream `check` refuses is dropped and the loop continues; only an accept
/// failure is yielded as an error.
pub fn owner_only_incoming_with<F>(listener: UnixListener, check: F) -> IncomingStream
where
    F: Fn(&PeerStream) -> bool + Send + 'static,
{
    Box::pin(async_stream::stream! {
        loop {
            match listener.accept().await {
                Ok((stream, _addr)) => {
                    // The audit token is read here, once, before anything is
                    // read from the peer.
                    let stream = PeerStream::new(stream);
                    if check(&stream) {
                        yield Ok(stream);
                    }
                }
                Err(e) => yield Err(e),
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use hyper_util::rt::TokioIo;
    use nodespace_proto::nodespace::database_service_server::DatabaseService;
    use nodespace_proto::nodespace::{
        CreateDatabaseRequest, DatabaseInfo, ListDatabasesRequest, ListDatabasesResponse,
        RegisterDatabaseRequest, RemoveDatabaseRequest, RemoveDatabaseResponse,
        RenameDatabaseRequest, SetDefaultDatabaseRequest,
    };
    use nodespace_proto::{DatabaseServiceClient, DatabaseServiceServer};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;
    use tonic::transport::{Channel, Endpoint, Server, Uri};
    use tonic::{Request, Response, Status};
    use tower::service_fn;

    // A real UCred for the current process, taken from a socketpair.
    fn own_cred() -> UCred {
        let (a, _b) = std::os::unix::net::UnixStream::pair().unwrap();
        a.set_nonblocking(true).unwrap();
        // Registering with tokio needs a runtime; peer_cred itself does not
        // touch the reactor, so read it through a std-to-tokio conversion.
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let _g = rt.enter();
        UnixStream::from_std(a).unwrap().peer_cred().unwrap()
    }

    #[test]
    fn same_uid_is_allowed() {
        let cred = own_cred();
        assert!(peer_allowed(Ok(cred), cred.uid()));
    }

    #[test]
    fn other_uid_is_refused() {
        let cred = own_cred();
        assert!(!peer_allowed(Ok(cred), cred.uid().wrapping_add(1)));
    }

    #[test]
    fn root_peer_is_refused_when_we_are_not_root() {
        assert!(!uid_matches(0, 501));
        assert!(uid_matches(0, 0));
    }

    #[test]
    fn unreadable_credentials_are_refused() {
        let err = io::Error::from(io::ErrorKind::PermissionDenied);
        assert!(!peer_allowed(Err(err), our_euid()));
    }

    #[derive(Default)]
    struct CountingDb {
        lists: Arc<AtomicUsize>,
    }

    #[tonic::async_trait]
    impl DatabaseService for CountingDb {
        async fn list(
            &self,
            _: Request<ListDatabasesRequest>,
        ) -> Result<Response<ListDatabasesResponse>, Status> {
            self.lists.fetch_add(1, Ordering::SeqCst);
            Ok(Response::new(ListDatabasesResponse::default()))
        }
        async fn create(
            &self,
            _: Request<CreateDatabaseRequest>,
        ) -> Result<Response<DatabaseInfo>, Status> {
            Err(Status::unimplemented("test stub"))
        }
        async fn register(
            &self,
            _: Request<RegisterDatabaseRequest>,
        ) -> Result<Response<DatabaseInfo>, Status> {
            Err(Status::unimplemented("test stub"))
        }
        async fn remove(
            &self,
            _: Request<RemoveDatabaseRequest>,
        ) -> Result<Response<RemoveDatabaseResponse>, Status> {
            Err(Status::unimplemented("test stub"))
        }
        async fn set_default(
            &self,
            _: Request<SetDefaultDatabaseRequest>,
        ) -> Result<Response<DatabaseInfo>, Status> {
            Err(Status::unimplemented("test stub"))
        }
        async fn rename(
            &self,
            _: Request<RenameDatabaseRequest>,
        ) -> Result<Response<DatabaseInfo>, Status> {
            Err(Status::unimplemented("test stub"))
        }
    }

    async fn dial(sock: std::path::PathBuf) -> Result<Channel, tonic::transport::Error> {
        Endpoint::from_static("http://localhost")
            .connect_with_connector(service_fn(move |_: Uri| {
                let sock = sock.clone();
                async move { UnixStream::connect(&sock).await.map(TokioIo::new) }
            }))
            .await
    }

    // Short path under /private/tmp: a tempdir under the default macOS temp
    // root can exceed the ~104 byte sockaddr_un limit.
    fn short_tempdir() -> tempfile::TempDir {
        let base = std::path::Path::new("/private/tmp");
        let base = if base.is_dir() {
            base
        } else {
            std::path::Path::new("/tmp")
        };
        tempfile::Builder::new()
            .prefix("ns-pa-")
            .tempdir_in(base)
            .unwrap()
    }

    // Real server and real tonic clients over a UDS; the check refuses the
    // first connection and allows the rest.
    #[tokio::test]
    async fn refused_peer_gets_a_transport_error_and_next_peer_is_served() {
        let dir = short_tempdir();
        let sock = dir.path().join("s.sock");
        let listener = UnixListener::bind(&sock).unwrap();
        let lists = Arc::new(AtomicUsize::new(0));
        let svc = CountingDb {
            lists: lists.clone(),
        };

        let refuse = Arc::new(std::sync::atomic::AtomicBool::new(true));
        let flag = refuse.clone();
        let incoming = owner_only_incoming_with(listener, move |_| !flag.load(Ordering::SeqCst));
        let server = tokio::spawn(async move {
            Server::builder()
                .add_service(DatabaseServiceServer::new(svc))
                .serve_with_incoming(incoming)
                .await
        });

        // While the check refuses, every connection is closed by the server, so
        // the HTTP/2 handshake or the call fails. A tonic channel reconnects on
        // its own, so refusing every attempt is what makes the outcome certain.
        let refused = tokio::time::timeout(std::time::Duration::from_secs(10), async {
            match dial(sock.clone()).await {
                Err(_) => true,
                Ok(ch) => DatabaseServiceClient::new(ch)
                    .list(ListDatabasesRequest {})
                    .await
                    .is_err(),
            }
        })
        .await
        .expect("the refused client must get an error, not hang");
        assert!(refused, "the refused client must see a transport error");
        assert_eq!(
            lists.load(Ordering::SeqCst),
            0,
            "the server must never see the refused RPC"
        );

        // Same server, check now allows: the next client is served.
        refuse.store(false, Ordering::SeqCst);
        let ch = dial(sock.clone()).await.expect("allowed client connects");
        DatabaseServiceClient::new(ch)
            .list(ListDatabasesRequest {})
            .await
            .expect("allowed client's RPC succeeds");
        assert_eq!(lists.load(Ordering::SeqCst), 1);
        assert!(!server.is_finished(), "a refusal must not stop the server");
        server.abort();
    }

    // Drives the production closure (the real peer_cred read and comparison)
    // with a daemon uid that is not ours: a real same-user connection is then a
    // foreign one and must be refused; with the real euid it is served.
    #[tokio::test]
    async fn production_check_refuses_a_foreign_uid_and_serves_our_own() {
        for (daemon_euid, expect_served) in
            [(our_euid().wrapping_add(1), false), (our_euid(), true)]
        {
            let dir = short_tempdir();
            let sock = dir.path().join("s.sock");
            let listener = UnixListener::bind(&sock).unwrap();
            let lists = Arc::new(AtomicUsize::new(0));
            let svc = CountingDb {
                lists: lists.clone(),
            };
            let incoming = owner_only_incoming_for_euid(listener, daemon_euid);
            let server = tokio::spawn(async move {
                Server::builder()
                    .add_service(DatabaseServiceServer::new(svc))
                    .serve_with_incoming(incoming)
                    .await
            });

            let outcome = tokio::time::timeout(std::time::Duration::from_secs(10), async {
                match dial(sock.clone()).await {
                    Err(e) => Err(e.to_string()),
                    Ok(ch) => DatabaseServiceClient::new(ch)
                        .list(ListDatabasesRequest {})
                        .await
                        .map(|_| ())
                        .map_err(|e| e.to_string()),
                }
            })
            .await
            .expect("must resolve, not hang");

            if expect_served {
                outcome.expect("a peer with the daemon's own uid is served");
                assert_eq!(lists.load(Ordering::SeqCst), 1);
            } else {
                assert!(outcome.is_err(), "a foreign uid must get a transport error");
                assert_eq!(
                    lists.load(Ordering::SeqCst),
                    0,
                    "no RPC may reach the server"
                );
            }
            server.abort();
        }
    }

    // The daemon's identity is its effective uid. geteuid and getuid agree in
    // every test process, so no behavioural test can tell them apart; pin the
    // source instead. The needle is built from fragments to not match itself.
    #[test]
    fn our_identity_is_the_effective_uid() {
        let src = include_str!("peer_auth.rs");
        let real = ["libc::get", "euid()"].concat();
        let wrong = ["libc::get", "uid()"].concat();
        assert!(src.contains(&real));
        assert!(!src.contains(&wrong));
    }
}
