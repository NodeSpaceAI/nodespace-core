//! gRPC service implementations exposed by `nodespaced`.
//!
//! Each module wraps a slice of `packages/core` or `packages/agent` business
//! logic and adapts it to the tonic-generated service trait.

pub mod agent_session_service;
pub mod assembly;
pub mod capture_service;
pub mod database_manager;
pub mod database_service;
pub mod embeddings_service;
pub mod import_service;
pub mod local_agent_service;
pub mod node_service;
pub mod settings_service;

pub use agent_session_service::AgentSessionHandler;
pub use assembly::{
    build_database_services, build_shared_services, DatabaseServices, SharedContext,
    SharedServices, SubtreeGateFactory,
};
pub use database_manager::{
    DatabaseEntry, DatabaseId, DatabaseListing, DatabaseManager, DatabaseStatus, Registry,
    RegistrySnapshot,
};
pub use database_service::DatabaseServiceImpl;
pub use embeddings_service::{EmbeddingReady, EmbeddingsServiceImpl};
pub use import_service::ImportServiceImpl;
pub use local_agent_service::{LocalAgentServiceImpl, SharedLocalAgent};
pub use node_service::NodeServiceImpl;
pub use settings_service::{McpConfig, SettingsServiceImpl};
