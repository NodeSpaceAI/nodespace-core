mod error;
pub mod events;
pub mod fractional_ordering;
mod index_manager;
pub mod schema;
mod sqlite_store;

pub use error::DatabaseError;
pub use events::{
    DomainEvent, EventEnvelope, EventMetadata, PlaybookExecutionContext, PropertyChange,
    RelationshipEvent,
};
pub use fractional_ordering::FractionalOrderCalculator;
pub use index_manager::IndexManager;
pub(crate) use sqlite_store::collection_not_root;
pub(crate) use sqlite_store::tx::Tx;
pub use sqlite_store::{
    ensure_sqlite_vec_registered, BulkNodeRow, RelationshipRecord, ResolvedEntity, SqliteStore,
    StoreChange, StoreOperation, VersionConflict,
};
