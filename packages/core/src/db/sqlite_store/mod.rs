use crate::db::fractional_ordering::FractionalOrderCalculator;
use crate::models::{DeleteResult, Node, NodeQuery, NodeUpdate, OrderBy};
use anyhow::{Context, Result};
use chrono::{DateTime, NaiveDate, Utc};
use connections::{Connections, ReadConn, WriteGuard};
use serde_json::Value;
use std::collections::HashMap;
use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::broadcast;
use tx::Tx;

/// Normalise a stored date string to YYYY-MM-DD on read.
/// Accepts YYYY-MM-DD (pass-through) or RFC 3339 (extract date portion).
/// Returns the original string for unrecognised values so callers can surface them.
fn normalize_date_field(s: &str) -> String {
    if NaiveDate::parse_from_str(s, "%Y-%m-%d").is_ok() {
        return s.to_string();
    }
    if let Ok(dt) = DateTime::parse_from_rfc3339(s) {
        return dt.format("%Y-%m-%d").to_string();
    }
    if let Ok(dt) = s.parse::<DateTime<Utc>>() {
        return dt.format("%Y-%m-%d").to_string();
    }
    s.to_string()
}

const DOMAIN_EVENT_CHANNEL_CAPACITY: usize = 128;
/// KNN over-fetch factor: vec0 returns top-k *chunks*, but search results are grouped
/// by node and many chunks map to one node, so we fetch `limit * this` to cover enough
/// distinct nodes.
const EMBEDDING_KNN_OVERFETCH: i64 = 10;
const BM25_MAX_TOKENS: usize = 4;
const BM25_STOP_WORDS: &[&str] = &[
    "a", "an", "the", "is", "are", "was", "were", "be", "been", "being", "have", "has", "had",
    "do", "does", "did", "will", "would", "could", "should", "may", "might", "shall", "can",
    "need", "dare", "ought", "i", "me", "my", "we", "our", "you", "your", "he", "she", "it",
    "they", "them", "their", "what", "which", "who", "whom", "this", "that", "these", "those",
    "to", "of", "in", "on", "at", "by", "for", "with", "about", "as", "how", "when", "where",
    "why",
];

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct RelationshipRecord {
    pub id: String,
    #[serde(rename = "in")]
    pub in_node: String,
    pub out_node: String,
    pub relationship_type: String,
    #[serde(default)]
    pub properties: Value,
}

impl RelationshipRecord {
    pub fn order(&self) -> f64 {
        self.properties
            .get("order")
            .and_then(|v| v.as_f64())
            .unwrap_or(0.0)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StoreOperation {
    Created,
    Updated,
    Deleted,
}

#[derive(Debug, Clone)]
pub struct StoreChange {
    pub operation: StoreOperation,
    pub node: Node,
    pub source: Option<String>,
    pub previous_node: Option<Node>,
    pub playbook_context: Option<crate::db::events::PlaybookExecutionContext>,
}

pub type StoreNotifier = Arc<dyn Fn(StoreChange) + Send + Sync>;

/// Register the statically-linked `sqlite-vec` extension as a SQLite auto-extension so
/// every connection opened afterwards has the `vec0` virtual-table module available.
/// Runs exactly once per process and must complete before any real store connection is
/// opened — `SqliteStore::new` awaits it first. `pub` so tests/tooling that open a raw
/// libsql connection (bypassing `SqliteStore::new`) to exercise the migration runner
/// directly can register `vec0` too, since migration 1 creates a vec0 table.
///
/// Ordering is critical: libsql lazily calls `sqlite3_config(SQLITE_CONFIG_SERIALIZED)`
/// on its first `connect()` (via a process-global `Once`), and `sqlite3_config` fails
/// once SQLite has been initialized. Registering an auto-extension auto-initializes
/// SQLite, so we open (and drop) a throwaway in-memory libsql connection FIRST to run
/// libsql's config, THEN register. Doing this in a single async `OnceCell` keeps the two
/// steps atomic so concurrent `new()` calls can't interleave the warm-up and the
/// registration.
pub async fn ensure_sqlite_vec_registered() {
    /// The libsql FFI signature for a SQLite extension entry point.
    type EntryPoint = unsafe extern "C" fn(
        *mut libsql::ffi::sqlite3,
        *mut *const std::os::raw::c_char,
        *const libsql::ffi::sqlite3_api_routines,
    ) -> std::os::raw::c_int;

    static VEC_INIT: tokio::sync::OnceCell<()> = tokio::sync::OnceCell::const_new();
    VEC_INIT
        .get_or_init(|| async {
            // Warm up libsql so its one-time sqlite3_config(SERIALIZED) runs before we
            // auto-initialize SQLite via sqlite3_auto_extension.
            if let Ok(db) = libsql::Builder::new_local(":memory:").build().await {
                let _ = db.connect();
            }
            // SAFETY: the `sqlite-vec` crate declares `sqlite3_vec_init` as a zero-arg
            // `extern "C"` fn, but its real C entry point has the 3-arg SQLite signature
            // `EntryPoint`. We transmute via `*const ()` to that true signature — the same
            // pattern the crate's own rusqlite example uses — and hand it to
            // `sqlite3_auto_extension`, which expects exactly that type.
            unsafe {
                let entry: EntryPoint =
                    std::mem::transmute(sqlite_vec::sqlite3_vec_init as *const ());
                libsql::ffi::sqlite3_auto_extension(Some(entry));
            }
        })
        .await;
}

/// **Why this crate's tests run at `--test-threads=1`** (`rust:test:core` in
/// `package.json`, and `rust:test:lib`/`rust:test:long`): the bundled SQLite
/// fork this crate links (`libsql-ffi`'s vendored `SQLite3MultipleCiphers`
/// amalgamation) has a real, unsynchronized data race in its WAL-manager
/// singleton, verified by reading the vendored C source directly:
///
/// ```c
/// RefCountedWalManager *make_sqlite3_wal_manager_rc() {
///     static int initialized = 0;
///     static RefCountedWalManager manager = { 0 };
///     if (!initialized) { manager.is_static = 1; manager.ref = ...; manager.n = 1; initialized = 1; }
///     return &manager;
/// }
/// ```
///
/// `make_sqlite3_wal_manager_rc` is called from `sqlite3_open_v2` (via
/// `openDatabase`) on every connection open, and `clone_wal_manager`'s
/// refcount bump (`p->n += 1`, also unguarded) fires on every pager attach —
/// open time, and again on any later WAL-mode switch. None of this is
/// covered by SQLite's own `SQLITE_CONFIG_SERIALIZED` mutexes: it runs
/// before `sqlite3_open_v2` reaches SQLite's internal locking at all. Two
/// connections opening at genuinely the same instant on separate OS threads
/// can race that check-then-act, or torn-write the shared refcount,
/// corrupting a struct every connection in the process then shares —
/// including connections that opened cleanly and never personally raced
/// anything. The corrupted state is a process-global struct that ordinary
/// SQLite operations (not just opens) read, which matches every symptom
/// observed: `SQLITE_MISUSE` ("bad parameter or other API misuse") on a
/// `PRAGMA`, a query, or a transaction rollback, at `SqliteStore::new()`/
/// `create_test_service()` setup time and against already-established
/// connections mid-test, with no code change between a failing and passing
/// run of the identical test. No crash was ever captured under a debugger,
/// so this is the strongest available explanation given the vendored source
/// and the observed failure shapes, not a confirmed-by-backtrace mechanism.
///
/// This is vendored C we cannot patch. This crate is the only workspace
/// member with a DIRECT `libsql`/`libsql-ffi` dependency, but `libsql-ffi`
/// is also linked TRANSITIVELY into `nodespace-agent`, `nodespace-daemon`,
/// and `nodespace-cli` (`cargo tree -i libsql-ffi`): each has its own
/// non-`#[ignore]`d tests that call `SqliteStore::new()` directly, both in
/// `tests/*.rs` integration targets and, more heavily, in `#[cfg(test)]`
/// unit tests inside `nodespace-agent`'s and `nodespace-daemon`'s own
/// `--lib` targets — `nodespace-agent`'s lib alone runs 621 tests in one
/// binary, not the "tens of tests" a prior version of this comment assumed
/// made the race negligible there.
///
/// That reachability is no longer theoretical: reproduced directly, once
/// in 100 `--test-threads=16` runs of `nodespace-agent`'s lib suite (a
/// setup-time `NodeService::new()` panic — `QueryFailed("Failed to query
/// nodes")` — with no code change between that run and every surrounding
/// passing run of the identical test; the exact failure shape this crate's
/// own root-caused issue described). `nodespace-daemon`'s lib and every
/// integration target named in that investigation stayed clean across
/// ~190 further runs at the same thread count, but per the "no guarded
/// region narrower than the whole process" reasoning two paragraphs below
/// (established from this crate's own prior narrow-fix failure), a clean
/// run elsewhere is the low-probability-but-nonzero outcome the vendored
/// bug predicts at these suites' smaller sizes, not evidence those targets
/// are unexposed. All three crates' `--lib --bins --tests` targets are
/// therefore now serialized the same way as this crate:
/// `rust:test:libsql-linked` in `package.json`, excluded from
/// `rust:test:workspace`'s `--test-threads=2` run exactly as this crate
/// is. If CI or a local run ever surfaces this failure shape in
/// `nodespace-cli` specifically (the one crate of the three whose `--lib`
/// target has no direct `SqliteStore` construction, so its exposure is
/// narrower than the other two), that is corroborating evidence, not a
/// surprise — not grounds to narrow the treatment back down.
///
/// In production there is exactly one `SqliteStore` per daemon lifetime, so
/// the race needs genuine test-suite concurrency (many independent stores
/// opening across real OS threads) to trigger — confirmed empirically:
/// `--test-threads=1` was 100% clean across 50+ consecutive runs, while
/// `--test-threads=16` reproduced the failure in roughly 5-15% of runs. A
/// prior attempt at a narrower fix (a mutex around just the connection-open
/// call, in `connections.rs`) did not hold up: one clean 40-run verification
/// still failed twice, including once on an ordinary query against an
/// already-open connection nowhere near an open call — evidence the
/// corruption isn't bounded to the instant of opening, so there is no
/// principled guarded region narrower than "every operation on every
/// connection, for the life of the process" — which is `--test-threads=1`
/// implemented as a hand-rolled lock, with strictly more surface for a
/// missed call site. Serializing the whole binary is the direct fix, not a
/// workaround: see `rust:test:core` in `package.json` (and
/// `rust:test:libsql-linked` for the same treatment applied to
/// `nodespace-agent`/`nodespace-daemon`/`nodespace-cli`, above).
pub struct SqliteStore {
    /// The store's SQLite connections. Reachable only through
    /// [`SqliteStore::write`] / [`SqliteStore::read`] — the raw handles live in
    /// the `connections` submodule precisely so that the sibling modules of
    /// this one cannot reach around them. See that module's doc comment.
    conns: Connections,
    event_tx: broadcast::Sender<crate::db::events::EventEnvelope>,
    valid_node_types: HashSet<String>,
    notifier: Option<StoreNotifier>,
}

impl SqliteStore {
    pub async fn new(db_path: PathBuf) -> Result<Self> {
        // Register sqlite-vec (once per process) BEFORE opening any connection,
        // so every connection — the writer here and every reader minted later —
        // picks up the `vec0` module. See `ensure_sqlite_vec_registered`.
        ensure_sqlite_vec_registered().await;

        let conns = Connections::open(&db_path).await?;

        // Bootstrap runs through the writer guard like any other write. The
        // read pool is still empty at this point and only fills on first use,
        // so no reader connection can exist before migrations have run.
        let valid_node_types = {
            let conn = conns.write().await;
            // Data-safety for app updates: snapshot the existing database before any
            // pending migration runs, so a new release's migration can never lose the
            // user's prior data irrecoverably. Best-effort — a backup failure is logged
            // and must not block startup.
            if let Err(e) =
                crate::db::migrations::backup_before_pending_migrations(&conn, &db_path).await
            {
                tracing::warn!(error = %e, "pre-migration database backup failed; proceeding");
            }
            Self::initialize_schema(&conn).await?;
            Self::build_schema_caches(&conn).await?
        };

        let (event_tx, _) = broadcast::channel(DOMAIN_EVENT_CHANNEL_CAPACITY);

        Ok(Self {
            conns,
            event_tx,
            valid_node_types,
            notifier: None,
        })
    }

    /// Acquire exclusive use of the writer connection. See
    /// [`Connections::write`] — in particular, the guard is NOT re-entrant.
    pub(crate) async fn write(&self) -> WriteGuard<'_> {
        self.conns.write().await
    }

    /// Check out a reader connection. See [`Connections::read`].
    ///
    /// A cursor derived from this checkout owns the connection until it drops,
    /// so a store method must not call another store method while one is still
    /// live — that pins two connections where one would do. Drain the cursor
    /// into an owned value and let it drop first. See [`ReadRows`].
    pub(crate) async fn read(&self) -> Result<ReadConn> {
        self.conns.read().await
    }

    /// The reader-checkout high-water mark, for tests that assert a call path
    /// holds no more connections than it needs. See [`ReaderGauge`].
    #[cfg(test)]
    pub(crate) fn readers_in_flight(&self) -> &connections::ReaderGauge {
        self.conns.readers_in_flight()
    }

    async fn initialize_schema(conn: &libsql::Connection) -> Result<()> {
        crate::db::migrations::run(conn)
            .await
            .context("Failed to run schema migrations")?;

        Self::backfill_fts_if_stale(conn).await?;

        Ok(())
    }

    /// One-time FTS5 backfill. The external-content `node_fts` triggers
    /// only index FUTURE writes, so any node predating the FTS table (user DBs are
    /// never reset — same reason the migration runner exists) is absent from
    /// the index and never returned by `bm25_search_roots`. Rebuild the index from
    /// `node`, but ONLY when it is out of sync, so a healthy DB does not re-index
    /// its whole corpus on every startup.
    ///
    /// The staleness signal is the count of ACTUALLY-INDEXED documents, read from
    /// FTS5's `node_fts_docsize` shadow table — NOT `count(*) FROM node_fts`, which
    /// for an external-content table reads rowids from the content table (`node`)
    /// and so always equals the node count regardless of index population. When the
    /// indexed-doc count differs from `node`, rebuild. No-op on a fresh DB (both 0)
    /// and after the first rebuild.
    async fn backfill_fts_if_stale(conn: &libsql::Connection) -> Result<()> {
        let count = |sql: &'static str| async move {
            let mut r = conn.query(sql, ()).await?;
            let n: i64 = r
                .next()
                .await?
                .map(|row| row.get(0))
                .transpose()?
                .unwrap_or(0);
            Ok::<i64, anyhow::Error>(n)
        };
        let indexed = count("SELECT count(*) FROM node_fts_docsize")
            .await
            .context("Failed to count indexed FTS docs")?;
        let node_count = count("SELECT count(*) FROM node")
            .await
            .context("Failed to count node rows")?;
        if indexed != node_count {
            conn.execute("INSERT INTO node_fts(node_fts) VALUES('rebuild')", ())
                .await
                .context("Failed to backfill FTS5 index")?;
        }
        Ok(())
    }

