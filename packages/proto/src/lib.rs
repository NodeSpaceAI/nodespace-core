pub mod extension_names;
pub mod requires_extension;
pub mod socket;

/// Generated gRPC types for the `nodespace` proto package.
///
/// Contains client/server stubs and all request/response message types.
/// This crate has no heavy dependencies (no storage engine, no tray-icon, no tokio features).
pub mod nodespace {
    #![allow(clippy::all)]
    tonic::include_proto!("nodespace");
}

/// Maximum size, in bytes, of a single gRPC message on any NodeSpace
/// connection — applied to both directions on both ends.
///
/// tonic defaults the *decode* limit to 4 MiB, which is far below what the
/// list-shaped RPCs in this contract can legitimately produce: `NodeService`
/// has several unary RPCs (`QueryNodesSimple`, `GetRoots`, `GetChildren`,
/// `GetCollectionMembers`, `ExportMarkdown`, …) whose response carries every
/// matching node's full record in one message. A real database of a few tens
/// of megabytes already produces a response of several megabytes, so the
/// default limit turns an ordinary read into an `OutOfRange` transport error.
///
/// The transport here is a local Unix socket / Named Pipe between processes
/// owned by the same user, and the daemon already holds the same records in
/// memory to build the response — so the limit exists to bound memory, not to
/// defend a trust boundary. This value leaves roughly an order of magnitude of
/// headroom over the largest response observed from a real database while
/// still refusing a runaway allocation.
///
/// A response large enough to approach this ceiling means the RPC needs a
/// bounded/paged contract, not a larger number here.
pub const MAX_MESSAGE_SIZE_BYTES: usize = 64 * 1024 * 1024;

/// Apply [`MAX_MESSAGE_SIZE_BYTES`] to a generated tonic client or server stub.
///
/// tonic configures message-size limits per generated stub rather than on the
/// shared `Channel`/`Server`, and each stub is a distinct type with no common
/// trait — so this is a macro rather than a function. Every NodeSpace client
/// and server stub must be constructed through it; a stub that skips it silently
/// keeps tonic's 4 MiB decode default.
///
/// ```rust,ignore
/// let node = nodespace_proto::with_message_limits!(
///     NodeServiceClient::with_interceptor(channel, interceptor)
/// );
/// ```
#[macro_export]
macro_rules! with_message_limits {
    ($stub:expr) => {
        $stub
            .max_decoding_message_size($crate::MAX_MESSAGE_SIZE_BYTES)
            .max_encoding_message_size($crate::MAX_MESSAGE_SIZE_BYTES)
    };
}

/// The gRPC metadata/header key a client sets to route a request at a specific
/// registered local database (ADR-053: "One Daemon, Multiple Local Databases").
///
/// Absent → the daemon serves its default database (single-database clients are
/// unchanged); present but unregistered → the routed handler rejects the request
/// rather than silently serving the default (no cross-database leak). Defined
/// here in the wire-contract crate so every client (CLI, desktop app) and the
/// daemon reference one canonical key.
pub const DATABASE_ID_HEADER: &str = "x-ns-database-id";

/// The environment variable the CLI reads its database selection from when
/// `--database` is absent. The daemon sets it, to the database's id, in the
/// environment of an agent session it launches, so commands run inside reach
/// the database the session was launched from.
pub const DATABASE_ENV_VAR: &str = "NODESPACE_DATABASE";

/// The gRPC metadata/header key a client sets to identify itself for
/// same-origin write-echo suppression on the `WatchNodes` stream.
///
/// Every write RPC that carries this header is scoped through
/// `NodeService::with_client(id)`, so its emitted events are stamped with
/// `source_client_id = id`. `WatchNodes` reads the same header off the
/// subscribing request and drops any event whose `source_client_id` matches —
/// the daemon, not the frontend, is the authority on "is this my own echo".
///
/// Absent → writes emit no `source_client_id` and are never suppressed on any
/// stream (unchanged original behavior, preserved for any caller that omits
/// the header). Each process that opens its own
/// long-lived connection to the daemon (one desktop-app window, one CLI
/// invocation, one local-agent session) should generate its own stable id for
/// the lifetime of that connection — never share one across processes, or a
/// genuinely foreign writer's events would be suppressed too.
pub const CLIENT_ID_HEADER: &str = "x-ns-client-id";

pub use nodespace::agent_session_service_client::AgentSessionServiceClient;
pub use nodespace::agent_session_service_server::AgentSessionServiceServer;
pub use nodespace::database_service_client::DatabaseServiceClient;
pub use nodespace::database_service_server::DatabaseServiceServer;
pub use nodespace::embeddings_service_client::EmbeddingsServiceClient;
pub use nodespace::embeddings_service_server::EmbeddingsServiceServer;
pub use nodespace::import_service_client::ImportServiceClient;
pub use nodespace::import_service_server::ImportServiceServer;
pub use nodespace::local_agent_service_client::LocalAgentServiceClient;
pub use nodespace::local_agent_service_server::LocalAgentServiceServer;
pub use nodespace::node_service_client::NodeServiceClient;
pub use nodespace::node_service_server::NodeServiceServer;
pub use nodespace::{
    AgentAvailability, CheckAvailabilityRequest, CheckAvailabilityResponse, CreateDatabaseRequest,
    DatabaseInfo, DatabaseStatus, LaunchSessionRequest, LaunchSessionResponse,
    ListDatabasesRequest, ListDatabasesResponse, ListSessionsRequest, ListSessionsResponse,
    NodeData, RegisterDatabaseRequest, RemoveDatabaseRequest, RemoveDatabaseResponse,
    RenameDatabaseRequest, ReportHarnessSessionRequest, ReportHarnessSessionResponse,
    ResizeRequest, ResizeResponse, SessionInfo, SetDefaultDatabaseRequest, StreamOutputRequest,
    TerminateSessionRequest, TerminateSessionResponse, WriteInputRequest, WriteInputResponse,
};
