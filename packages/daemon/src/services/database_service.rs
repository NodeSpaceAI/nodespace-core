//! tonic `DatabaseService` implementation — the client-facing registry manager
//! for the daemon's local databases (ADR-053: "One Daemon, Multiple Local
//! Databases").
//!
//! Unlike the per-database services (nodes, agents, import, ...), this service
//! is process-global: it operates on the [`DatabaseManager`] registry itself
//! rather than on any one open database, so it is not routed by the
//! `x-ns-database-id` header. It is the reachable path clients use to list,
//! create, register, remove, rename, and choose the default database that
//! header-less requests fall back to.

use std::path::PathBuf;
use std::sync::Arc;

use tonic::{Request, Response, Status};

use crate::nodespace::database_service_server::DatabaseService as GrpcDatabaseService;
use crate::nodespace::{
    CreateDatabaseRequest, DatabaseInfo, DatabaseStatus as ProtoDatabaseStatus,
    ListDatabasesRequest, ListDatabasesResponse, RegisterDatabaseRequest, RemoveDatabaseRequest,
    RemoveDatabaseResponse, RenameDatabaseRequest, SetDefaultDatabaseRequest,
};
use crate::services::database_manager::{
    DatabaseId, DatabaseListing, DatabaseManager, DatabaseStatus,
};

/// gRPC `DatabaseService` backed by the shared [`DatabaseManager`].
#[derive(Clone)]
pub struct DatabaseServiceImpl {
    manager: Arc<DatabaseManager>,
}

impl DatabaseServiceImpl {
    pub fn new(manager: Arc<DatabaseManager>) -> Self {
        Self { manager }
    }

    /// Build the proto `DatabaseInfo` for `id` from a fresh registry snapshot.
    ///
    /// Every mutating RPC re-reads the snapshot through this so the returned
    /// `status` and `is_default` reflect the write just applied (the manager's
    /// mutators return the bare entry, not its derived runtime status).
    async fn info_for(&self, id: &DatabaseId) -> Result<DatabaseInfo, Status> {
        let snapshot = self.manager.list().await;
        snapshot
            .databases
            .iter()
            .find(|listing| &listing.entry.id == id)
            .map(listing_to_info)
            .ok_or_else(|| {
                Status::internal(format!("database {id} missing from registry after write"))
            })
    }
}

/// Map a registry listing (entry + derived status + default flag) onto the proto
/// `DatabaseInfo` response.
fn listing_to_info(listing: &DatabaseListing) -> DatabaseInfo {
    let entry = &listing.entry;
    DatabaseInfo {
        id: entry.id.to_string(),
        name: entry.name.clone(),
        path: entry.path.display().to_string(),
        created_at: entry.created_at.to_rfc3339(),
        last_opened_at: entry.last_opened_at.map(|t| t.to_rfc3339()),
        is_default: listing.is_default,
        status: proto_status(listing.status) as i32,
        extensions: entry
            .extensions
            .iter()
            .map(|(key, value)| (key.clone(), value.clone()))
            .collect(),
        unsupported_extensions: listing.unsupported_extensions.clone(),
    }
}

/// Translate the manager's runtime status enum into the generated proto enum.
fn proto_status(status: DatabaseStatus) -> ProtoDatabaseStatus {
    match status {
        DatabaseStatus::Open => ProtoDatabaseStatus::Open,
        DatabaseStatus::Closed => ProtoDatabaseStatus::Closed,
        DatabaseStatus::Missing => ProtoDatabaseStatus::Missing,
        DatabaseStatus::RequiresExtension => ProtoDatabaseStatus::RequiresExtension,
    }
}

#[tonic::async_trait]
impl GrpcDatabaseService for DatabaseServiceImpl {
    async fn list(
        &self,
        _request: Request<ListDatabasesRequest>,
    ) -> Result<Response<ListDatabasesResponse>, Status> {
        let snapshot = self.manager.list().await;
        let databases = snapshot.databases.iter().map(listing_to_info).collect();
        let default_database_id = snapshot
            .default_database
            .map(|id| id.to_string())
            .unwrap_or_default();
        Ok(Response::new(ListDatabasesResponse {
            databases,
            default_database_id,
        }))
    }

    async fn create(
        &self,
        request: Request<CreateDatabaseRequest>,
    ) -> Result<Response<DatabaseInfo>, Status> {
        let req = request.into_inner();
        let entry = self
            .manager
            .create(req.name, req.path.map(PathBuf::from))
            .await
            .map_err(|e| Status::internal(e.to_string()))?;
        Ok(Response::new(self.info_for(&entry.id).await?))
    }