    async fn build_schema_caches(conn: &libsql::Connection) -> Result<HashSet<String>> {
        let mut rows = conn
            .query("SELECT id FROM node WHERE node_type = 'schema'", ())
            .await
            .context("Failed to query schema nodes for cache")?;

        let mut types = HashSet::new();
        while let Some(row) = rows.next().await? {
            let id: String = row.get(0)?;
            types.insert(id);
        }
        Ok(types)
    }

    pub fn set_notifier(&mut self, notifier: StoreNotifier) {
        self.notifier = Some(notifier);
    }

    fn notify(&self, change: StoreChange) {
        if let Some(notifier) = &self.notifier {
            notifier(change);
        }
    }

    pub fn subscribe_to_events(&self) -> broadcast::Receiver<crate::db::events::EventEnvelope> {
        self.event_tx.subscribe()
    }

    fn validate_node_type(&self, node_type: &str) -> Result<()> {
        if node_type.is_empty() {
            return Err(anyhow::anyhow!("Node type cannot be empty"));
        }
        if self.valid_node_types.contains(node_type) {
            return Ok(());
        }
        // Allow schema node type always
        if node_type == "schema" {
            return Ok(());
        }
        Err(anyhow::anyhow!(
            "Invalid node type '{}'. Valid types: {:?}",
            node_type,
            self.valid_node_types
        ))
    }

    /// Reject any `lifecycle_status` outside the supported allow-list before it
    /// reaches a SQL write. The only supported values are `"active"` and
    /// `"archived"` (local deletion is a hard delete, so there is no `"deleted"`
    /// state). Guarding here — the single point every INSERT/UPDATE of the column
    /// flows through — structurally prevents an unsupported value (e.g. from a
    /// play action or API caller) from letting hidden nodes resurface in
    /// full-text and semantic search.
    fn validate_lifecycle_status(status: &str) -> Result<()> {
        if crate::models::is_valid_lifecycle_status(status) {
            return Ok(());
        }
        Err(anyhow::anyhow!(
            "Invalid lifecycle_status '{}'. Valid values: {:?}",
            status,
            crate::models::LIFECYCLE_STATUSES
        ))
    }

    pub(crate) fn add_to_schema_cache(&mut self, type_name: String) {
        self.valid_node_types.insert(type_name);
    }

    pub fn close(&self) -> Result<()> {
        Ok(())
    }

    fn row_to_node(row: &libsql::Row) -> Result<Node> {
        let id: String = row.get(0)?;
        let node_type: String = row.get(1)?;
        let content: String = row.get(2)?;
        let properties_str: String = row.get(3)?;
        let title: Option<String> = row.get(4)?;
        let lifecycle_status: String = row.get(5)?;
        let version: i64 = row.get(6)?;
        // col 7 = sync_seq (ignored)
        let created_at_str: String = row.get(8)?;
        let modified_at_str: String = row.get(9)?;

        let properties: Value =
            serde_json::from_str(&properties_str).unwrap_or(serde_json::json!({}));
        let created_at = DateTime::parse_from_rfc3339(&created_at_str)
            .with_context(|| format!("Invalid created_at timestamp: {}", created_at_str))?
            .with_timezone(&Utc);
        let modified_at = DateTime::parse_from_rfc3339(&modified_at_str)
            .with_context(|| format!("Invalid modified_at timestamp: {}", modified_at_str))?
            .with_timezone(&Utc);

        Ok(Node {
            id,
            node_type,
            content,
            version,
            created_at,
            modified_at,
            properties,
            mentions: Vec::new(),
            mentioned_in: Vec::new(),
            title,
            lifecycle_status,
        })
    }

    fn row_to_relationship(row: &libsql::Row) -> Result<RelationshipRecord> {
        let id: String = row.get(0)?;
        let in_node: String = row.get(1)?;
        let out_node: String = row.get(2)?;
        let relationship_type: String = row.get(3)?;
        let props_str: String = row.get(4)?;
        let properties: Value = serde_json::from_str(&props_str).unwrap_or(serde_json::json!({}));
        Ok(RelationshipRecord {
            id,
            in_node,
            out_node,
            relationship_type,
            properties,
        })
    }

    async fn query_nodes_from_sql(
        &self,
        sql: &str,
        params: impl libsql::params::IntoParams,
    ) -> Result<Vec<Node>> {
        let mut rows = self
            .read()
            .await?
            .query(sql, params)
            .await
            .context("Failed to query nodes")?;
        let mut nodes = Vec::new();
        while let Some(row) = rows.next().await? {
            nodes.push(Self::row_to_node(&row)?);
        }
        Ok(nodes)
    }

    /// Run a scalar `SELECT COUNT(*) ...` and return the single result column.
    /// The counting counterpart to `query_nodes_from_sql`: used wherever a
    /// caller needs a total without materializing (or transferring) full
    /// `Node` rows.
    async fn count_from_sql(
        &self,
        sql: &str,
        params: impl libsql::params::IntoParams,
    ) -> Result<i64> {
        let mut rows = self
            .read()
            .await?
            .query(sql, params)
            .await
            .context("Failed to count nodes")?;
        let row = rows
            .next()
            .await?
            .ok_or_else(|| anyhow::anyhow!("No result for count"))?;
        Ok(row.get::<i64>(0).unwrap_or(0))
    }
}

