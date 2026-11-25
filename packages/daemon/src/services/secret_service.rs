//! tonic `SecretService`: the CLI's way into the device credential store.
//!
//! The daemon owns the store so that one process creates and reads every item
//! (the operating system ties an item's access to the program that made it).
//! The service is process-global, not routed by database.
//!
//! A value enters only through `Set`. Nothing returned by this service, and no
//! error it produces, carries a value: failures map to a status whose message
//! is the [`SecretError`] text, which names the rule or the reference only.
//!
//! `Set` and `Delete` change what a tool sends to a remote service, so the
//! daemon's service asks the person at the screen first (see
//! [`crate::user_presence`]): a program of the user cannot overwrite or remove
//! a credential on its own through the daemon. A request that is not confirmed
//! changes nothing, and a refused `Set` has its value wiped.

use std::sync::Arc;

use nodespace_core::secrets::{
    validate_secret_name, validate_secret_value, SecretError, SecretStore,
};
use tonic::{Request, Response, Status};
use zeroize::{Zeroize, Zeroizing};

use crate::nodespace::secret_service_server::SecretService as GrpcSecretService;
use crate::nodespace::{
    DeleteSecretRequest, DeleteSecretResponse, ListSecretNamesRequest, ListSecretNamesResponse,
    SetSecretRequest, SetSecretResponse,
};
use crate::services::remote_confirmation::AnswerGate;
use crate::user_presence::{credential_reason, UserPresence};

/// What a peer the gate refuses is told.
fn refused() -> Status {
    Status::permission_denied("this client may not change credentials")
}

/// A refused `Set`: its value is wiped like on every other path.
fn refuse_and_wipe(request: &mut SetSecretRequest) -> Status {
    request.value.zeroize();
    refused()
}

/// Wipes a `Set` request's value when it goes out of scope, on every path: a
/// refusal, a store failure, and a future dropped while the person is still
/// being asked (a client that hung up). Holds the value where it lies.
struct WipeOnDrop<'a>(&'a mut String);

impl Drop for WipeOnDrop<'_> {
    fn drop(&mut self) {
        self.0.zeroize();
    }
}

/// gRPC `SecretService` backed by a [`SecretStore`].
#[derive(Clone)]
pub struct SecretServiceImpl {
    store: Arc<SecretStore>,
    /// Who may change a credential. `None` for a caller that is trusted by
    /// construction (in-process use, tests, a transport with no peer identity).
    gate: Option<Arc<dyn AnswerGate>>,
    /// Asks the person at the screen before a credential is changed. Always
    /// there: a service that should ask no one is given a check that says yes
    /// (tests do), never none.
    presence: Arc<UserPresence>,
}

impl SecretServiceImpl {
    /// The service with no peer gate, asking the person at the screen
    /// through `presence` before `Set` or `Delete` changes anything.
    pub fn new(store: Arc<SecretStore>, presence: Arc<UserPresence>) -> Self {
        Self {
            store,
            gate: None,
            presence,
        }
    }

    /// The service with a gate on what changes a credential: `Set` and `Delete`
    /// are refused for a peer that is a program the daemon started for a tool (it
    /// could overwrite or remove a credential the user relies on) or one the
    /// daemon cannot identify, and then asked of the person at the screen.
    /// `ListNames` shows names only and is neither gated nor asked.
    pub fn with_gate(
        store: Arc<SecretStore>,
        gate: Arc<dyn AnswerGate>,
        presence: Arc<UserPresence>,
    ) -> Self {
        Self {
            store,
            gate: Some(gate),
            presence,
        }
    }

    /// Whether the person at the screen confirmed changing `name`.
    async fn confirmed(&self, name: &str) -> Result<(), Status> {
        self.presence
            .confirm(credential_reason(name))
            .await
            .map_err(|refusal| {
                tracing::info!(?refusal, "a credential change was not confirmed");
                refusal.status()
            })
    }

    /// Whether the peer of `request` may change a credential.
    fn may_change<T>(&self, request: &Request<T>) -> bool {
        self.gate.as_deref().is_none_or(|gate| {
            crate::services::remote_confirmation::peer_allowed(gate, request, "change a credential")
        })
    }