    async fn register(
        &self,
        request: Request<RegisterDatabaseRequest>,
    ) -> Result<Response<DatabaseInfo>, Status> {
        let req = request.into_inner();
        let entry = self
            .manager
            .register(PathBuf::from(req.path))
            .await
            .map_err(|e| Status::internal(e.to_string()))?;
        Ok(Response::new(self.info_for(&entry.id).await?))
    }

    async fn remove(
        &self,
        request: Request<RemoveDatabaseRequest>,
    ) -> Result<Response<RemoveDatabaseResponse>, Status> {
        let id = DatabaseId::from(request.into_inner().id);
        self.manager
            .remove(&id)
            .await
            .map_err(|e| Status::not_found(e.to_string()))?;
        Ok(Response::new(RemoveDatabaseResponse { id: id.to_string() }))
    }

    async fn set_default(
        &self,
        request: Request<SetDefaultDatabaseRequest>,
    ) -> Result<Response<DatabaseInfo>, Status> {
        let id = DatabaseId::from(request.into_inner().id);
        self.manager
            .set_default(&id)
            .await
            .map_err(|e| Status::not_found(e.to_string()))?;
        Ok(Response::new(self.info_for(&id).await?))
    }

    async fn rename(
        &self,
        request: Request<RenameDatabaseRequest>,
    ) -> Result<Response<DatabaseInfo>, Status> {
        let req = request.into_inner();
        let id = DatabaseId::from(req.id);
        self.manager
            .rename(&id, req.name)
            .await
            .map_err(|e| Status::not_found(e.to_string()))?;
        Ok(Response::new(self.info_for(&id).await?))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::services::assembly::SharedContext;
    use crate::services::database_manager::DatabaseEntry;
    use nodespace_agent::pty::PtySessionManager;
    use nodespace_core::services::EmbeddingScheduler;
    use nodespace_nlp_engine::EmbeddingService;
    use std::collections::{BTreeMap, HashMap};
    use tokio::sync::watch;

    /// A model-less build context (mirrors `database_manager::tests`): with
    /// `has_model = false` no embedding wiring runs, so the dropped watch sender
    /// is harmless.
    fn test_context() -> SharedContext {
        let (_tx, model) = watch::channel::<Option<Arc<EmbeddingService>>>(None);
        SharedContext {
            pty_manager: Arc::new(PtySessionManager::new()),
            model,
            has_model: false,
            model_load_failed: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            scheduler: Arc::new(EmbeddingScheduler::new()),
            subtree_gate_factory: Arc::new(std::sync::OnceLock::new()),
            local_agent: crate::SharedLocalAgent::new(
                crate::nodespace_dir()
                    .expect("nodespace dir")
                    .join("daemon.toml"),
            ),
            supported_extensions: Vec::new(),
        }
    }

    async fn service() -> (DatabaseServiceImpl, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let registry = dir.path().join("databases.toml");
        let manager = DatabaseManager::load(registry, test_context())
            .await
            .unwrap();
        (DatabaseServiceImpl::new(Arc::new(manager)), dir)
    }

    /// `DatabaseInfo` carries the entry's extension keys, every key and value
    /// unchanged.
    #[test]
    fn listing_to_info_carries_the_extensions_map() {
        let entry = DatabaseEntry {
            id: DatabaseId::from("01J00000000000000000000000".to_string()),
            name: "Work".into(),
            path: PathBuf::from("/tmp/work.db"),
            created_at: chrono::Utc::now(),
            last_opened_at: None,
            extensions: BTreeMap::from([
                ("plugin_state".to_string(), "keep".to_string()),
                ("plugin_label".to_string(), "blue".to_string()),
            ]),
        };

        let info = listing_to_info(&DatabaseListing {
            entry,
            status: DatabaseStatus::Closed,
            is_default: false,
            unsupported_extensions: Vec::new(),
        });

        assert_eq!(
            info.extensions,
            HashMap::from([
                ("plugin_state".to_string(), "keep".to_string()),
                ("plugin_label".to_string(), "blue".to_string()),
            ])
        );
    }

    /// A refused database reaches the wire as REQUIRES_EXTENSION with the ids
    /// it requires; any other status carries no ids.
    #[test]
    fn listing_to_info_carries_the_refusal() {
        let entry = DatabaseEntry {
            id: DatabaseId::from("01J00000000000000000000000".to_string()),
            name: "Marked".into(),
            path: PathBuf::from("/tmp/marked.db"),
            created_at: chrono::Utc::now(),
            last_opened_at: None,
            extensions: BTreeMap::new(),
        };

        let info = listing_to_info(&DatabaseListing {
            entry,
            status: DatabaseStatus::RequiresExtension,
            is_default: true,
            unsupported_extensions: vec!["fixture".to_string()],
        });

        assert_eq!(info.status, ProtoDatabaseStatus::RequiresExtension as i32);
        assert_eq!(info.unsupported_extensions, vec!["fixture".to_string()]);
    }

    #[tokio::test]
    async fn create_list_set_default_rename_remove_round_trip() {
        let (svc, dir) = service().await;

        // Create two databases; the first becomes the default automatically.
        let first = svc
            .create(Request::new(CreateDatabaseRequest {
                name: "First".into(),
                path: Some(dir.path().join("first.db").display().to_string()),
            }))
            .await
            .unwrap()
            .into_inner();
        assert_eq!(first.name, "First");
        assert!(first.is_default);
        // Create makes the file and opens it before registering, so a freshly
        // created entry reports Open — never Missing.
        assert_eq!(first.status, ProtoDatabaseStatus::Open as i32);
        assert!(dir.path().join("first.db").exists());

        let second = svc
            .create(Request::new(CreateDatabaseRequest {
                name: "Second".into(),
                path: Some(dir.path().join("second.db").display().to_string()),
            }))
            .await
            .unwrap()
            .into_inner();
        assert!(!second.is_default);

        // List reports both, with the first as default.
        let listed = svc
            .list(Request::new(ListDatabasesRequest {}))
            .await
            .unwrap()
            .into_inner();
        assert_eq!(listed.databases.len(), 2);
        assert_eq!(listed.default_database_id, first.id);

        // Switch the default to the second.
        let now_default = svc
            .set_default(Request::new(SetDefaultDatabaseRequest {
                id: second.id.clone(),
            }))
            .await
            .unwrap()
            .into_inner();
        assert!(now_default.is_default);
        let listed = svc
            .list(Request::new(ListDatabasesRequest {}))
            .await
            .unwrap()
            .into_inner();
        assert_eq!(listed.default_database_id, second.id);

        // Rename the first; the label changes, the id does not.
        let renamed = svc
            .rename(Request::new(RenameDatabaseRequest {
                id: first.id.clone(),
                name: "Renamed".into(),
            }))
            .await
            .unwrap()
            .into_inner();
        assert_eq!(renamed.id, first.id);
        assert_eq!(renamed.name, "Renamed");

        // Remove the first; it drops out of the listing, second stays default.
        let removed = svc
            .remove(Request::new(RemoveDatabaseRequest {
                id: first.id.clone(),
            }))
            .await
            .unwrap()
            .into_inner();
        assert_eq!(removed.id, first.id);
        let listed = svc
            .list(Request::new(ListDatabasesRequest {}))
            .await
            .unwrap()
            .into_inner();
        assert_eq!(listed.databases.len(), 1);
        assert_eq!(listed.databases[0].id, second.id);
    }

    #[tokio::test]
    async fn register_rejects_an_absent_file_and_accepts_an_existing_one() {
        let (svc, dir) = service().await;

        // Registering never creates files, so an absent target is rejected
        // rather than registered as permanently Missing.
        let err = svc
            .register(Request::new(RegisterDatabaseRequest {
                path: dir.path().join("not-here.db").display().to_string(),
            }))
            .await
            .unwrap_err();
        assert!(
            err.message().contains("no database file exists"),
            "unexpected error: {}",
            err.message()
        );

        // An existing file registers and reports Closed until first opened.
        let present = dir.path().join("present.db");
        std::fs::write(&present, b"").unwrap();
        let info = svc
            .register(Request::new(RegisterDatabaseRequest {
                path: present.display().to_string(),
            }))
            .await
            .unwrap()
            .into_inner();
        assert_eq!(info.status, ProtoDatabaseStatus::Closed as i32);
        assert!(info.is_default); // first registered → default
    }

    #[tokio::test]
    async fn mutations_on_unknown_id_are_not_found() {
        let (svc, _dir) = service().await;
        let unknown = || "ZZZ-NOT-REGISTERED".to_string();

        for status in [
            svc.remove(Request::new(RemoveDatabaseRequest { id: unknown() }))
                .await
                .map(|_| ())
                .unwrap_err(),
            svc.set_default(Request::new(SetDefaultDatabaseRequest { id: unknown() }))
                .await
                .map(|_| ())
                .unwrap_err(),
            svc.rename(Request::new(RenameDatabaseRequest {
                id: unknown(),
                name: "x".into(),
            }))
            .await
            .map(|_| ())
            .unwrap_err(),
        ] {
            assert_eq!(status.code(), tonic::Code::NotFound);
        }
    }
}
