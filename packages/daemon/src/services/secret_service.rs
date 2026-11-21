//! tonic `SecretService`: the CLI's way into the device credential store.
//!
//! The daemon owns the store so that one process creates and reads every item
//! (the operating system ties an item's access to the program that made it).
//! The service is process-global, not routed by database.
//!
//! A value enters only through `Set`. Nothing returned by this service, and no
//! error it produces, carries a value: failures map to a status whose message
//! is the [`SecretError`] text, which names the rule or the reference only.

use std::sync::Arc;

use nodespace_core::secrets::{SecretError, SecretStore};
use tonic::{Request, Response, Status};
use zeroize::Zeroize;

use crate::nodespace::secret_service_server::SecretService as GrpcSecretService;
use crate::nodespace::{
    DeleteSecretRequest, DeleteSecretResponse, ListSecretNamesRequest, ListSecretNamesResponse,
    SetSecretRequest, SetSecretResponse,
};
use crate::services::remote_confirmation::AnswerGate;

/// What a peer the gate refuses is told.
fn refused() -> Status {
    Status::permission_denied("this client may not change credentials")
}

/// A refused `Set`: its value is wiped like on every other path.
fn refuse_and_wipe(request: &mut SetSecretRequest) -> Status {
    request.value.zeroize();
    refused()
}

/// gRPC `SecretService` backed by a [`SecretStore`].
#[derive(Clone)]
pub struct SecretServiceImpl {
    store: Arc<SecretStore>,
    /// Who may change a credential. `None` for a caller that is trusted by
    /// construction (in-process use, tests, a transport with no peer identity).
    gate: Option<Arc<dyn AnswerGate>>,
}

impl SecretServiceImpl {
    pub fn new(store: Arc<SecretStore>) -> Self {
        Self { store, gate: None }
    }

    /// The service with a gate on what changes a credential: `Set` and `Delete`
    /// are refused for a peer that is a program the daemon started for a tool (it
    /// could overwrite or remove a credential the user relies on) or one the
    /// daemon cannot identify. `ListNames` shows names only and is not gated.
    pub fn with_gate(store: Arc<SecretStore>, gate: Arc<dyn AnswerGate>) -> Self {
        Self {
            store,
            gate: Some(gate),
        }
    }

    /// Whether the peer of `request` may change a credential.
    fn may_change<T>(&self, request: &Request<T>) -> bool {
        self.gate.as_deref().is_none_or(|gate| {
            crate::services::remote_confirmation::peer_allowed(gate, request, "change a credential")
        })
    }

    /// The service over the operating system's credential store.
    pub fn keychain() -> Self {
        let store = Arc::new(SecretStore::keychain());
        // A Windows named pipe carries no peer pid, so there is nothing to judge
        // there: the pipe's own access control is the barrier on that platform.
        #[cfg(unix)]
        {
            Self::with_gate(
                store,
                Arc::new(
                    crate::services::remote_confirmation::SpawnedProcessGate::for_this_process(),
                ),
            )
        }
        #[cfg(not(unix))]
        {
            Self::new(store)
        }
    }
}

fn status(error: SecretError) -> Status {
    match error {
        SecretError::InvalidName | SecretError::InvalidValue(_) | SecretError::TooLong { .. } => {
            Status::invalid_argument(error.to_string())
        }
        SecretError::TooManySecrets { .. } => Status::resource_exhausted(error.to_string()),
        SecretError::NotSet(_) | SecretError::EnvNotSet(_) => Status::not_found(error.to_string()),
        SecretError::Store(_) => Status::unavailable(error.to_string()),
        SecretError::NotRestored { .. } => Status::internal(error.to_string()),
    }
}

/// Stores the request's value, then overwrites it: the request is the daemon's
/// last copy of the value, and it is wiped whether the store accepted it or
/// not.
fn store_and_wipe(store: &SecretStore, request: &mut SetSecretRequest) -> Result<(), SecretError> {
    let result = store.set(&request.name, &request.value);
    request.value.zeroize();
    result
}

#[tonic::async_trait]
impl GrpcSecretService for SecretServiceImpl {
    async fn set(
        &self,
        request: Request<SetSecretRequest>,
    ) -> Result<Response<SetSecretResponse>, Status> {
        let allowed = self.may_change(&request);
        let mut req = request.into_inner();
        if !allowed {
            return Err(refuse_and_wipe(&mut req));
        }
        // The OS store can block (a prompt, a locked keychain): keep it off the
        // async worker threads.
        let store = Arc::clone(&self.store);
        tokio::task::spawn_blocking(move || store_and_wipe(&store, &mut req))
            .await
            .map_err(|_| Status::internal("credential store task failed"))?
            .map_err(status)?;
        Ok(Response::new(SetSecretResponse {}))
    }