    /// `Set`, on the request in place: the value is wiped where it lies on
    /// every path that does not store it (a refusal, a bad value, a dialog that
    /// is not confirmed, a future dropped while the dialog is open), and an
    /// accepted one is moved out and wiped once stored, so no copy of the value
    /// outlives the call.
    async fn set_in_place(
        &self,
        request: &mut Request<SetSecretRequest>,
    ) -> Result<Response<SetSecretResponse>, Status> {
        if !self.may_change(request) {
            return Err(refuse_and_wipe(request.get_mut()));
        }
        let SetSecretRequest { name, value } = request.get_mut();
        let value = WipeOnDrop(value);
        // A request the store would refuse is refused before anyone is asked.
        let checked =
            validate_secret_name(name).and_then(|()| validate_secret_value(value.0.as_str()));
        if let Err(error) = checked {
            return Err(status(error));
        }
        self.confirmed(name).await?;
        // Moved into memory that is wiped when it is dropped, however the
        // store task ends (a panic in it included).
        let name = name.clone();
        let value_to_store = Zeroizing::new(std::mem::take(&mut *value.0));
        drop(value);
        // The OS store can block (a prompt, a locked keychain): keep it off the
        // async worker threads.
        let store = Arc::clone(&self.store);
        tokio::task::spawn_blocking(move || store.set(&name, &value_to_store))
            .await
            .map_err(|_| Status::internal("credential store task failed"))?
            .map_err(status)?;
        Ok(Response::new(SetSecretResponse {}))
    }

    /// The service over the operating system's credential store with the peer
    /// gate, asking the person at the screen through `presence` (the one
    /// `crate::install_approval_services` installed) before a change. Where the
    /// platform has no way to ask, every `Set` and `Delete` is refused.
    pub fn keychain_with_presence(presence: Arc<UserPresence>) -> Self {
        let store = Arc::new(crate::team_keychain::credential_store());
        // A Windows named pipe carries no peer pid, so there is nothing to judge
        // there: the pipe's own access control is the barrier on that platform.
        #[cfg(unix)]
        {
            Self::with_gate(
                store,
                Arc::new(
                    crate::services::remote_confirmation::SpawnedProcessGate::for_this_process(),
                ),
                presence,
            )
        }
        #[cfg(not(unix))]
        {
            Self::new(store, presence)
        }
    }

