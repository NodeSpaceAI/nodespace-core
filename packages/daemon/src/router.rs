//! Shared gRPC router factory (ADR-043), one of core's daemon extension points
//! (ADR-082).
//!
//! `build_base_router` is the single source of truth for which services belong
//! in a NodeSpace daemon. `nodespaced` calls it, and any other daemon built on
//! this crate calls it and chains its own services on top. Adding a field to
//! `BaseServices` is a compile error for every caller until it supplies the
//! implementation, so no daemon silently falls behind the base set.

use nodespace_proto::with_message_limits;
use tonic::service::Routes;
use tonic::transport::Server;
use tower::Layer;

use crate::{
    AgentSessionHandler, AgentSessionServiceServer, DatabaseServiceImpl, DatabaseServiceServer,
    EmbeddingsServiceImpl, EmbeddingsServiceServer, ImportServiceImpl, ImportServiceServer,
    LocalAgentServiceImpl, LocalAgentServiceServer, NodeServiceImpl, NodeServiceServer,
};

/// All base service implementations required by a NodeSpace daemon.
///
/// Every daemon built on this crate constructs this and passes it to
/// [`build_base_router`]; extra services are added after.
pub struct BaseServices {
    pub node_service: NodeServiceImpl,
    pub agent_session: AgentSessionHandler,
    pub import: ImportServiceImpl,
    pub local_agent: LocalAgentServiceImpl,
    /// `None` only when no NLP model file exists at daemon startup.
    pub embeddings: Option<EmbeddingsServiceImpl>,
    /// Registry manager for the daemon's local databases (ADR-053). Process-global
    /// (not routed): it operates on the registry itself, not a single database.
    pub database: DatabaseServiceImpl,
}

/// Build the base tonic router with all base services registered.
///
/// Accepts a `Server<L>` (already configured with any transport layers such as
/// `TrayMetricsLayer`) so callers can inject middleware before services are
/// registered. The returned `Router` can be extended with more services:
///
/// ```rust,ignore
/// // No middleware:
/// let router = build_base_router(Server::builder(), base_services);
///
/// // With a middleware layer:
/// let router = build_base_router(
///     Server::builder().layer(TrayMetricsLayer::new(controller)),
///     base_services,
/// );
///
/// // Extra service — wrap the added stub in `with_message_limits!` too, or it
/// // keeps tonic's 4 MiB decode default while every base service does not:
/// let router = build_base_router(Server::builder(), base_services)
///     .add_service(with_message_limits!(ExtraServiceServer::new(extra)));
/// ```
pub fn build_base_router<L>(
    mut server: Server<L>,
    services: BaseServices,
) -> tonic::transport::server::Router<L>
where
    L: Layer<Routes> + Clone,
{
    let router = server
        .add_service(with_message_limits!(NodeServiceServer::new(
            services.node_service
        )))
        .add_service(with_message_limits!(AgentSessionServiceServer::new(
            services.agent_session
        )))
        .add_service(with_message_limits!(ImportServiceServer::new(
            services.import
        )))
        .add_service(with_message_limits!(LocalAgentServiceServer::new(
            services.local_agent
        )))
        .add_service(with_message_limits!(DatabaseServiceServer::new(
            services.database
        )));

    match services.embeddings {
        Some(emb) => router.add_service(with_message_limits!(EmbeddingsServiceServer::new(emb))),
        None => router,
    }
}
