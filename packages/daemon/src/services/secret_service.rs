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

/// gRPC `SecretService` backed by a [`SecretStore`].
#[derive(Clone)]
pub struct SecretServiceImpl {
    store: Arc<SecretStore>,
}

impl SecretServiceImpl {
    pub fn new(store: Arc<SecretStore>) -> Self {
        Self { store }
    }

    /// The service over the operating system's credential store.
    pub fn keychain() -> Self {
        Self::new(Arc::new(SecretStore::keychain()))
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
    }
}

#[tonic::async_trait]
impl GrpcSecretService for SecretServiceImpl {
    async fn set(
        &self,
        request: Request<SetSecretRequest>,
    ) -> Result<Response<SetSecretResponse>, Status> {
        let mut req = request.into_inner();
        // The OS store can block (a prompt, a locked keychain): keep it off the
        // async worker threads.
        let store = Arc::clone(&self.store);
        tokio::task::spawn_blocking(move || {
            let result = store.set(&req.name, &req.value);
            // The request is the daemon's last copy of the value: overwrite it.
            req.value.zeroize();
            result
        })
        .await
        .map_err(|_| Status::internal("credential store task failed"))?
        .map_err(status)?;
        Ok(Response::new(SetSecretResponse {}))
    }

    async fn delete(
        &self,
        request: Request<DeleteSecretRequest>,
    ) -> Result<Response<DeleteSecretResponse>, Status> {
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
}