    /// The service over the operating system's credential store for a daemon
    /// that has not installed a way to ask the person at the screen: every
    /// `Set` and `Delete` is refused, saying that
    /// `crate::install_approval_services` was not called. A daemon builds
    /// [`Self::keychain_with_presence`] with what that returns instead.
    pub fn keychain() -> Self {
        Self::keychain_with_presence(Arc::new(UserPresence::not_installed()))
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

#[tonic::async_trait]
impl GrpcSecretService for SecretServiceImpl {
    async fn set(
        &self,
        mut request: Request<SetSecretRequest>,
    ) -> Result<Response<SetSecretResponse>, Status> {
        self.set_in_place(&mut request).await
    }

    async fn delete(
        &self,
        request: Request<DeleteSecretRequest>,
    ) -> Result<Response<DeleteSecretResponse>, Status> {
        if !self.may_change(&request) {
            return Err(refused());
        }
        let name = request.into_inner().name;
        validate_secret_name(&name).map_err(status)?;
        self.confirmed(&name).await?;
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

    /// A check that says yes to every change, with no quiet time.
    fn yes() -> Arc<UserPresence> {
        Arc::new(UserPresence::without_quiet_time(
            crate::user_presence::fake::FakePresence::new(crate::user_presence::fake::Answer::Yes),
        ))
    }

    fn service() -> SecretServiceImpl {
        SecretServiceImpl::new(
            Arc::new(SecretStore::new(Arc::new(InMemorySecretBackend::default()))),
            yes(),
        )
    }

    async fn set(s: &SecretServiceImpl, name: &str, value: &str) -> Result<(), Status> {
        s.set(Request::new(SetSecretRequest {
            name: name.into(),
            value: value.into(),
        }))
        .await
        .map(|_| ())
    }

    /// A store that panics while it writes fails the `Set` with an error,
    /// not the daemon. The value had already left the request for memory that
    /// is wiped when dropped (`Zeroizing`), which a panic drops too.
    #[tokio::test]
    async fn a_store_that_panics_while_writing_fails_the_set_and_the_request_holds_no_value() {
        struct Panics;
        impl nodespace_core::secrets::SecretBackend for Panics {
            fn set(&self, _: &str, _: &str) -> Result<(), SecretError> {
                panic!("the platform store failed in the middle of a write")
            }
            fn get(&self, _: &str) -> Result<Option<String>, SecretError> {
                Ok(None)
            }
            fn delete(&self, _: &str) -> Result<bool, SecretError> {
                Ok(false)
            }
        }
        let s = SecretServiceImpl::new(Arc::new(SecretStore::new(Arc::new(Panics))), yes());
        let mut request = Request::new(SetSecretRequest {
            name: "tok".into(),
            value: TOKEN.into(),
        });
        let failed = s.set_in_place(&mut request).await.unwrap_err();
        assert_eq!(failed.code(), tonic::Code::Internal);
        assert!(request.get_ref().value.is_empty());
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
        let s = SecretServiceImpl::new(Arc::new(SecretStore::new(Arc::new(Limited))), yes());
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
        let s = SecretServiceImpl::new(Arc::new(SecretStore::new(Arc::new(Broken))), yes());
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
        let fake =
            crate::user_presence::fake::FakePresence::new(crate::user_presence::fake::Answer::Yes);
        let s = SecretServiceImpl::with_gate(
            Arc::clone(&store),
            Arc::new(Peer(Some(4242))),
            Arc::new(UserPresence::new(fake.clone())),
        );
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
        // Refused before anyone was asked.
        assert!(fake.asked().is_empty());
    }

    /// Through the service's own `Set` path: the gate refuses, and the value in
    /// the request is overwritten where it lies.
    #[tokio::test]
    async fn a_set_the_gate_refuses_has_its_value_wiped_on_the_service_path() {
        let store = Arc::new(SecretStore::new(Arc::new(InMemorySecretBackend::default())));
        let s = SecretServiceImpl::with_gate(Arc::clone(&store), Arc::new(Peer(Some(4242))), yes());
        let mut request = Request::new(SetSecretRequest {
            name: "tok".into(),
            value: TOKEN.into(),
        });
        let ptr = request.get_ref().value.as_ptr();
        let len = request.get_ref().value.len();
        let denied = s.set_in_place(&mut request).await.unwrap_err();
        assert_eq!(denied.code(), tonic::Code::PermissionDenied);
        assert!(request.get_ref().value.is_empty());
        // The buffer that held it is zeroed, not just shortened.
        let bytes = unsafe { std::slice::from_raw_parts(ptr, len) };
        assert!(bytes.iter().all(|b| *b == 0), "the value bytes were left");
        assert!(store.names().unwrap().is_empty());
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

    // -- confirmed at the screen --------------------------------------------

    use crate::user_presence::fake::{Answer, FakePresence};

    /// An ungated service (an admitted peer) that asks `fake`.
    fn asking(fake: &Arc<FakePresence>) -> (SecretServiceImpl, Arc<SecretStore>) {
        let store = Arc::new(SecretStore::new(Arc::new(InMemorySecretBackend::default())));
        (asking_over(fake, &store), store)
    }

    fn asking_over(fake: &Arc<FakePresence>, store: &Arc<SecretStore>) -> SecretServiceImpl {
        SecretServiceImpl::new(
            Arc::clone(store),
            Arc::new(UserPresence::without_quiet_time(fake.clone())),
        )
    }

    /// A change the person did not confirm (cancelled, or no screen to ask
    /// at) changes nothing, and a refused `Set` has its value wiped.
    #[tokio::test]
    async fn a_change_that_is_not_confirmed_leaves_the_store_unchanged_and_wipes_the_value() {
        let cases: [(fn() -> Answer, tonic::Code); 2] = [
            (|| Answer::No, tonic::Code::PermissionDenied),
            (|| Answer::Unavailable, tonic::Code::FailedPrecondition),
        ];
        for (answer, code) in cases {
            let fake = FakePresence::new(answer());
            let (s, store) = asking(&fake);
            store.set("tok", "ghp_ORIGINALVALUE1").unwrap();
            let mut request = Request::new(SetSecretRequest {
                name: "tok".into(),
                value: TOKEN.into(),
            });
            let ptr = request.get_ref().value.as_ptr();
            let len = request.get_ref().value.len();
            let refused = s.set_in_place(&mut request).await.unwrap_err();
            assert_eq!(refused.code(), code);
            assert!(request.get_ref().value.is_empty());
            let bytes = unsafe { std::slice::from_raw_parts(ptr, len) };
            assert!(bytes.iter().all(|b| *b == 0), "the value bytes were left");
            assert!(!refused.message().contains(TOKEN));
            // A fresh dialog (the one above may have started a cooldown).
            let s = asking_over(&FakePresence::new(answer()), &store);
            let refused = s
                .delete(Request::new(DeleteSecretRequest { name: "tok".into() }))
                .await
                .unwrap_err();
            assert_eq!(refused.code(), code);
            assert_eq!(store.names().unwrap(), ["tok"]);
            assert_eq!(
                store.get("tok").unwrap().unwrap().expose(),
                "ghp_ORIGINALVALUE1"
            );
        }
    }

    /// The dialog names the credential, and a confirmed change goes through.
    #[tokio::test]
    async fn a_confirmed_change_goes_through_and_the_dialog_names_the_credential() {
        let fake = FakePresence::new(Answer::Yes);
        let (s, store) = asking(&fake);
        set(&s, "tok", TOKEN).await.unwrap();
        assert_eq!(store.get("tok").unwrap().unwrap().expose(), TOKEN);
        let removed = s
            .delete(Request::new(DeleteSecretRequest { name: "tok".into() }))
            .await
            .unwrap()
            .into_inner();
        assert!(removed.existed);
        let reason = crate::user_presence::credential_reason("tok");
        assert_eq!(fake.asked(), [reason.clone(), reason]);
        // Listing names asks no one.
        s.list_names(Request::new(ListSecretNamesRequest {}))
            .await
            .unwrap();
        assert_eq!(fake.asked().len(), 2);
    }

    /// A request the store would refuse is refused before the person is asked.
    #[tokio::test]
    async fn a_bad_name_or_value_is_refused_before_anyone_is_asked() {
        let fake = FakePresence::new(Answer::Yes);
        let (s, store) = asking(&fake);
        let err = set(&s, "bad name", TOKEN).await.unwrap_err();
        assert_eq!(err.code(), tonic::Code::InvalidArgument);
        let err = set(&s, "ok", "").await.unwrap_err();
        assert_eq!(err.code(), tonic::Code::InvalidArgument);
        let err = s
            .delete(Request::new(DeleteSecretRequest { name: "a b".into() }))
            .await
            .unwrap_err();
        assert_eq!(err.code(), tonic::Code::InvalidArgument);
        assert!(fake.asked().is_empty());
        assert!(store.names().unwrap().is_empty());
    }

    /// A daemon that builds the credential service without installing a way
    /// to ask the person at the screen gets one that refuses every change, and
    /// says why: nothing changes a credential unasked.
    #[tokio::test]
    async fn a_service_built_without_an_installed_check_refuses_every_change() {
        let service = SecretServiceImpl::keychain();
        let refused = service.confirmed("tok").await.unwrap_err();
        assert_eq!(refused.code(), tonic::Code::FailedPrecondition);
        assert!(
            refused.message().contains("install_approval_services"),
            "{}",
            refused.message()
        );
    }

    /// A `Set` whose caller stops waiting while the dialog is open (a client
    /// that hung up drops the call) has its value wiped where it lies.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_set_dropped_while_the_dialog_is_open_has_its_value_wiped() {
        let (fake, answer) = crate::user_presence::fake::FakePresence::waiting();
        let s = SecretServiceImpl::new(
            Arc::new(SecretStore::new(Arc::new(InMemorySecretBackend::default()))),
            Arc::new(UserPresence::new(fake.clone())),
        );
        let mut request = Request::new(SetSecretRequest {
            name: "tok".into(),
            value: TOKEN.into(),
        });
        let ptr = request.get_ref().value.as_ptr();
        let len = request.get_ref().value.len();
        let cut_short = tokio::time::timeout(
            std::time::Duration::from_millis(200),
            s.set_in_place(&mut request),
        )
        .await;
        assert!(cut_short.is_err(), "the dialog was still open");
        assert_eq!(fake.asked().len(), 1);
        let bytes = unsafe { std::slice::from_raw_parts(ptr, len) };
        assert!(bytes.iter().all(|b| *b == 0), "the value bytes were left");
        answer.send(false).ok();
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
                yes(),
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
                yes(),
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
