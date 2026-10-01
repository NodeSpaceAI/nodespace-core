//! Request-scoped database routing middleware (ADR-053: "One Daemon, Multiple
//! Local Databases").
//!
//! [`DbManagerLayer`] injects the process-global [`DatabaseManager`] into every
//! request's extensions so the per-database gRPC handlers can resolve the
//! `x-ns-database-id` routing header and dispatch to the right database.
//! `nodespaced`'s serve loops install the layer. A host that serves a single
//! database may leave it out: its handlers then find no manager in the
//! extensions and serve their own single-database service set, which is also
//! how directly-constructed test impls behave.
//!
//! The layer is a thin synchronous injector: it only clones an `Arc` into the
//! request extensions. The (async) resolve-and-open work happens inside the
//! handlers, where errors turn naturally into `tonic::Status`.

use std::sync::Arc;
use std::task::{Context, Poll};

use tonic::codegen::http;
use tonic::Status;
use tower::{Layer, Service};

use crate::services::database_manager::DatabaseManager;
use crate::services::DatabaseServices;

/// The metadata/header key a client sets to target a specific registered
/// database. Re-exported from the wire-contract crate so the daemon and every
/// client share one canonical key; see [`nodespace_proto::DATABASE_ID_HEADER`]
/// for the full semantics.
pub use nodespace_proto::DATABASE_ID_HEADER;

/// Resolve the database a routed request targets (ADR-053). This is the single
/// routing contract shared by every per-database service's `route` adapter;
/// most reach it through [`route_or_self`].
///
/// When the routing middleware ([`DbManagerLayer`]) injected a
/// [`DatabaseManager`], the `x-ns-database-id` header selects a registered
/// database (an unregistered id → `NOT_FOUND`) and an absent header selects the
/// default; the target's service set is opened (lazily) and returned.
///
/// Returns `Ok(None)` when no manager was injected AND no routing header is
/// present — the caller then serves its own single-database service set, which
/// is how a host without the layer, and directly-constructed test impls,
/// behave. A request that names a database on a daemon without routing
/// installed is rejected (`UNIMPLEMENTED`) rather than silently served from the
/// active database: answering with another database's data is a wrong-database
/// read the caller cannot detect.
pub(crate) async fn routed_database_services<T>(
    request: &tonic::Request<T>,
) -> Result<Option<Arc<DatabaseServices>>, Status> {
    let header = request
        .metadata()
        .get(DATABASE_ID_HEADER)
        .map(|v| v.to_str())
        .transpose()
        .map_err(|_| Status::invalid_argument("x-ns-database-id must be valid ASCII"))?;
    let Some(manager) = request.extensions().get::<Arc<DatabaseManager>>() else {
        return match header {
            Some(id) => Err(Status::unimplemented(format!(
                "this daemon does not route requests to a specific database; \
                 cannot serve the request targeting database id {id}"
            ))),
            None => Ok(None),
        };
    };
    let id = manager
        .resolve_database_id(header)
        .await
        .map_err(|e| Status::not_found(e.to_string()))?;
    let services = manager
        .get_or_open(&id)
        .await
        .map_err(|e| Status::internal(e.to_string()))?;
    Ok(Some(services))
}

/// The common `route` adapter: resolve the target database via
/// [`routed_database_services`] and return `pick`'s service from it, or a
/// clone of `this` when no routing is in play. Services whose routed handle
/// can be absent (embeddings) must not use this — falling back to `this`
/// there would silently serve another database.
pub(crate) async fn route_or_self<S: Clone, T>(
    this: &S,
    request: &tonic::Request<T>,
    pick: impl FnOnce(&DatabaseServices) -> &S,
) -> Result<S, Status> {
    Ok(match routed_database_services(request).await? {
        Some(services) => pick(&services).clone(),
        None => this.clone(),
    })
}

/// `tower::Layer` that inserts the shared [`Arc<DatabaseManager>`] into each
/// request's extensions. See the module docs for what happens when a host
/// leaves the layer out.
#[derive(Clone)]
pub struct DbManagerLayer {
    manager: Arc<DatabaseManager>,
}

impl DbManagerLayer {
    pub fn new(manager: Arc<DatabaseManager>) -> Self {
        Self { manager }
    }
}

impl<S> Layer<S> for DbManagerLayer {
    type Service = DbManagerInjector<S>;

    fn layer(&self, inner: S) -> Self::Service {
        DbManagerInjector {
            inner,
            manager: self.manager.clone(),
        }
    }
}

/// The [`Service`] produced by [`DbManagerLayer`]. Clones the manager into the
/// request extensions, then defers to the inner service unchanged.
#[derive(Clone)]
pub struct DbManagerInjector<S> {
    inner: S,
    manager: Arc<DatabaseManager>,
}

impl<S, B> Service<http::Request<B>> for DbManagerInjector<S>
where
    S: Service<http::Request<B>>,
{
    type Response = S::Response;
    type Error = S::Error;
    type Future = S::Future;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.inner.poll_ready(cx)
    }

    fn call(&mut self, mut req: http::Request<B>) -> Self::Future {
        // tonic carries http-request extensions through to the typed
        // `tonic::Request`, so handlers read this via `req.extensions()`.
        req.extensions_mut().insert(self.manager.clone());
        self.inner.call(req)
    }
}