    async fn delete(
        &self,
        request: Request<DeleteSecretRequest>,
    ) -> Result<Response<DeleteSecretResponse>, Status> {
        if !self.may_change(&request) {
            return Err(refused());
        }
        let name = request.into_inner().name;
        let store = Arc::clone(&self.store);
        let existed = tokio::task::spawn_blocking(move || store.delete(&name))
            .await
            .map_err(|_| Status::internal("credential store task failed"))?
            .map_err(status)?;
        Ok(Response::new(DeleteSecretResponse { existed }))
    }

    async fn list_names(
        &self,
        _request: Request<ListSecretNamesRequest>,
    ) -> Result<Response<ListSecretNamesResponse>, Status> {
        let store = Arc::clone(&self.store);
        let names = tokio::task::spawn_blocking(move || store.names())
            .await
            .map_err(|_| Status::internal("credential store task failed"))?
            .map_err(status)?;
        Ok(Response::new(ListSecretNamesResponse { names }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nodespace_core::secrets::InMemorySecretBackend;
    use prost::Message;

    const TOKEN: &str = "ghp_SUPERSECRETVALUE123";

    fn service() -> SecretServiceImpl {
        SecretServiceImpl::new(Arc::new(SecretStore::new(Arc::new(
            InMemorySecretBackend::default(),
        ))))
    }

    async fn set(s: &SecretServiceImpl, name: &str, value: &str) -> Result<(), Status> {
        s.set(Request::new(SetSecretRequest {
            name: name.into(),
            value: value.into(),
        }))
        .await
        .map(|_| ())
    }

    #[test]
    fn the_request_value_is_wiped_whether_the_store_accepts_it_or_not() {
        let store = SecretStore::new(Arc::new(InMemorySecretBackend::default()));
        // Accepted.
        let mut ok = SetSecretRequest {
            name: "tok".into(),
            value: TOKEN.into(),
        };
        let ptr = ok.value.as_ptr();
        let len = ok.value.len();
        assert!(store_and_wipe(&store, &mut ok).is_ok());
        assert!(ok.value.is_empty());
        // The buffer that held it is zeroed, not just shortened.
        let bytes = unsafe { std::slice::from_raw_parts(ptr, len) };
        assert!(bytes.iter().all(|b| *b == 0), "the value bytes were left");
        // Refused (a bad name).
        let mut bad = SetSecretRequest {
            name: "bad name".into(),
            value: TOKEN.into(),
        };
        let ptr = bad.value.as_ptr();
        let len = bad.value.len();
        assert!(store_and_wipe(&store, &mut bad).is_err());
        assert!(bad.value.is_empty());
        let bytes = unsafe { std::slice::from_raw_parts(ptr, len) };
        assert!(bytes.iter().all(|b| *b == 0), "the value bytes were left");
    }

    #[tokio::test]
    async fn set_then_list_returns_names_and_no_value_anywhere_in_the_response() {
        let s = service();
        set(&s, "tok", TOKEN).await.unwrap();
        let resp = s
            .list_names(Request::new(ListSecretNamesRequest {}))
            .await
            .unwrap()
            .into_inner();
        assert_eq!(resp.names, vec!["tok".to_string()]);
        assert!(!format!("{resp:?}").contains(TOKEN));
        let wire = resp.encode_to_vec();
        assert!(!String::from_utf8_lossy(&wire).contains(TOKEN));
    }

    #[tokio::test]
    async fn delete_reports_whether_it_existed() {
        let s = service();
        set(&s, "tok", TOKEN).await.unwrap();
        let del = |n: &str| s.delete(Request::new(DeleteSecretRequest { name: n.into() }));
        assert!(del("tok").await.unwrap().into_inner().existed);
        assert!(!del("tok").await.unwrap().into_inner().existed);
        let names = s
            .list_names(Request::new(ListSecretNamesRequest {}))
            .await
            .unwrap()
            .into_inner()
            .names;
        assert!(names.is_empty());
    }

    #[tokio::test]
    async fn a_bad_name_or_value_is_invalid_argument_and_the_error_carries_no_value() {
        let s = service();
        let err = set(&s, "bad name", TOKEN).await.unwrap_err();
        assert_eq!(err.code(), tonic::Code::InvalidArgument);
        assert!(!err.message().contains(TOKEN));
        let err = set(&s, "ok", "").await.unwrap_err();
        assert_eq!(err.code(), tonic::Code::InvalidArgument);
        let err = s
            .delete(Request::new(DeleteSecretRequest { name: "a b".into() }))
            .await
            .unwrap_err();
        assert_eq!(err.code(), tonic::Code::InvalidArgument);
    }

    #[tokio::test]
    async fn a_platform_limit_is_reported_with_its_own_status_and_message() {
        struct Limited;
        impl nodespace_core::secrets::SecretBackend for Limited {
            fn set(&self, _: &str, _: &str) -> Result<(), SecretError> {
                Ok(())
            }
            fn get(&self, _: &str) -> Result<Option<String>, SecretError> {
                Ok(None)
            }
            fn delete(&self, _: &str) -> Result<bool, SecretError> {
                Ok(false)
            }
            fn max_item_size(&self) -> Option<usize> {
                Some(8)
            }
        }
        let s = SecretServiceImpl::new(Arc::new(SecretStore::new(Arc::new(Limited))));
        let err = set(&s, "tok", "this value is over eight bytes")
            .await
            .unwrap_err();
        assert_eq!(err.code(), tonic::Code::InvalidArgument);
        assert!(err.message().contains("too long for this platform"));
        assert!(!err.message().contains("over eight"));
    }

    #[tokio::test]
    async fn a_store_failure_is_unavailable_and_names_no_value() {
        struct Broken;
        impl nodespace_core::secrets::SecretBackend for Broken {
            fn set(&self, _: &str, _: &str) -> Result<(), SecretError> {
                Err(SecretError::Store("locked".into()))
            }
            fn get(&self, _: &str) -> Result<Option<String>, SecretError> {
                Err(SecretError::Store("locked".into()))
            }
            fn delete(&self, _: &str) -> Result<bool, SecretError> {
                Err(SecretError::Store("locked".into()))
            }
        }
        let s = SecretServiceImpl::new(Arc::new(SecretStore::new(Arc::new(Broken))));
        let err = set(&s, "tok", TOKEN).await.unwrap_err();
        assert_eq!(err.code(), tonic::Code::Unavailable);
        assert!(!err.message().contains(TOKEN));
    }

    // -- who may change a credential ----------------------------------------

    struct Peer(Option<u32>);
    impl AnswerGate for Peer {
        fn may_answer(&self, pid: Option<u32>) -> bool {
            pid.is_some() && pid == self.0
        }
    }

    #[tokio::test]
    async fn a_refused_peer_can_neither_set_nor_delete_a_credential() {
        let store = Arc::new(SecretStore::new(Arc::new(InMemorySecretBackend::default())));
        let s = SecretServiceImpl::with_gate(Arc::clone(&store), Arc::new(Peer(Some(4242))));
        store.set("tok", "ghp_ORIGINALVALUE1").unwrap();
        // A plain in-process request has no peer pid: the gate refuses it.
        let denied = set(&s, "tok", "ghp_OVERWRITTENVALUE").await.unwrap_err();
        assert_eq!(denied.code(), tonic::Code::PermissionDenied);
        let denied = set(&s, "new", TOKEN).await.unwrap_err();
        assert_eq!(denied.code(), tonic::Code::PermissionDenied);
        let denied = s
            .delete(Request::new(DeleteSecretRequest { name: "tok".into() }))
            .await
            .unwrap_err();
        assert_eq!(denied.code(), tonic::Code::PermissionDenied);
        // Nothing changed, and listing names still works.
        assert_eq!(store.names().unwrap(), ["tok"]);
        assert_eq!(
            store.get("tok").unwrap().unwrap().expose(),
            "ghp_ORIGINALVALUE1"
        );
        assert!(s
            .list_names(Request::new(ListSecretNamesRequest {}))
            .await
            .is_ok());
    }

    #[test]
    fn a_refused_set_wipes_its_value() {
        let mut request = SetSecretRequest {
            name: "tok".into(),
            value: TOKEN.into(),
        };
        let status = refuse_and_wipe(&mut request);
        assert_eq!(status.code(), tonic::Code::PermissionDenied);
        assert!(request.value.is_empty());
    }

    #[tokio::test]
    async fn an_ungated_service_keeps_working_for_a_trusted_caller() {
        let s = service();
        assert!(set(&s, "tok", TOKEN).await.is_ok());
    }

    #[test]
    fn the_peer_pid_of_a_request_with_no_connection_is_unknown() {
        assert_eq!(
            crate::services::remote_confirmation::peer_pid(&Request::new(())),
            None
        );
    }

    // -- the real socket path ------------------------------------------------

    #[cfg(unix)]
    mod over_a_socket {
        use super::*;
        use crate::nodespace::secret_service_client::SecretServiceClient;
        use crate::nodespace::secret_service_server::SecretServiceServer;
        use crate::services::remote_confirmation::SpawnedProcessGate;
        use hyper_util::rt::TokioIo;
        use tonic::transport::{Endpoint, Server, Uri};
        use tower::service_fn;

        async fn serve(
            svc: SecretServiceImpl,
        ) -> (
            SecretServiceClient<tonic::transport::Channel>,
            tempfile::TempDir,
        ) {
            let dir = tempfile::Builder::new()
                .prefix("ns-sec-")
                .tempdir_in("/private/tmp")
                .unwrap_or_else(|_| tempfile::tempdir().unwrap());
            let sock = dir.path().join("s.sock");
            let listener = tokio::net::UnixListener::bind(&sock).unwrap();
            tokio::spawn(async move {
                Server::builder()
                    .add_service(SecretServiceServer::new(svc))
                    .serve_with_incoming(crate::peer_auth::owner_only_incoming(listener))
                    .await
            });
            let path = sock.clone();
            let channel = Endpoint::from_static("http://localhost")
                .connect_with_connector(service_fn(move |_: Uri| {
                    let path = path.clone();
                    async move {
                        tokio::net::UnixStream::connect(&path)
                            .await
                            .map(TokioIo::new)
                    }
                }))
                .await
                .unwrap();
            (SecretServiceClient::new(channel), dir)
        }

        fn set_request(name: &str, value: &str) -> SetSecretRequest {
            SetSecretRequest {
                name: name.into(),
                value: value.into(),
            }
        }

        /// The daemon's own gate, over a real socket, with this test process
        /// as the daemon: the peer is the daemon itself, so it is refused.
        #[tokio::test]
        async fn the_daemon_gate_refuses_a_peer_that_is_the_daemon() {
            let store = Arc::new(SecretStore::new(Arc::new(InMemorySecretBackend::default())));
            store.set("tok", "ghp_ORIGINALVALUE1").unwrap();
            let svc = SecretServiceImpl::with_gate(
                Arc::clone(&store),
                Arc::new(SpawnedProcessGate::for_this_process()),
            );
            let (mut client, _dir) = serve(svc).await;
            let denied = client
                .set(set_request("tok", "ghp_OVERWRITTENVALUE"))
                .await
                .unwrap_err();
            assert_eq!(denied.code(), tonic::Code::PermissionDenied);
            let denied = client
                .delete(DeleteSecretRequest { name: "tok".into() })
                .await
                .unwrap_err();
            assert_eq!(denied.code(), tonic::Code::PermissionDenied);
            assert_eq!(
                store.get("tok").unwrap().unwrap().expose(),
                "ghp_ORIGINALVALUE1"
            );
            let names = client
                .list_names(ListSecretNamesRequest {})
                .await
                .unwrap()
                .into_inner()
                .names;
            assert_eq!(names, ["tok"]);
        }

        /// The kernel's pid for the connection is what the gate judges: a gate
        /// that lets exactly this process through lets the change happen.
        #[tokio::test]
        async fn an_allowed_peer_changes_a_credential_over_the_socket() {
            let store = Arc::new(SecretStore::new(Arc::new(InMemorySecretBackend::default())));
            let svc = SecretServiceImpl::with_gate(
                Arc::clone(&store),
                Arc::new(Peer(Some(std::process::id()))),
            );
            let (mut client, _dir) = serve(svc).await;
            client.set(set_request("tok", TOKEN)).await.unwrap();
            assert_eq!(store.get("tok").unwrap().unwrap().expose(), TOKEN);
            let removed = client
                .delete(DeleteSecretRequest { name: "tok".into() })
                .await
                .unwrap()
                .into_inner();
            assert!(removed.existed);
            assert!(store.names().unwrap().is_empty());
        }
    }
}