// The remaining `impl SqliteStore` methods are split by concern into these
// child modules; each is an additional `impl SqliteStore` block over the same
// struct. See ADR-053 groundwork (node CRUD / relationships / embeddings / search).
mod conflicts;
mod connections;
mod embeddings;
mod nodes;
mod relationships;
mod search;
pub(crate) mod tx;

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use tempfile::TempDir;

    async fn create_test_store() -> Result<(Arc<SqliteStore>, TempDir)> {
        use crate::services::NodeService;

        let temp_dir = TempDir::new()?;
        let db_path = temp_dir.path().join("test.db");
        let mut store_arc = Arc::new(SqliteStore::new(db_path).await?);

        let _ = NodeService::new(&mut store_arc)
            .await
            .map_err(|e| anyhow::anyhow!("Failed to initialize NodeService: {}", e))?;

        Ok((store_arc, temp_dir))
    }

    /// End-to-end data-safety wiring: opening an existing (pre-migration) database
    /// through `SqliteStore::new` snapshots it BEFORE the pending migration runs,
    /// then migrates the live database forward. Guards the app-update guarantee that
    /// a new release never loses the user's prior data.
    #[tokio::test]
    async fn new_backs_up_an_existing_db_before_migrating_then_upgrades() -> Result<()> {
        use crate::db::migrations::LATEST_VERSION;
        let temp_dir = TempDir::new()?;
        let db_path = temp_dir.path().join("existing.db");

        // Simulate a database left by a prior release: migrated to LATEST-1 with data.
        {
            crate::db::ensure_sqlite_vec_registered().await;
            let conn = libsql::Builder::new_local(&db_path)
                .build()
                .await?
                .connect()?;
            crate::db::migrations::run_up_to(&conn, LATEST_VERSION - 1).await?;
            conn.execute(
                "INSERT INTO node (id, node_type, content, created_at, modified_at) \
                 VALUES ('m', 'text', 'keep', '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z')",
                (),
            )
            .await?;
        }

        // Opening through the store must snapshot then upgrade.
        let store = SqliteStore::new(db_path.clone()).await?;

        let backups = temp_dir.path().join("backups");
        let has_backup = std::fs::read_dir(&backups)
            .map(|rd| {
                rd.filter_map(|e| e.ok())
                    .any(|e| e.file_name().to_string_lossy().ends_with(".bak"))
            })
            .unwrap_or(false);
        assert!(
            has_backup,
            "opening a pre-migration db must leave a backup snapshot"
        );

        let mut rows = store.read().await?.query("PRAGMA user_version", ()).await?;
        let version: i64 = rows.next().await?.unwrap().get(0)?;
        assert_eq!(
            version, LATEST_VERSION,
            "the live db must be migrated to LATEST"
        );
        Ok(())
    }

    #[tokio::test]
    async fn test_collection_members_recursive_includes_subcollection_members() -> Result<()> {
        // members of a SUB-collection must be returned for the parent.
        // member_of stores in_node = member/child, out_node = collection/parent;
        // add_to_collection(member, collection) creates that edge.
        let (store, _t) = create_test_store().await?;

        let parent = Node::new("collection".to_string(), "Parent".to_string(), json!({}));
        let parent_id = parent.id.clone();
        store.create_node(parent, None, None).await?;

        let sub = Node::new("collection".to_string(), "Sub".to_string(), json!({}));
        let sub_id = sub.id.clone();
        store.create_node(sub, None, None).await?;
        store
            .add_to_collection(&sub_id, &parent_id, &json!({}))
            .await?; // Sub member_of Parent

        let direct = Node::new("text".to_string(), "direct member".to_string(), json!({}));
        let direct_id = direct.id.clone();
        store.create_node(direct, None, None).await?;
        store
            .add_to_collection(&direct_id, &parent_id, &json!({}))
            .await?;

        let nested = Node::new("text".to_string(), "nested member".to_string(), json!({}));
        let nested_id = nested.id.clone();
        store.create_node(nested, None, None).await?;
        store
            .add_to_collection(&nested_id, &sub_id, &json!({}))
            .await?; // member of the SUB only

        let members = store.get_collection_members_recursive(&parent_id).await?;
        assert!(
            members.contains(&nested_id),
            "a member of a sub-collection must be returned for the parent (was silently dropped); got {members:?}"
        );
        assert!(members.contains(&direct_id), "direct member missing");
        assert!(
            members.contains(&sub_id),
            "sub-collection itself is a member of the parent"
        );
        Ok(())
    }

    #[tokio::test]
    async fn test_bulk_create_mentions_skips_dangling_links() -> Result<()> {
        // a `[[link]]` to a node that wasn't imported (or a typo) is an FK
        // violation (relationship.out_node REFERENCES node(id), foreign_keys ON).
        // The whole-batch transaction used to roll back on the first dangling link
        // — losing EVERY mention. Now dangling pairs are skipped and the valid ones
        // still land.
        let (store, _t) = create_test_store().await?;

        let a = Node::new("text".to_string(), "doc A".to_string(), json!({}));
        let a_id = a.id.clone();
        store.create_node(a, None, None).await?;

        let b = Node::new("text".to_string(), "doc B".to_string(), json!({}));
        let b_id = b.id.clone();
        store.create_node(b, None, None).await?;

        // One valid mention (A→B) interleaved with two dangling ones (targets that
        // don't exist). Pre-fix the whole batch failed; now only the valid lands.
        let created = store
            .bulk_create_mentions(&[
                (a_id.clone(), "ghost-1".to_string()),
                (a_id.clone(), b_id.clone()),
                (a_id.clone(), "ghost-2".to_string()),
            ])
            .await?;
        assert_eq!(
            created, 1,
            "the valid mention must be created despite the dangling ones"
        );

        let out = store.get_outgoing_mentions(&a_id).await?;
        assert!(out.contains(&b_id), "A→B mention must exist; got {out:?}");
        assert!(
            !out.iter().any(|t| t.starts_with("ghost-")),
            "dangling mentions must NOT have been created; got {out:?}"
        );

        // A batch that is ENTIRELY dangling is a no-op (0 created), not an error.
        let none = store
            .bulk_create_mentions(&[(a_id.clone(), "ghost-3".to_string())])
            .await?;
        assert_eq!(
            none, 0,
            "an all-dangling batch creates nothing and does not error"
        );
        Ok(())
    }

    #[tokio::test]
    async fn test_member_of_cycle_is_rejected() -> Result<()> {
        // collection hierarchy is a DAG. With `B member_of A`, adding
        // `A member_of B` would close a cycle and must be rejected.
        let (store, _t) = create_test_store().await?;

        let a = Node::new("collection".to_string(), "A".to_string(), json!({}));
        let a_id = a.id.clone();
        store.create_node(a, None, None).await?;
        let b = Node::new("collection".to_string(), "B".to_string(), json!({}));
        let b_id = b.id.clone();
        store.create_node(b, None, None).await?;

        store.add_to_collection(&b_id, &a_id, &json!({})).await?; // B member_of A

        // Adding A member_of B closes the cycle → error.
        let err = store
            .validate_no_member_of_cycle(&a_id, &b_id)
            .await
            .expect_err("A member_of B must be rejected as a cycle");
        assert!(err.to_string().contains("collection_cycle"));

        // A self-edge is a cycle.
        assert!(store
            .validate_no_member_of_cycle(&a_id, &a_id)
            .await
            .is_err());

        // A non-cyclic hierarchy edge is allowed (B member_of A already holds; a
        // fresh unrelated parent is fine).
        let c = Node::new("collection".to_string(), "C".to_string(), json!({}));
        let c_id = c.id.clone();
        store.create_node(c, None, None).await?;
        assert!(store
            .validate_no_member_of_cycle(&a_id, &c_id)
            .await
            .is_ok());
        Ok(())
    }

    #[tokio::test]
    async fn test_add_to_collection_rejects_hierarchy_cycle() -> Result<()> {
        // The raw store path is reached directly (e.g. by markdown import), so it
        // must reject a collection-membership edge that closes a hierarchy cycle on
        // its own, not only via the service path.
        let (store, _t) = create_test_store().await?;

        let a = Node::new("collection".to_string(), "A".to_string(), json!({}));
        let a_id = a.id.clone();
        store.create_node(a, None, None).await?;
        let b = Node::new("collection".to_string(), "B".to_string(), json!({}));
        let b_id = b.id.clone();
        store.create_node(b, None, None).await?;

        store.add_to_collection(&a_id, &b_id, &json!({})).await?; // A member_of B (valid)

        // B member_of A would close the cycle → rejected at the store chokepoint.
        let err = store
            .add_to_collection(&b_id, &a_id, &json!({}))
            .await
            .expect_err("a cyclic collection membership must be rejected by add_to_collection");
        assert!(err.to_string().contains("collection_cycle"), "got: {err}");

        // A content member (no member_of descendants) is unaffected by the guard.
        let root = Node::new("text".to_string(), "root".to_string(), json!({}));
        let root_id = root.id.clone();
        store.create_node(root, None, None).await?;
        assert!(store
            .add_to_collection(&root_id, &a_id, &json!({}))
            .await?
            .is_some());
        Ok(())
    }

    #[tokio::test]
    async fn test_root_only_content_membership_is_enforced() -> Result<()> {
        // ADR-059 §2: a content node may be `member_of` a collection only when it
        // is a root node (no `has_child` parent). Enforced in `add_to_collection`,
        // so every single-add path is gated — including the CLI (which routes
        // through `ops::collection_ops` → `add_to_collection`).
        let (store, _t) = create_test_store().await?;

        let coll = Node::new("collection".to_string(), "Coll".to_string(), json!({}));
        let coll_id = coll.id.clone();
        store.create_node(coll, None, None).await?;

        // A ROOT content node can be filed. (criterion: root content succeeds)
        let root_text = Node::new("text".to_string(), "root doc".to_string(), json!({}));
        let root_id = root_text.id.clone();
        store.create_node(root_text, None, None).await?;
        assert!(
            store
                .add_to_collection(&root_id, &coll_id, &json!({}))
                .await?
                .is_some(),
            "a root content node must be fileable into a collection"
        );

        // An INTERIOR content node (has a `has_child` parent) is REJECTED. This is
        // the CLI-path regression: filing an interior node into a (restricted or
        // any) collection is refused with an actionable, node-naming error.
        let interior = store
            .create_child_node_atomic(&root_id, "text", "an interior child", json!({}), None)
            .await?;
        let err = store
            .add_to_collection(&interior.id, &coll_id, &json!({}))
            .await
            .expect_err("an interior content node must not be fileable into a collection");
        let msg = err.to_string();
        assert!(
            msg.contains("member_of_not_root") && msg.contains(&interior.id),
            "rejection must name the node and why it was refused; got: {msg}"
        );

        // The GENERIC relationship path is gated too. A `member_of` edge created
        // with an explicit `order` — the CLI `relationship create --edge-data`,
        // the play `add_relationship` action, and
        // `NodeService::create_relationship`'s non-auto-order fork — routes through
        // `create_generic_relationship`, which must reject an interior node just as
        // `add_to_collection` does. (Regression for the bypass where adding one
        // `order` JSON key skipped the guard.)
        let err_generic = store
            .create_generic_relationship(
                &interior.id,
                &coll_id,
                "member_of",
                &json!({"order": 5.0}),
            )
            .await
            .expect_err("member_of via the generic path must reject an interior node");
        assert!(
            err_generic.to_string().contains("member_of_not_root"),
            "generic-path rejection must carry the same reason; got: {err_generic}"
        );
        // A non-member_of generic edge is unaffected by the rule.
        assert!(
            store
                .create_generic_relationship(&interior.id, &root_id, "mentions", &json!({}))
                .await
                .is_ok(),
            "the root-only rule must not touch non-member_of generic edges"
        );

        // Person-node membership is EXEMPT (grantee membership, ADR-037 §4) — even
        // when the person node is interior.
        let interior_person = store
            .create_child_node_atomic(&root_id, "person", "Ada", json!({}), None)
            .await?;
        assert!(
            store
                .add_to_collection(&interior_person.id, &coll_id, &json!({}))
                .await
                .is_ok(),
            "person-node member_of edges are exempt from the root-only rule"
        );

        // Collection-to-collection nesting is EXEMPT — even an interior collection.
        let interior_coll = store
            .create_child_node_atomic(&root_id, "collection", "Nested", json!({}), None)
            .await?;
        assert!(
            store
                .add_to_collection(&interior_coll.id, &coll_id, &json!({}))
                .await
                .is_ok(),
            "collection nesting is exempt from the root-only rule"
        );

        // End-to-end: a restricted task inside an OPEN project still works. The
        // project ROOT is filed into the open collection; the task lives under it
        // as an interior node and carries NO membership of its own — its access
        // rides its root. Creating it must succeed (it is not a membership write).
        let project = Node::new("project".to_string(), "Open Project".to_string(), json!({}));
        let project_id = project.id.clone();
        store.create_node(project, None, None).await?;
        store
            .add_to_collection(&project_id, &coll_id, &json!({}))
            .await?; // project root filed
        let task = store
            .create_child_node_atomic(&project_id, "task", "a restricted task", json!({}), None)
            .await?;
        assert!(
            store.get_node_memberships(&task.id).await?.is_empty(),
            "an interior task under a filed project holds no membership of its own"
        );
        Ok(())
    }

    #[tokio::test]
    async fn test_bulk_root_only_membership_is_enforced() -> Result<()> {
        // The bulk path (`bulk_add_to_collections`) is what the sync-apply
        // cold-sweep calls directly, so the same rule must gate incoming edges.
        let (store, _t) = create_test_store().await?;

        let coll = Node::new("collection".to_string(), "Coll".to_string(), json!({}));
        let coll_id = coll.id.clone();
        store.create_node(coll, None, None).await?;

        let r1 = Node::new("text".to_string(), "root 1".to_string(), json!({}));
        let r1_id = r1.id.clone();
        store.create_node(r1, None, None).await?;
        let r2 = Node::new("text".to_string(), "root 2".to_string(), json!({}));
        let r2_id = r2.id.clone();
        store.create_node(r2, None, None).await?;

        // An all-root batch applies cleanly.
        let created = store
            .bulk_add_to_collections(&[
                (r1_id.clone(), coll_id.clone()),
                (r2_id.clone(), coll_id.clone()),
            ])
            .await?;
        assert_eq!(created.len(), 2, "both root memberships must be created");

        // A batch containing ONE interior node is refused (cold-sweep parity): an
        // incoming non-root membership edge cannot slip in via the bulk path.
        let interior = store
            .create_child_node_atomic(&r1_id, "text", "interior child", json!({}), None)
            .await?;
        let r3 = Node::new("text".to_string(), "root 3".to_string(), json!({}));
        let r3_id = r3.id.clone();
        store.create_node(r3, None, None).await?;
        let err = store
            .bulk_add_to_collections(&[
                (r3_id.clone(), coll_id.clone()),
                (interior.id.clone(), coll_id.clone()),
            ])
            .await
            .expect_err("a bulk batch with an interior node must be refused");
        assert!(
            err.to_string().contains(&interior.id),
            "the rejection must name the offending interior node; got: {err}"
        );
        Ok(())
    }

    #[tokio::test]
    async fn test_bulk_add_skips_collection_hierarchy_cycle() -> Result<()> {
        // The bulk path is the sync-apply cold-sweep's direct entry point, and a
        // cyclic collection pair (concurrently written by two devices) can land
        // there. It must skip the cycle-forming edge — never write a collection as a
        // descendant of itself — while still applying the batch's valid edges.
        let (store, _t) = create_test_store().await?;

        let mk_coll = |name: &str| Node::new("collection".to_string(), name.to_string(), json!({}));

        // Cross-batch: A is already a member of B (so A is a descendant of B).
        let a = mk_coll("A");
        let a_id = a.id.clone();
        store.create_node(a, None, None).await?;
        let b = mk_coll("B");
        let b_id = b.id.clone();
        store.create_node(b, None, None).await?;
        let first = store
            .bulk_add_to_collections(&[(a_id.clone(), b_id.clone())])
            .await?;
        assert_eq!(first.len(), 1, "A member_of B is a valid first edge");

        // A later batch carries the cycle-closing B member_of A plus a valid edge.
        let root = Node::new("text".to_string(), "root".to_string(), json!({}));
        let root_id = root.id.clone();
        store.create_node(root, None, None).await?;
        let created = store
            .bulk_add_to_collections(&[
                (b_id.clone(), a_id.clone()), // cycle: B would become a descendant of itself
                (root_id.clone(), a_id.clone()),
            ])
            .await?;
        assert_eq!(
            created.len(),
            1,
            "only the valid root membership lands; the cyclic edge is skipped"
        );
        assert!(
            created.iter().all(|(_, member, _, _)| member != &b_id),
            "the cyclic B member_of A edge must not be created"
        );

        // Intra-batch: both opposing edges arrive in ONE batch. The cycle check runs
        // against the transaction, so the second edge sees the first and is skipped.
        let c = mk_coll("C");
        let c_id = c.id.clone();
        store.create_node(c, None, None).await?;
        let d = mk_coll("D");
        let d_id = d.id.clone();
        store.create_node(d, None, None).await?;
        let both = store
            .bulk_add_to_collections(&[(c_id.clone(), d_id.clone()), (d_id.clone(), c_id.clone())])
            .await?;
        assert_eq!(
            both.len(),
            1,
            "one direction lands; the same-batch reverse edge is skipped as a cycle"
        );
        Ok(())
    }

    #[tokio::test]
    async fn test_reparent_guard_rejects_moving_a_member_under_a_parent() -> Result<()> {
        // ADR-059 §2 (reparent side): the store's `move_node` chokepoint rejects
        // giving a `has_child` parent to a node that holds a `member_of` edge, so
        // every reparent path (service move_node, upsert_node_with_parent, ...) is
        // covered. Non-members move freely; move-to-root is allowed; collection and
        // person nodes are exempt.
        let (store, _t) = create_test_store().await?;

        let coll = Node::new("collection".to_string(), "Coll".to_string(), json!({}));
        let coll_id = coll.id.clone();
        store.create_node(coll, None, None).await?;

        let parent = Node::new("text".to_string(), "parent".to_string(), json!({}));
        let parent_id = parent.id.clone();
        store.create_node(parent, None, None).await?;

        // A root member cannot be moved under a parent.
        let member = Node::new("text".to_string(), "root member".to_string(), json!({}));
        let member_id = member.id.clone();
        store.create_node(member, None, None).await?;
        store
            .add_to_collection(&member_id, &coll_id, &json!({}))
            .await?;
        let err = store
            .move_node(&member_id, Some(&parent_id), None)
            .await
            .expect_err("store must reject reparenting a collection member");
        assert!(
            err.to_string().contains("member_of_not_root") && err.to_string().contains(&coll_id),
            "rejection must name the reason and the collection; got: {err}"
        );

        // Moving the same member to root is allowed (the guard only fires on gaining a parent).
        assert!(
            store.move_node(&member_id, None, None).await.is_ok(),
            "moving a member to root must be allowed"
        );

        // A non-member node moves under a parent freely.
        let plain = Node::new("text".to_string(), "plain".to_string(), json!({}));
        let plain_id = plain.id.clone();
        store.create_node(plain, None, None).await?;
        assert!(
            store
                .move_node(&plain_id, Some(&parent_id), None)
                .await
                .is_ok(),
            "a non-member node must move under a parent freely"
        );

        // Person-node membership is exempt: an interior person member is allowed.
        let person = Node::new("person".to_string(), "Ada".to_string(), json!({}));
        let person_id = person.id.clone();
        store.create_node(person, None, None).await?;
        store
            .add_to_collection(&person_id, &coll_id, &json!({}))
            .await?;
        assert!(
            store
                .move_node(&person_id, Some(&parent_id), None)
                .await
                .is_ok(),
            "person-node membership is exempt from the reparent rule"
        );

        // The sync cold-sweep bulk attach path (`bulk_create_has_child`) is gated
        // too, symmetric with the forward `bulk_add_to_collections` guard: a batch
        // that would give a parent to a root member is rejected.
        let bulk_member = Node::new("text".to_string(), "bulk member".to_string(), json!({}));
        let bulk_member_id = bulk_member.id.clone();
        store.create_node(bulk_member, None, None).await?;
        store
            .add_to_collection(&bulk_member_id, &coll_id, &json!({}))
            .await?;
        let plain2 = Node::new("text".to_string(), "plain2".to_string(), json!({}));
        let plain2_id = plain2.id.clone();
        store.create_node(plain2, None, None).await?;
        let bulk_err = store
            .bulk_create_has_child(&[
                (parent_id.clone(), plain2_id.clone(), 1.0),
                (parent_id.clone(), bulk_member_id.clone(), 2.0),
            ])
            .await
            .expect_err("bulk_create_has_child must reject attaching a root member to a parent");
        assert!(
            bulk_err.to_string().contains("member_of_not_root"),
            "bulk cold-sweep reparent must be gated; got: {bulk_err}"
        );
        Ok(())
    }

    #[tokio::test]
    async fn test_backfill_fts_reindexes_unindexed_nodes() -> Result<()> {
        // a node present in `node` but missing from `node_fts` (the pre-FTS
        // corpus) must be re-indexed by the one-time backfill.
        let (store, _t) = create_test_store().await?;

        let node = Node::new(
            "text".to_string(),
            "alpha uniquetoken9173".to_string(),
            json!({}),
        );
        let nid = node.id.clone();
        store.create_node(node, None, None).await?;

        // Simulate a pre-FTS node: drop it from the external-content index.
        let rowid: i64 = {
            let mut r = store
                .read()
                .await?
                .query(
                    "SELECT rowid FROM node WHERE id = ?1",
                    libsql::params![nid.clone()],
                )
                .await?;
            r.next().await?.unwrap().get(0)?
        };
        store
            .write()
            .await
            .execute(
                "INSERT INTO node_fts(node_fts, rowid, id, content) VALUES('delete', ?1, ?2, ?3)",
                libsql::params![rowid, nid.clone(), "alpha uniquetoken9173"],
            )
            .await?;

        let matches = |store: Arc<SqliteStore>| async move {
            let mut r = store
                .read()
                .await?
                .query(
                    "SELECT count(*) FROM node_fts WHERE node_fts MATCH 'uniquetoken9173'",
                    (),
                )
                .await?;
            Ok::<i64, anyhow::Error>(r.next().await?.unwrap().get(0)?)
        };
        assert_eq!(
            matches(store.clone()).await?,
            0,
            "node should be missing from the index"
        );

        // Bind the guard rather than writing `&store.write().await.clone()`:
        // in that form the temporary guard lives to the end of the statement,
        // i.e. across the whole `backfill_fts_if_stale` await, so the idiom
        // would self-deadlock if it were ever copied somewhere that re-enters
        // `write()`. Binding makes the hold explicit and its scope obvious.
        let db = store.write().await;
        SqliteStore::backfill_fts_if_stale(&db).await?;
        drop(db);

        assert_eq!(
            matches(store.clone()).await?,
            1,
            "backfill should re-index the node"
        );
        Ok(())
    }

    #[tokio::test]
    async fn test_append_order_uses_negative_sibling_not_default() -> Result<()> {
        // appending after a sibling whose order is <= 0 must compute from
        // that real max, not fall back to the first-item default. A sibling at
        // -1.0 (a legitimate prepend result) → next order 0.0 (= -1.0 + 1.0), NOT
        // the buggy 1.0 the `last_order > 0.0` sentinel produced.
        let (store, _t) = create_test_store().await?;

        let coll = Node::new("collection".to_string(), "C".to_string(), json!({}));
        let cid = coll.id.clone();
        store.create_node(coll, None, None).await?;
        let m = Node::new("text".to_string(), "m".to_string(), json!({}));
        let mid = m.id.clone();
        store.create_node(m, None, None).await?;
        store.add_to_collection(&mid, &cid, &json!({})).await?; // member_of: in_node=member, out_node=collection

        store
            .write()
            .await
            .execute(
                "UPDATE relationship SET properties = json_set(properties, '$.order', -1.0) \
                 WHERE in_node = ?1 AND out_node = ?2 AND relationship_type = 'member_of'",
                libsql::params![mid.clone(), cid.clone()],
            )
            .await?;

        let next = store.get_next_member_order(&cid).await?;
        assert_eq!(
            next, 0.0,
            "append after a sibling at -1.0 should be 0.0, not the 1.0 first-item default"
        );
        Ok(())
    }

    #[tokio::test]
    async fn test_persisted_version_distinguishes_missing_from_present() -> Result<()> {
        // the primitive that disambiguates a no-op version-checked update —
        // a real row reports Some(version); a missing row (incl. a date-format id
        // that get_node would virtualize) reports None, NOT a phantom version.
        let (store, _t) = create_test_store().await?;

        let node = Node::new("text".to_string(), "x".to_string(), json!({}));
        let nid = node.id.clone();
        let created = store.create_node(node, None, None).await?;
        assert_eq!(store.persisted_version(&nid).await?, Some(created.version));

        assert_eq!(store.persisted_version("does-not-exist").await?, None);
        // A date-format id with no real row must report None (no virtual v1).
        assert_eq!(store.persisted_version("2026-01-01").await?, None);
        Ok(())
    }

    #[tokio::test]
    async fn test_bulk_delete_removes_all_in_one_transaction() -> Result<()> {
        // bulk_delete deletes every existing target (all-or-nothing) and
        // returns the deleted nodes; a missing id is simply skipped.
        let (store, _t) = create_test_store().await?;

        let mut ids = Vec::new();
        for i in 0..3 {
            let n = Node::new("text".to_string(), format!("n{i}"), json!({}));
            ids.push(n.id.clone());
            store.create_node(n, None, None).await?;
        }
        ids.push("never-existed".to_string());

        let deleted = store.bulk_delete(&ids, None).await?;
        assert_eq!(
            deleted.len(),
            3,
            "only the 3 existing nodes are reported deleted"
        );

        for id in ids.iter().take(3) {
            assert!(
                store.get_node(id).await?.is_none(),
                "node {id} should be gone"
            );
        }
        Ok(())
    }

    /// `bulk_delete`'s `DELETE FROM node WHERE id IN (...)` used to build one
    /// unchunked placeholder list sized to the whole input — the same
    /// unchunked-past-SQLite's-32766-ceiling defect fixed elsewhere in this
    /// module (see `nodes::large_subtree_chunking_tests`), just missed in
    /// that sweep because `get_nodes_by_ids` a few lines above it in the same
    /// function was already correctly chunked and easy to mistake for
    /// covering the whole function. 1,000 ids (> the production `ID_CHUNK` =
    /// 900, so the DELETE spans 2 chunks: 900 + 100) is enough to exercise
    /// the chunk-loop without paying the 30k-row FTS5 cost the full
    /// ceiling-proving tests need — seeded via a single multi-row INSERT
    /// rather than 1,000 individual `create_node` round trips, so this stays
    /// fast enough to run unconditionally.
    #[tokio::test]
    async fn test_bulk_delete_spans_multiple_chunks() -> Result<()> {
        let (store, _t) = create_test_store().await?;

        let ids: Vec<String> = (0..1_000)
            .map(|_| uuid::Uuid::new_v4().to_string())
            .collect();
        let now = Utc::now().to_rfc3339();
        let placeholders: Vec<String> = (1..=ids.len())
            .map(|i| format!("(?{i}, 'text', '', '{{}}', NULL, 'active', 1, '{now}', '{now}')"))
            .collect();
        let sql = format!(
            "INSERT INTO node (id, node_type, content, properties, title, lifecycle_status, version, created_at, modified_at) VALUES {}",
            placeholders.join(", ")
        );
        let params: Vec<libsql::Value> = ids
            .iter()
            .map(|id| libsql::Value::Text(id.clone()))
            .collect();
        store.write().await.execute(&sql, params).await?;

        let deleted = store.bulk_delete(&ids, None).await?;
        assert_eq!(deleted.len(), 1_000, "every seeded node reported deleted");

        for id in &ids {
            assert!(
                !store.node_exists(id).await?,
                "node {id} survived the delete — a chunk's DELETE was skipped"
            );
        }
        Ok(())
    }

    #[tokio::test]
    async fn test_create_and_get_node() -> Result<()> {
        let (store, _temp_dir) = create_test_store().await?;

        let node = Node::new("text".to_string(), "Test content".to_string(), json!({}));
        let created = store.create_node(node.clone(), None, None).await?;
        assert_eq!(created.id, node.id);
        assert_eq!(created.content, "Test content");

        let fetched = store.get_node(&node.id).await?;
        assert!(fetched.is_some());
        assert_eq!(fetched.unwrap().id, node.id);

        Ok(())
    }

    #[tokio::test]
    async fn test_sqlite_file_format() -> Result<()> {
        // Phase 1 acceptance criterion: verify the file is a valid SQLite file
        let temp_dir = TempDir::new()?;
        let db_path = temp_dir.path().join("test_node.db");

        let store = SqliteStore::new(db_path.clone()).await?;

        let node = Node::new("text".to_string(), "Hello SQLite".to_string(), json!({}));
        store.create_node(node, None, None).await?;

        // Verify file exists and starts with SQLite magic bytes
        let file_bytes = std::fs::read(&db_path)?;
        assert!(file_bytes.len() > 16, "DB file too small");
        assert_eq!(
            &file_bytes[0..16],
            b"SQLite format 3\0",
            "Not a valid SQLite file"
        );

        Ok(())
    }

    // ---- sqlite-vec ----

    /// One 768-dim embedding whose only nonzero component is `axis` (a unit vector).
    /// Two such vectors are identical iff they share an axis (cosine sim 1.0) and
    /// orthogonal otherwise (cosine sim 0.0) — handy for deterministic KNN assertions.
    fn unit_embedding(node_id: &str, axis: usize) -> crate::models::NewEmbedding {
        let mut vector = vec![0.0f32; 768];
        vector[axis] = 1.0;
        crate::models::NewEmbedding {
            node_id: node_id.to_string(),
            vector,
            model_name: Some("test-model".to_string()),
            chunk_index: 0,
            chunk_start: 0,
            chunk_end: 100,
            total_chunks: 1,
            content_hash: format!("hash-{axis}"),
            token_count: 10,
        }
    }

    fn unit_query(axis: usize) -> Vec<f32> {
        let mut v = vec![0.0f32; 768];
        v[axis] = 1.0;
        v
    }

    async fn vec_row_count(store: &SqliteStore) -> Result<i64> {
        let mut rows = store
            .read()
            .await?
            .query("SELECT COUNT(*) FROM vec_embeddings", ())
            .await?;
        Ok(rows.next().await?.unwrap().get(0)?)
    }

    #[tokio::test]
    async fn test_get_embeddings_roundtrip_and_modified_since() -> Result<()> {
        // read-API: vectors must round-trip out of the le-f32 blob, both
        // chunks come back in order, and the modified-since cursor filters.
        let (store, _tmp) = create_test_store().await?;
        let node = store
            .create_node(
                Node::new("text".to_string(), "vec node".to_string(), json!({})),
                None,
                None,
            )
            .await?;

        let mut e0 = unit_embedding(&node.id, 0);
        e0.total_chunks = 2;
        let mut e1 = unit_embedding(&node.id, 5);
        e1.chunk_index = 1;
        e1.total_chunks = 2;
        store.upsert_embeddings(&node.id, vec![e0, e1]).await?;

        let got = store.get_embeddings(&node.id).await?;
        assert_eq!(got.len(), 2, "both chunks returned");
        assert_eq!(got[0].node, node.id);
        assert_eq!(got[0].chunk_index, 0);
        assert_eq!(got[0].vector.len(), 768);
        assert_eq!(got[0].vector[0], 1.0, "axis-0 unit vector round-trips");
        assert!(!got[0].stale, "freshly upserted vectors are not stale");
        assert_eq!(got[1].chunk_index, 1);
        assert_eq!(got[1].vector[5], 1.0, "axis-5 unit vector round-trips");

        // Cursor: epoch returns everything (local-origin), a far-future cursor none.
        let epoch = DateTime::<Utc>::from_timestamp(0, 0).unwrap();
        assert_eq!(store.embeddings_modified_since(epoch).await?.len(), 2);
        let future = Utc::now() + chrono::Duration::days(1);
        assert!(store.embeddings_modified_since(future).await?.is_empty());

        // Provenance: a node's REMOTE (pulled) embedding must NOT show
        // up in the push sweep, so a received vector is never re-pushed.
        let other = store
            .create_node(
                Node::new("text".to_string(), "remote node".to_string(), json!({})),
                None,
                None,
            )
            .await?;
        store
            .apply_remote_embeddings(&other.id, vec![unit_embedding(&other.id, 7)])
            .await?;
        // get_embeddings still returns it locally (it IS stored)...
        assert_eq!(store.get_embeddings(&other.id).await?.len(), 1);
        // ...but the push sweep only sees the 2 local-origin chunks, not the remote one.
        assert_eq!(
            store.embeddings_modified_since(epoch).await?.len(),
            2,
            "remote-origin embeddings are excluded from the push sweep"
        );

        // The recurring sweep must ride idx_emb_modified, not full-scan + filesort
        // (review concern): assert the query plan uses the index and the
        // ORDER BY is index-covered.
        let mut plan = store
            .read()
            .await?
            .query(
                "EXPLAIN QUERY PLAN SELECT id FROM embedding WHERE origin = 'local' AND modified_at >= ?1 \
                 ORDER BY modified_at, node_id, chunk_index",
                libsql::params!["1970-01-01T00:00:00+00:00".to_string()],
            )
            .await?;
        let mut detail = String::new();
        while let Some(row) = plan.next().await? {
            let d: String = row.get(3)?; // EXPLAIN QUERY PLAN: (id, parent, notused, detail)
            detail.push_str(&d);
            detail.push(' ');
        }
        assert!(
            detail.contains("idx_emb_modified"),
            "sweep must use idx_emb_modified; plan was: {detail}"
        );
        assert!(
            !detail.to_uppercase().contains("TEMP B-TREE"),
            "ORDER BY must be index-covered (no filesort); plan was: {detail}"
        );

        // A node with no embeddings reads back empty (not an error).
        let other = store
            .create_node(
                Node::new("text".to_string(), "no emb".to_string(), json!({})),
                None,
                None,
            )
            .await?;
        assert!(store.get_embeddings(&other.id).await?.is_empty());
        Ok(())
    }

    #[tokio::test]
    async fn test_search_embeddings_self_query() -> Result<()> {
        let (store, _tmp) = create_test_store().await?;
        let node = store
            .create_node(
                Node::new("text".to_string(), "vec node".to_string(), json!({})),
                None,
                None,
            )
            .await?;

        store
            .upsert_embeddings(&node.id, vec![unit_embedding(&node.id, 0)])
            .await?;

        let results = store
            .search_embeddings(&unit_query(0), 10, Some(0.5))
            .await?;

        assert!(!results.is_empty(), "self-query should match");
        assert_eq!(results[0].node_id, node.id);
        assert!(
            (results[0].max_similarity - 1.0).abs() < 1e-3,
            "self-similarity should be ~1.0, got {}",
            results[0].max_similarity
        );
        Ok(())
    }

    #[tokio::test]
    async fn test_delete_embeddings_clears_vec0() -> Result<()> {
        let (store, _tmp) = create_test_store().await?;
        let node = store
            .create_node(
                Node::new("text".to_string(), "vec node".to_string(), json!({})),
                None,
                None,
            )
            .await?;

        store
            .upsert_embeddings(&node.id, vec![unit_embedding(&node.id, 0)])
            .await?;
        assert_eq!(vec_row_count(&store).await?, 1);
        assert!(!store
            .search_embeddings(&unit_query(0), 10, Some(0.5))
            .await?
            .is_empty());

        store.delete_embeddings(&node.id).await?;

        assert_eq!(vec_row_count(&store).await?, 0, "vec0 should be cleared");
        assert!(
            store
                .search_embeddings(&unit_query(0), 10, Some(0.5))
                .await?
                .is_empty(),
            "search should return nothing after delete"
        );
        Ok(())
    }

    #[tokio::test]
    async fn test_reupsert_replaces_vec0() -> Result<()> {
        let (store, _tmp) = create_test_store().await?;
        let node = store
            .create_node(
                Node::new("text".to_string(), "vec node".to_string(), json!({})),
                None,
                None,
            )
            .await?;

        // First embed on axis 0, then re-embed on axis 1.
        store
            .upsert_embeddings(&node.id, vec![unit_embedding(&node.id, 0)])
            .await?;
        store
            .upsert_embeddings(&node.id, vec![unit_embedding(&node.id, 1)])
            .await?;

        assert_eq!(vec_row_count(&store).await?, 1, "no stale duplicate rows");

        // Query by the NEW axis ranks the node highly...
        let by_new = store
            .search_embeddings(&unit_query(1), 10, Some(0.5))
            .await?;
        assert_eq!(by_new[0].node_id, node.id);
        assert!((by_new[0].max_similarity - 1.0).abs() < 1e-3);

        // ...and the OLD axis no longer matches (orthogonal → sim 0).
        let by_old = store
            .search_embeddings(&unit_query(0), 10, Some(0.5))
            .await?;
        assert!(
            by_old.iter().all(|r| r.node_id != node.id),
            "old vector should no longer match"
        );
        Ok(())
    }

    #[tokio::test]
    async fn test_upsert_embeddings_batches_across_chunk_boundary() -> Result<()> {
        // embedding/vec_embeddings inserts are now batched into multi-row
        // statements chunked under SQLite's bound-parameter ceiling. Exercise a
        // chunk count large enough to span more than one batch statement.
        let (store, _tmp) = create_test_store().await?;
        let node = store
            .create_node(
                Node::new("text".to_string(), "many chunks".to_string(), json!({})),
                None,
                None,
            )
            .await?;

        let n = 130; // > EMBEDDING_CHUNK (60) and > VEC_CHUNK's per-call fraction
        let embeddings: Vec<_> = (0..n)
            .map(|i| {
                let mut e = unit_embedding(&node.id, i % 768);
                e.chunk_index = i as i32;
                e.total_chunks = n as i32;
                e
            })
            .collect();
        store.upsert_embeddings(&node.id, embeddings).await?;

        let got = store.get_embeddings(&node.id).await?;
        assert_eq!(got.len(), n, "all chunks across batch boundaries persisted");
        assert_eq!(vec_row_count(&store).await?, n as i64);
        for (i, e) in got.iter().enumerate() {
            assert_eq!(e.chunk_index, i as i32, "chunk order preserved");
        }
        Ok(())
    }

    #[tokio::test]
    async fn test_create_stale_embedding_markers_bulk_across_chunk_boundary() -> Result<()> {
        // marker inserts are now batched multi-row statements. Exercise a
        // node count large enough to span more than one batch (ID_CHUNK = 180),
        // plus a duplicate node_id to confirm INSERT OR IGNORE still dedupes via
        // idx_emb_unique(node_id, model_name, chunk_index).
        let (store, _tmp) = create_test_store().await?;
        let mut node_ids = Vec::new();
        for i in 0..200 {
            let node = store
                .create_node(
                    Node::new("text".to_string(), format!("root {i}"), json!({})),
                    None,
                    None,
                )
                .await?;
            node_ids.push(node.id);
        }
        // Duplicate the first id to verify OR IGNORE dedupes rather than erroring.
        node_ids.push(node_ids[0].clone());

        let created = store.create_stale_embedding_markers_bulk(&node_ids).await?;
        assert_eq!(created, node_ids.len());

        let mut rows = store
            .read()
            .await?
            .query("SELECT COUNT(*) FROM embedding WHERE stale = 1", ())
            .await?;
        let count: i64 = rows.next().await?.unwrap().get(0)?;
        assert_eq!(
            count, 200,
            "one marker per distinct node, duplicate ignored by unique index"
        );
        Ok(())
    }

    #[tokio::test]
    async fn test_mark_stale_excludes_from_search() -> Result<()> {
        let (store, _tmp) = create_test_store().await?;
        let node = store
            .create_node(
                Node::new("text".to_string(), "vec node".to_string(), json!({})),
                None,
                None,
            )
            .await?;

        store
            .upsert_embeddings(&node.id, vec![unit_embedding(&node.id, 0)])
            .await?;
        store.mark_root_embedding_stale(&node.id).await?;

        assert_eq!(
            vec_row_count(&store).await?,
            0,
            "stale node removed from vec0"
        );
        assert!(
            store
                .search_embeddings(&unit_query(0), 10, Some(0.5))
                .await?
                .is_empty(),
            "stale embeddings must not be searchable"
        );
        Ok(())
    }

    #[tokio::test]
    async fn test_stale_marker_never_in_knn() -> Result<()> {
        let (store, _tmp) = create_test_store().await?;
        let node = store
            .create_node(
                Node::new("text".to_string(), "marker node".to_string(), json!({})),
                None,
                None,
            )
            .await?;

        // A placeholder marker carries a dummy unit [1,0,0,...] vector but stale=1.
        store.create_stale_embedding_marker(&node.id).await?;

        assert_eq!(
            vec_row_count(&store).await?,
            0,
            "markers not mirrored to vec0"
        );
        // Search by the placeholder's own vector must not surface the marker node.
        let results = store
            .search_embeddings(&unit_query(0), 10, Some(0.5))
            .await?;
        assert!(
            results.iter().all(|r| r.node_id != node.id),
            "stale marker must never appear in KNN results"
        );
        Ok(())
    }

    // C3c: store-layer atomicity — exercises the in-transaction version check
    // that the service-level pre-validation cannot catch (concurrent version bump).
    #[tokio::test]
    async fn test_move_children_to_parent_store_rejects_stale_version() -> Result<()> {
        let (store, _temp) = create_test_store().await?;

        let parent = Node::new("text".to_string(), "Parent".to_string(), json!({}));
        let parent_id = parent.id.clone();
        store.create_node(parent, None, None).await?;

        let new_parent = Node::new("text".to_string(), "New Parent".to_string(), json!({}));
        let new_parent_id = new_parent.id.clone();
        store.create_node(new_parent, None, None).await?;

        // Create children and wire parent edges via move_node (store layer).
        let child1 = Node::new("text".to_string(), "Child 1".to_string(), json!({}));
        let child1_id = child1.id.clone();
        let child1_version = child1.version;
        store.create_node(child1, None, None).await?;
        store.move_node(&child1_id, Some(&parent_id), None).await?;

        let child2 = Node::new("text".to_string(), "Child 2".to_string(), json!({}));
        let child2_id = child2.id.clone();
        // Use a stale version (0) — this bypasses service-level pre-validation,
        // so only the in-transaction SELECT changes() check catches the mismatch.
        let stale_version: i64 = 0;
        store.create_node(child2, None, None).await?;
        store.move_node(&child2_id, Some(&parent_id), None).await?;

        let result = store
            .move_children_to_parent(
                &new_parent_id,
                &[
                    (child1_id.as_str(), child1_version),
                    (child2_id.as_str(), stale_version),
                ],
            )
            .await;

        assert!(
            result.is_err(),
            "stale version should cause store-level failure"
        );
        let err_msg = result.unwrap_err().to_string();
        assert!(
            err_msg.contains("VERSION_CONFLICT"),
            "error should contain VERSION_CONFLICT, got: {}",
            err_msg
        );

        // ALL-OR-NOTHING: child1 must NOT have moved (transaction was rolled back).
        let parent_of_child1 = store.get_parent_id(&child1_id).await?;
        assert_eq!(
            parent_of_child1.as_deref(),
            Some(parent_id.as_str()),
            "child1 should still be under original parent after rollback"
        );

        Ok(())
    }

    #[tokio::test]
    async fn test_move_node_cross_parent_leaves_exactly_one_edge() -> Result<()> {
        // the cross-parent move deletes the old has_child edge and inserts
        // the new one in ONE transaction. After a successful move the node must
        // have EXACTLY ONE has_child edge, pointing at the new parent — never zero
        // (orphaned root, the bug when a non-transactional INSERT failed after the
        // DELETE committed) and never two (old edge left behind).
        let (store, _temp) = create_test_store().await?;

        let parent_a = Node::new("text".to_string(), "Parent A".to_string(), json!({}));
        let parent_a_id = parent_a.id.clone();
        store.create_node(parent_a, None, None).await?;

        let parent_b = Node::new("text".to_string(), "Parent B".to_string(), json!({}));
        let parent_b_id = parent_b.id.clone();
        store.create_node(parent_b, None, None).await?;

        let child = Node::new("text".to_string(), "Child".to_string(), json!({}));
        let child_id = child.id.clone();
        store.create_node(child, None, None).await?;

        // Wire under A, then move to B — this exercises the cross-parent branch.
        store.move_node(&child_id, Some(&parent_a_id), None).await?;
        store.move_node(&child_id, Some(&parent_b_id), None).await?;

        let mut rows = store
            .read()
            .await?
            .query(
                "SELECT in_node FROM relationship WHERE out_node = ?1 AND relationship_type = 'has_child'",
                libsql::params![child_id.clone()],
            )
            .await?;
        let mut parents: Vec<String> = Vec::new();
        while let Some(row) = rows.next().await? {
            parents.push(row.get(0)?);
        }
        assert_eq!(
            parents,
            vec![parent_b_id.clone()],
            "after a cross-parent move the child must have exactly one has_child edge, under the new parent"
        );
        assert_eq!(
            store.get_parent_id(&child_id).await?.as_deref(),
            Some(parent_b_id.as_str()),
        );

        Ok(())
    }

    #[tokio::test]
    async fn test_move_children_to_parent_store_success_and_order() -> Result<()> {
        let (store, _temp) = create_test_store().await?;

        let parent = Node::new("text".to_string(), "Parent".to_string(), json!({}));
        let parent_id = parent.id.clone();
        store.create_node(parent, None, None).await?;

        let new_parent = Node::new("text".to_string(), "New Parent".to_string(), json!({}));
        let new_parent_id = new_parent.id.clone();
        store.create_node(new_parent, None, None).await?;

        let mut child_ids = Vec::new();
        let mut child_versions = Vec::new();
        for i in 0..3 {
            let child = Node::new("text".to_string(), format!("Child {}", i), json!({}));
            let cid = child.id.clone();
            let ver = child.version;
            store.create_node(child, None, None).await?;
            store.move_node(&cid, Some(&parent_id), None).await?;
            child_ids.push(cid);
            child_versions.push(ver);
        }

        let pairs: Vec<(&str, i64)> = child_ids
            .iter()
            .zip(child_versions.iter())
            .map(|(id, &ver)| (id.as_str(), ver))
            .collect();

        let orders = store
            .move_children_to_parent(&new_parent_id, &pairs)
            .await?;

        assert_eq!(orders.len(), 3);
        // Orders must be strictly increasing (sibling order preserved).
        assert!(orders[0] < orders[1] && orders[1] < orders[2]);

        // All children now live under new_parent.
        let new_children = store.get_children(&new_parent_id).await?;
        let new_ids: Vec<&str> = new_children.iter().map(|n| n.id.as_str()).collect();
        for id in &child_ids {
            assert!(
                new_ids.contains(&id.as_str()),
                "child {} should be under new_parent",
                id
            );
        }

        Ok(())
    }

    /// Moving children into a NON-EMPTY parent must APPEND after its existing
    /// children, not restart at 1.0, 2.0… The existing children are seeded via
    /// `create_child_node_atomic` (which appends at 1.0, 2.0), so the old code —
    /// which assigned the moved children a fresh 1.0, 2.0 ignoring existing
    /// siblings — collided their order keys. Deterministic, single-threaded.
    #[tokio::test]
    async fn move_children_into_non_empty_parent_appends_without_collision() -> Result<()> {
        let (store, _temp) = create_test_store().await?;

        // new_parent already holds two children at orders 1.0 and 2.0.
        let new_parent = Node::new("text".to_string(), "New Parent".to_string(), json!({}));
        let new_parent_id = new_parent.id.clone();
        store.create_node(new_parent, None, None).await?;
        for i in 0..2 {
            store
                .create_child_node_atomic(
                    &new_parent_id,
                    "text",
                    &format!("existing {i}"),
                    json!({}),
                    None,
                )
                .await?;
        }

        // Two children under a source parent, to be moved across.
        let src = Node::new("text".to_string(), "Src".to_string(), json!({}));
        let src_id = src.id.clone();
        store.create_node(src, None, None).await?;
        let mut moved_ids = Vec::new();
        let mut moved_vers = Vec::new();
        for i in 0..2 {
            let c = Node::new("text".to_string(), format!("moving {i}"), json!({}));
            let cid = c.id.clone();
            let ver = c.version;
            store.create_node(c, None, None).await?;
            store.move_node(&cid, Some(&src_id), None).await?;
            moved_ids.push(cid);
            moved_vers.push(ver);
        }
        let pairs: Vec<(&str, i64)> = moved_ids
            .iter()
            .zip(moved_vers.iter())
            .map(|(id, &v)| (id.as_str(), v))
            .collect();
        let mut moved_orders = store
            .move_children_to_parent(&new_parent_id, &pairs)
            .await?;

        // All four children now live under new_parent.
        let children = store.get_children(&new_parent_id).await?;
        assert_eq!(
            children.len(),
            4,
            "new_parent should hold 2 existing + 2 moved"
        );

        // Every sibling order key is distinct — no collision with the existing
        // children (the old code produced 1.0, 1.0, 2.0, 2.0 here).
        let mut rows = store
            .read()
            .await?
            .query(
                "SELECT json_extract(properties, '$.order') FROM relationship \
                 WHERE in_node = ?1 AND relationship_type = 'has_child'",
                libsql::params![new_parent_id.clone()],
            )
            .await?;
        let mut orders: Vec<f64> = Vec::new();
        while let Some(r) = rows.next().await? {
            orders.push(r.get::<f64>(0)?);
        }
        assert_eq!(orders.len(), 4);
        orders.sort_by(|a, b| a.partial_cmp(b).unwrap());
        for pair in orders.windows(2) {
            assert!(
                (pair[1] - pair[0]).abs() > f64::EPSILON,
                "colliding sibling order keys after move into non-empty parent: {orders:?}"
            );
        }

        // The moved children hold the two LARGEST order keys — appended after the
        // existing children rather than overlapping them.
        moved_orders.sort_by(|a, b| a.partial_cmp(b).unwrap());
        assert_eq!(
            moved_orders,
            vec![orders[2], orders[3]],
            "moved children should be appended after existing (orders: {orders:?})"
        );

        Ok(())
    }

    /// two `SqliteStore`s opened against the same file (simulating a dev +
    /// production daemon both holding the DB) must not surface SQLITE_BUSY as a
    /// hard error on the loser of a write race. `busy_timeout` (set by migration 1,
    /// applied per-connection in `initialize_schema`) makes the second writer retry
    /// until the first releases its lock, instead of failing immediately.
    ///
    /// Requires a multi-thread runtime: libsql's local connection executes
    /// synchronously (no `spawn_blocking`), so on a current-thread runtime the
    /// lock-holder and writer tasks would starve each other instead of running
    /// concurrently.
    #[tokio::test(flavor = "multi_thread")]
    async fn test_concurrent_writers_retry_instead_of_erroring_on_busy() -> Result<()> {
        let temp_dir = TempDir::new()?;
        let db_path = temp_dir.path().join("shared.db");

        let store_a = SqliteStore::new(db_path.clone()).await?;
        let store_b = SqliteStore::new(db_path.clone()).await?;

        // Hold store_a's write lock open in a background task. Cloning the
        // connection out of the guard is deliberate: this test drives raw
        // BEGIN IMMEDIATE / COMMIT to hold a real SQLite write lock, which is
        // exactly the cross-store contention the store-wide guard cannot cover.
        let conn_a = store_a.write().await.clone();
        let (release_tx, release_rx) = tokio::sync::oneshot::channel::<()>();
        let (locked_tx, locked_rx) = tokio::sync::oneshot::channel::<()>();
        let hold_task = tokio::spawn(async move {
            conn_a.execute("BEGIN IMMEDIATE", ()).await?;
            conn_a
                .execute(
                    "INSERT INTO node (id, node_type, content, properties, lifecycle_status, version, created_at, modified_at) VALUES ('lock-holder', 'text', '', '{}', 'active', 1, '2026-01-01', '2026-01-01')",
                    (),
                )
                .await?;
            let _ = locked_tx.send(());
            // Hold the lock until the test tells us to release it.
            let _ = release_rx.await;
            conn_a.execute("COMMIT", ()).await?;
            anyhow::Ok(())
        });

        locked_rx
            .await
            .context("lock holder failed to acquire write lock")?;

        // store_b attempts a write while store_a holds the lock. Without
        // busy_timeout this fails immediately with SQLITE_BUSY; with it, the
        // write blocks until store_a commits (well under the 5s timeout) then
        // succeeds.
        let write_task = tokio::spawn(async move {
            let node = Node::new("text".to_string(), "from store_b".to_string(), json!({}));
            store_b.create_node(node, None, None).await
        });

        // Release store_a's lock shortly after store_b's write is issued, so the
        // retry path is actually exercised rather than racing a lock that's
        // already free.
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        let _ = release_tx.send(());

        hold_task.await.context("lock holder task panicked")??;
        write_task
            .await
            .context("writer task panicked")?
            .context("store_b write should retry and succeed, not error with SQLITE_BUSY")?;

        Ok(())
    }

    /// Regression for the concurrent-reorder race: many sibling reorders
    /// running at once must all succeed and leave the parent with exactly N
    /// children carrying N distinct fractional-order keys. Without the store's
    /// write guard, the read → compute → write-back interleaves, so concurrent
    /// moves compute order keys against the same stale snapshot — producing
    /// errored moves and/or colliding order values.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn concurrent_reorders_keep_all_children_with_distinct_order() -> Result<()> {
        let (store, _t) = create_test_store().await?;

        let parent = Node::new("text".to_string(), "parent".to_string(), json!({}));
        let parent_id = parent.id.clone();
        store.create_node(parent, None, None).await?;

        const N: usize = 16;
        let mut child_ids = Vec::new();
        for i in 0..N {
            let c = Node::new("text".to_string(), format!("c{i}"), json!({}));
            let cid = c.id.clone();
            store.create_node(c, None, None).await?;
            // Wire under the parent (move_node with a None sibling inserts at the
            // beginning; exact position doesn't matter for this setup).
            store.move_node(&cid, Some(&parent_id), None).await?;
            child_ids.push(cid);
        }

        // Concurrently move every child to the beginning — maximum contention on
        // the sibling-order read/compute/write.
        let mut handles = Vec::new();
        for cid in &child_ids {
            let store = store.clone();
            let cid = cid.clone();
            let parent_id = parent_id.clone();
            handles.push(tokio::spawn(async move {
                store.move_node(&cid, Some(&parent_id), None).await
            }));
        }
        for h in handles {
            h.await
                .expect("reorder task panicked")
                .expect("a concurrent reorder returned an error");
        }

        // No child was lost or duplicated.
        let children = store.get_children(&parent_id).await?;
        assert_eq!(
            children.len(),
            N,
            "concurrent reorders lost/duplicated a child"
        );

        // Every sibling still has a distinct order key (no colliding writes).
        let mut rows = store
            .read()
            .await?
            .query(
                "SELECT json_extract(properties, '$.order') FROM relationship \
                 WHERE in_node = ?1 AND relationship_type = 'has_child'",
                libsql::params![parent_id.clone()],
            )
            .await?;
        let mut orders: Vec<f64> = Vec::new();
        while let Some(r) = rows.next().await? {
            orders.push(r.get::<f64>(0)?);
        }
        assert_eq!(orders.len(), N, "expected {N} sibling edges");
        orders.sort_by(|a, b| a.partial_cmp(b).unwrap());
        for pair in orders.windows(2) {
            assert!(
                (pair[1] - pair[0]).abs() > f64::EPSILON,
                "colliding sibling order keys after concurrent reorders: {orders:?}"
            );
        }

        Ok(())
    }

    /// A read-path method must not hold its own result cursor open across a
    /// nested call to another store method.
    ///
    /// `ReadRows` owns its reader connection until it drops, so a nested
    /// `self.read()` issued while the outer cursor is still live checks out a
    /// *second* connection and neither goes back to the pool until the whole
    /// function returns. That doubles the connections a single sequential call
    /// pins, halving effective concurrency against `MAX_IDLE_READERS` and
    /// inflating the pool's peak.
    ///
    /// Each of these methods reads rows and then consults another store method
    /// with what it found, so each is a place the mistake is easy to make.
    /// Driven sequentially with nothing else in flight, one connection at a
    /// time is all any of them needs.
    ///
    /// Covers all ten sites. Every `peak()` assertion is paired with a check
    /// that the fixture actually reaches the nested call — without one, a
    /// fixture that matches no row leaves the assertion passing vacuously
    /// against the very bug it exists to catch.
    #[tokio::test]
    async fn a_read_path_does_not_pin_a_second_connection_for_a_nested_read() -> Result<()> {
        let (store, _t) = create_test_store().await?;

        // A collection with a member, so `get_collection_by_name` finds a row and
        // then resolves it via the nested `get_node`.
        let mut collection_node = Node::new("collection".to_string(), String::new(), json!({}));
        collection_node.title = Some("Reading".to_string());
        let collection = store.create_node(collection_node, None, None).await?;

        let parent = store
            .create_node(
                Node::new("text".to_string(), "parent".to_string(), json!({})),
                None,
                None,
            )
            .await?;
        // A real `has_child` edge, so the subtree walk takes its multi-node path
        // rather than the single-root early return.
        store
            .create_child_node_atomic(&parent.id, "text", "child", json!({}), None)
            .await?;

        // A `mentions` edge INTO `parent`, so `get_incoming_mention_containers`
        // finds a source and goes on to resolve its container.
        let mentioner = store
            .create_node(
                Node::new("text".to_string(), "mentions parent".to_string(), json!({})),
                None,
                None,
            )
            .await?;
        assert_eq!(
            store
                .bulk_create_mentions(&[(mentioner.id.clone(), parent.id.clone())])
                .await?,
            1,
            "fixture must create the mention the assertion below depends on"
        );

        // An embedding on a root node, so both KNN searches return a hit and go
        // on to resolve it via the nested `get_node`.
        let embedded = store
            .create_node(
                Node::new("text".to_string(), "embedded".to_string(), json!({})),
                None,
                None,
            )
            .await?;
        store
            .upsert_embeddings(&embedded.id, vec![unit_embedding(&embedded.id, 0)])
            .await?;

        // A root node filed into a collection, so `assert_may_gain_parent` finds
        // an offender and goes on to call `get_node_memberships` for the message.
        let member = store
            .create_node(
                Node::new("text".to_string(), "root member".to_string(), json!({})),
                None,
                None,
            )
            .await?;
        assert!(
            store
                .add_to_collection(&member.id, &collection.id, &json!({}))
                .await?
                .is_some(),
            "fixture must file the member, or the offender path below is never reached"
        );

        // Each case: the method, and the nested store call it must not hold a
        // cursor across.
        let gauge = store.readers_in_flight();

        gauge.reset_peak();
        store.get_all_schemas().await?;
        assert_eq!(
            gauge.peak(),
            1,
            "get_all_schemas held a cursor across get_all_schema_declarations"
        );

        gauge.reset_peak();
        assert_eq!(
            store
                .get_subtree_with_relationships(&parent.id)
                .await?
                .0
                .len(),
            2,
            "fixture must produce a real subtree, not the single-root early return"
        );
        assert_eq!(
            gauge.peak(),
            1,
            "get_subtree_with_relationships held a cursor across a nested read"
        );

        gauge.reset_peak();
        assert!(
            store.get_collection_by_name("Reading").await?.is_some(),
            "fixture must match, or the nested get_node below is never reached"
        );
        assert_eq!(
            gauge.peak(),
            1,
            "get_collection_by_name held a cursor across get_node"
        );

        gauge.reset_peak();
        assert_eq!(
            store
                .get_collections_by_names(&["Reading".to_string()])
                .await?
                .len(),
            1,
            "fixture must match, or the nested get_node below is never reached"
        );
        assert_eq!(
            gauge.peak(),
            1,
            "get_collections_by_names held a cursor across get_node"
        );

        gauge.reset_peak();
        assert_eq!(
            store
                .get_incoming_mention_containers(&parent.id)
                .await?
                .len(),
            1,
            "fixture must produce a mention source, or the nested reads are never reached"
        );
        assert_eq!(
            gauge.peak(),
            1,
            "get_incoming_mention_containers held a cursor across a nested read"
        );

        gauge.reset_peak();
        assert!(
            !store.bm25_search_roots("child", 50).await?.is_empty(),
            "fixture must produce an FTS hit, or the nested get_nodes_by_ids is never reached"
        );
        assert_eq!(
            gauge.peak(),
            1,
            "bm25_search_roots held a cursor across get_nodes_by_ids"
        );

        gauge.reset_peak();
        assert!(
            store.get_schema_node("task").await?.is_some(),
            "fixture must find a seeded core schema, or get_schema_declarations is never reached"
        );
        assert_eq!(
            gauge.peak(),
            1,
            "get_schema_node held a cursor across get_schema_declarations"
        );

        // The offender path: a node holding `member_of` may not gain a parent, and
        // building that rejection consults `get_node_memberships`.
        gauge.reset_peak();
        assert!(
            store
                .assert_may_gain_parent(&[member.id.as_str()])
                .await
                .is_err(),
            "fixture must trip the root-only membership guard, or the nested read is never reached"
        );
        assert_eq!(
            gauge.peak(),
            1,
            "assert_may_gain_parent held a cursor across get_node_memberships"
        );

        gauge.reset_peak();
        assert!(
            !store
                .search_embeddings(&unit_query(0), 10, Some(0.5))
                .await?
                .is_empty(),
            "fixture must return a KNN hit, or the nested get_node is never reached"
        );
        assert_eq!(
            gauge.peak(),
            1,
            "search_embeddings held a cursor across get_node"
        );

        gauge.reset_peak();
        assert!(
            !store
                .search_embeddings_by_node_type(&unit_query(0), "text", 10, Some(0.5))
                .await?
                .is_empty(),
            "fixture must return a typed KNN hit, or the nested get_node is never reached"
        );
        assert_eq!(
            gauge.peak(),
            1,
            "search_embeddings_by_node_type held a cursor across get_node"
        );

        // A single-root subtree takes the early-return branch, which reaches
        // `get_node` from a different point in the same function.
        gauge.reset_peak();
        store.get_subtree_with_relationships(&collection.id).await?;
        assert_eq!(
            gauge.peak(),
            1,
            "get_subtree_with_relationships held a cursor across get_node on the leaf path"
        );

        Ok(())
    }

    /// The read path must be physically incapable of writing. `ReadConn` gives
    /// compile-time enforcement; `PRAGMA query_only` is the runtime backstop for
    /// anything that reaches the reader connection another way.
    #[tokio::test]
    async fn read_connection_is_query_only() -> Result<()> {
        let (store, _t) = create_test_store().await?;

        let mut rows = store.read().await?.query("PRAGMA query_only", ()).await?;
        let flag: i64 = rows
            .next()
            .await?
            .expect("query_only returns a row")
            .get(0)?;
        assert_eq!(flag, 1, "read connection must be query_only");

        // And the writer must NOT be, or every write would fail.
        let mut rows = store.write().await.query("PRAGMA query_only", ()).await?;
        let flag: i64 = rows
            .next()
            .await?
            .expect("query_only returns a row")
            .get(0)?;
        assert_eq!(flag, 0, "writer connection must stay writable");

        Ok(())
    }

    /// `bulk_add_to_collections` derives every target collection's base order
    /// from one batched `MAX()` read rather than a round trip per collection.
    /// This locks in that the batched form reproduces the per-collection
    /// semantics it replaced: append AFTER a collection's existing members,
    /// independently per collection, and start a fresh sequence for an empty
    /// one — with no order key reused inside a collection.
    #[tokio::test]
    async fn bulk_add_appends_after_existing_members_per_collection() -> Result<()> {
        async fn member_orders(store: &SqliteStore, collection_id: &str) -> Result<Vec<f64>> {
            let mut rows = store
                .read()
                .await?
                .query(
                    "SELECT json_extract(properties, '$.order') FROM relationship \
                     WHERE out_node = ?1 AND relationship_type = 'member_of'",
                    libsql::params![collection_id.to_string()],
                )
                .await?;
            let mut orders = Vec::new();
            while let Some(row) = rows.next().await? {
                orders.push(row.get::<Option<f64>>(0)?.unwrap_or(0.0));
            }
            orders.sort_by(|a, b| a.partial_cmp(b).unwrap());
            Ok(orders)
        }

        let (store, _t) = create_test_store().await?;

        let new_node = |content: &str, node_type: &str| {
            let n = Node::new(node_type.to_string(), content.to_string(), json!({}));
            (n.id.clone(), n)
        };

        let (seeded_id, seeded) = new_node("seeded", "collection");
        store.create_node(seeded, None, None).await?;
        let (empty_id, empty) = new_node("empty", "collection");
        store.create_node(empty, None, None).await?;

        // One pre-existing member in `seeded`; `empty` has none.
        let (first_id, first) = new_node("first", "text");
        store.create_node(first, None, None).await?;
        store
            .add_to_collection(&first_id, &seeded_id, &json!({}))
            .await?;
        let seeded_before = member_orders(&store, &seeded_id).await?;
        assert_eq!(seeded_before.len(), 1);

        let mut ids = Vec::new();
        for i in 0..4 {
            let (id, n) = new_node(&format!("m{i}"), "text");
            store.create_node(n, None, None).await?;
            ids.push(id);
        }

        // Both collections filled in a single call, which is what makes the
        // batched read have to keep them apart.
        store
            .bulk_add_to_collections(&[
                (ids[0].clone(), seeded_id.clone()),
                (ids[1].clone(), seeded_id.clone()),
                (ids[2].clone(), empty_id.clone()),
                (ids[3].clone(), empty_id.clone()),
            ])
            .await?;

        let seeded_after = member_orders(&store, &seeded_id).await?;
        assert_eq!(seeded_after.len(), 3, "seeded collection kept every member");
        for pair in seeded_after.windows(2) {
            assert!(
                pair[1] - pair[0] > f64::EPSILON,
                "colliding member order keys in the seeded collection: {seeded_after:?}"
            );
        }
        assert!(
            seeded_after[1] > seeded_before[0] && seeded_after[2] > seeded_before[0],
            "bulk-added members must land AFTER the pre-existing member: \
             before {seeded_before:?}, after {seeded_after:?}"
        );

        let empty_after = member_orders(&store, &empty_id).await?;
        assert_eq!(empty_after.len(), 2, "empty collection took both members");
        assert!(
            empty_after[1] - empty_after[0] > f64::EPSILON,
            "colliding member order keys in the empty collection: {empty_after:?}"
        );

        Ok(())
    }

    /// A cursor left open by one task must not pin another task's reads to a
    /// stale snapshot.
    ///
    /// A SQLite connection with a partially consumed statement sits in an
    /// implicit read transaction, and every later statement on that same
    /// connection is stuck on its snapshot until it finishes. Because these
    /// cursors are drained across `await` points, a single shared reader
    /// connection makes this trivially reachable — including "read the row I
    /// just committed" returning nothing. Each in-flight read therefore gets a
    /// connection of its own.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn an_open_cursor_does_not_make_other_reads_stale() -> Result<()> {
        let (store, _t) = create_test_store().await?;

        // Seed a row so the cursor below actually opens a read transaction.
        store
            .create_node(
                Node::new("text".to_string(), "seed".to_string(), json!({})),
                None,
                None,
            )
            .await?;

        // Hold a partially consumed cursor open for the rest of the test.
        let mut held = store.read().await?.query("SELECT id FROM node", ()).await?;
        assert!(held.next().await?.is_some(), "expected at least one row");

        // Commit a new node while that cursor is open…
        let fresh = Node::new("text".to_string(), "fresh".to_string(), json!({}));
        let fresh_id = fresh.id.clone();
        store.create_node(fresh, None, None).await?;

        // …and a read must still see it.
        assert!(
            store.get_node(&fresh_id).await?.is_some(),
            "a read was pinned to a stale snapshot by another task's open cursor"
        );

        drop(held);
        Ok(())
    }

    /// A read issued while another task has a transaction open must see the
    /// database as it was BEFORE that transaction — not its uncommitted rows.
    ///
    /// Sharing one connection with the writer fails this: SQLite shows a connection its
    /// own uncommitted writes, so the reader observes a row that is about to be
    /// rolled back.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn read_does_not_observe_an_open_transaction() -> Result<()> {
        let (store, _t) = create_test_store().await?;

        let ghost_id = uuid::Uuid::new_v4().to_string();

        let db = store.write().await;
        let tx = db.transaction().await?;
        tx.execute(
            "INSERT INTO node (id, node_type, content, properties, title, lifecycle_status, version, created_at, modified_at) \
             VALUES (?1, 'text', 'uncommitted', '{}', NULL, 'active', 1, '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z')",
            libsql::params![ghost_id.clone()],
        )
        .await?;

        // Read from another task. `timeout` so that a read path which wrongly
        // waits on the write lock fails the test instead of hanging it.
        let reader = {
            let store = store.clone();
            let id = ghost_id.clone();
            tokio::spawn(async move { store.get_node(&id).await })
        };
        let seen = tokio::time::timeout(std::time::Duration::from_secs(5), reader)
            .await
            .context("a concurrent read blocked on the open transaction")???;
        assert!(
            seen.is_none(),
            "read observed a row from another task's uncommitted transaction"
        );

        // Rolling back must leave nothing behind either.
        tx.rollback().await?;
        drop(db);
        assert!(store.get_node(&ghost_id).await?.is_none());

        Ok(())
    }

    /// The data-loss scenario: an acknowledged single-statement write must not
    /// be absorbed into — and rolled back with — another task's transaction.
    ///
    /// Sharing one connection with the writer, the update below runs INSIDE the open
    /// transaction, reports success to its caller, and then vanishes when that
    /// transaction rolls back.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_write_is_not_absorbed_by_another_tasks_transaction() -> Result<()> {
        let (store, _t) = create_test_store().await?;

        let keeper = Node::new("text".to_string(), "before".to_string(), json!({}));
        let keeper_id = keeper.id.clone();
        store.create_node(keeper, None, None).await?;

        // Open a transaction and leave it open, as a long import would.
        let db = store.write().await;
        let tx = db.transaction().await?;
        tx.execute(
            "INSERT INTO node (id, node_type, content, properties, title, lifecycle_status, version, created_at, modified_at) \
             VALUES ('doomed-import-row', 'text', 'x', '{}', NULL, 'active', 1, '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z')",
            (),
        )
        .await?;

        // An unrelated edit lands concurrently.
        let editor = {
            let store = store.clone();
            let id = keeper_id.clone();
            tokio::spawn(async move {
                store
                    .update_node(
                        &id,
                        NodeUpdate {
                            content: Some("after".to_string()),
                            ..Default::default()
                        },
                        None,
                    )
                    .await
            })
        };

        // It must be waiting, not executing on our connection.
        tokio::time::sleep(std::time::Duration::from_millis(250)).await;
        assert!(
            !editor.is_finished(),
            "a concurrent write completed while another task held an open transaction — \
             it ran inside that transaction"
        );

        // The import fails and rolls back.
        tx.rollback().await?;
        drop(db);

        tokio::time::timeout(std::time::Duration::from_secs(5), editor)
            .await
            .context("the queued write never completed after the transaction ended")???;

        assert!(
            store.get_node("doomed-import-row").await?.is_none(),
            "the rolled-back import row must be gone"
        );
        assert_eq!(
            store.get_node(&keeper_id).await?.expect("keeper").content,
            "after",
            "the acknowledged edit was rolled back with an unrelated transaction"
        );

        Ok(())
    }

    /// A second transaction must queue behind the first, not fail with
    /// "cannot start a transaction within a transaction".
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_second_transaction_queues_instead_of_erroring() -> Result<()> {
        let (store, _t) = create_test_store().await?;

        let parent = Node::new("text".to_string(), "parent".to_string(), json!({}));
        let parent_id = parent.id.clone();
        store.create_node(parent, None, None).await?;

        let db = store.write().await;
        let tx = db.transaction().await?;

        let second = {
            let store = store.clone();
            let parent_id = parent_id.clone();
            tokio::spawn(async move {
                store
                    .create_child_node_atomic(&parent_id, "text", "child", json!({}), None)
                    .await
            })
        };

        tokio::time::sleep(std::time::Duration::from_millis(250)).await;
        assert!(
            !second.is_finished(),
            "a second transaction started while the first was still open"
        );

        tx.rollback().await?;
        drop(db);

        let child = tokio::time::timeout(std::time::Duration::from_secs(5), second)
            .await
            .context("the queued transaction never completed")??
            .context("second transaction failed instead of queueing")?;

        assert_eq!(store.get_children(&parent_id).await?.len(), 1);
        assert_eq!(child.content, "child");

        Ok(())
    }
}
