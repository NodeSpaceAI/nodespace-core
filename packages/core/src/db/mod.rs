pub mod core_type_shape;
mod error;
pub mod events;
pub mod fractional_ordering;
pub(crate) mod json_path;
pub mod required_extensions;
pub mod schema;
mod sqlite_store;

pub use error::DatabaseError;
pub use events::{
    DomainEvent, EventEnvelope, EventMetadata, PlaybookExecutionContext, PropertyChange,
    RelationshipEvent,
};
pub use fractional_ordering::FractionalOrderCalculator;
pub(crate) use sqlite_store::path_reaches_condition;
pub(crate) use sqlite_store::tx::Tx;
pub(crate) use sqlite_store::Placed;
pub(crate) use sqlite_store::{composite_similarity_score, cosine_similarity, NodeMove};
pub use sqlite_store::{
    ensure_sqlite_vec_registered, BulkNodeRow, ChildPlacement, KnowledgeListing, PathReach,
    RelationshipRecord, ResolvedEntity, SqliteStore, StoreChange, StoreOperation,
    TreeInvariantRule, TreeInvariantViolation, VersionConflict,
};
