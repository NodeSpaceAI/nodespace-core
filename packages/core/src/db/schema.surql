-- ============================================================================
-- NodeSpace SurrealDB Schema - Universal Graph Architecture (Issue #783, #788)
-- ============================================================================
--
-- This schema implements the Universal Graph Architecture where:
-- 1. Node table - Universal storage for ALL node types with embedded properties
-- 2. Relationship table - Universal storage for ALL relationships with relationship_type discriminator
-- 3. Properties - Type-specific data stored in properties field (JSON) for both
--
-- Key Patterns:
-- - ALL node data (including schemas) in single `node` table
-- - ALL relationships in single `relationship` table with relationship_type discriminator
-- - Zero DDL for custom types/relationships (enables Playbook Marketplace)
-- ============================================================================

-- ============================================================================
-- NODE TABLE (SCHEMAFULL - Universal storage for all node types)
-- ============================================================================

-- Node table: Universal storage for ALL node types with embedded properties
DEFINE TABLE IF NOT EXISTS node SCHEMAFULL;

-- Required universal fields
-- NOTE: 'id' is the Record ID (primary key) and is automatically managed by SurrealDB
-- Do NOT redefine it - let SurrealDB handle it natively as a Thing type
DEFINE FIELD IF NOT EXISTS content ON TABLE node TYPE string DEFAULT "";
DEFINE FIELD IF NOT EXISTS node_type ON TABLE node TYPE string ASSERT $value != NONE;
DEFINE FIELD IF NOT EXISTS properties ON TABLE node FLEXIBLE TYPE object DEFAULT {};  -- Type-specific properties (e.g., status, priority for tasks)
DEFINE FIELD IF NOT EXISTS version ON TABLE node TYPE int DEFAULT 1 ASSERT $value >= 1;
DEFINE FIELD IF NOT EXISTS created_at ON TABLE node TYPE datetime DEFAULT time::now();
DEFINE FIELD IF NOT EXISTS modified_at ON TABLE node TYPE datetime DEFAULT time::now();

-- Title field for efficient @mention autocomplete search (Issue #821)
-- Optional field populated for root nodes and task nodes only
-- Contains markdown-stripped content for clean display and efficient search
DEFINE FIELD IF NOT EXISTS title ON TABLE node TYPE option<string>;

-- Lifecycle status for knowledge governance (Issue #755)
-- Controls node visibility in search and queries:
--   - "active" (default): Included in search, visible in UI
--   - "archived": Excluded from search by default, hidden from default views, restorable
--   - "deleted": Soft-deleted, in trash, excluded from all queries, purgeable
DEFINE FIELD IF NOT EXISTS lifecycle_status ON TABLE node TYPE string DEFAULT "active";

-- Indexes for performance
DEFINE INDEX IF NOT EXISTS idx_node_type ON TABLE node COLUMNS node_type;
DEFINE INDEX IF NOT EXISTS idx_node_modified ON TABLE node COLUMNS modified_at;
DEFINE INDEX IF NOT EXISTS idx_node_lifecycle ON TABLE node COLUMNS lifecycle_status;

-- Standard index for @mention autocomplete (Issue #821)
-- Full-text search with ANALYZER requires SurrealDB 2.x+ with proper configuration
-- For now, use a standard index which still enables efficient exact-match and prefix queries
DEFINE INDEX IF NOT EXISTS idx_node_title ON TABLE node COLUMNS title;

-- Note: Partial indexes (WHERE clause) are not supported in SurrealDB 2.x
-- Properties are queried via node_type filter + properties.field access

-- NO beforeSiblingId - structure lives in has_child relationships!
-- NO parentId - hierarchy lives in has_child relationships!

-- ============================================================================
-- UNIVERSAL RELATIONSHIP TABLE (Issue #788 - Single table for all relationships)
-- ============================================================================
--
-- All relationships stored in single `relationship` table with type discrimination.
-- This enables custom relationship types without DDL (Playbook Marketplace).
--
-- Core relationship_types:
--   - has_child: Document tree hierarchy (order in properties)
--   - mentions: @references and [[links]] (context, offset in properties)
--   - member_of: Collection membership
-- ============================================================================

DEFINE TABLE IF NOT EXISTS relationship SCHEMAFULL TYPE RELATION IN node OUT node;

-- The discriminator field
DEFINE FIELD IF NOT EXISTS relationship_type ON TABLE relationship TYPE string ASSERT $value != NONE;

-- Type-specific data stored in properties
DEFINE FIELD IF NOT EXISTS properties ON TABLE relationship FLEXIBLE TYPE object DEFAULT {};

-- System metadata
DEFINE FIELD IF NOT EXISTS version ON TABLE relationship TYPE int DEFAULT 1;
DEFINE FIELD IF NOT EXISTS created_at ON TABLE relationship TYPE datetime DEFAULT time::now();
DEFINE FIELD IF NOT EXISTS modified_at ON TABLE relationship TYPE datetime DEFAULT time::now();

-- Core indexes for all relationship queries
DEFINE INDEX IF NOT EXISTS idx_relationship_type ON TABLE relationship COLUMNS relationship_type;
DEFINE INDEX IF NOT EXISTS idx_rel_in ON TABLE relationship COLUMNS in, relationship_type;
DEFINE INDEX IF NOT EXISTS idx_rel_out ON TABLE relationship COLUMNS out, relationship_type;

-- Composite index for ordered children lookup (has_child relationships)
-- Query: SELECT * FROM relationship WHERE in = $parent AND relationship_type = 'has_child' ORDER BY properties.order
DEFINE INDEX IF NOT EXISTS idx_rel_child_order ON TABLE relationship COLUMNS in, relationship_type, properties.order;

-- Unique constraint for all relationship types
-- Design Decision (Issue #788): We use a universal unique constraint on (in, out, relationship_type)
-- rather than a partial index only for has_child. This prevents duplicate relationships between
-- any two nodes for the same relationship type. While the original spec only required uniqueness
-- for has_child (a node can only have one parent), applying this universally:
-- 1. Simplifies schema - one constraint covers all relationship types
-- 2. Prevents data anomalies in all relationship types (e.g., duplicate mentions, member_of)
-- 3. Aligns with graph database best practices (edges should be unique by type)
-- Future consideration: If a relationship type needs to allow duplicates (e.g., multiple mentions
-- with different contexts), that type should use a custom edge table via the schema system.
DEFINE INDEX IF NOT EXISTS idx_rel_unique_child ON TABLE relationship COLUMNS in, out, relationship_type UNIQUE;

-- ============================================================================
-- EMBEDDINGS TABLE (Root-Aggregate Model for Semantic Search)
-- ============================================================================
--
-- NodeSpace uses a root-aggregate embedding model:
-- - Only ROOT nodes (no parent relationship) of embeddable types get embedded
-- - Embeddings represent the semantic content of the entire subtree
-- - Chunking support for content > 512 tokens
-- - Backend-managed queue with debouncing (no frontend involvement)
--
-- Embeddable types: text, header, code-block, schema
-- NOT embeddable: task, date, person, ai-chat, any child node
-- ============================================================================

DEFINE TABLE IF NOT EXISTS embedding SCHEMAFULL;

-- Link to root node
-- Uses record<node> type for SurrealDB record link semantics
DEFINE FIELD IF NOT EXISTS node ON TABLE embedding TYPE record<node>;

-- Vector data (768 dimensions for nomic-embed-text-v1.5)
DEFINE FIELD IF NOT EXISTS vector ON TABLE embedding TYPE array<float>;
DEFINE FIELD IF NOT EXISTS dimension ON TABLE embedding TYPE int DEFAULT 768;

-- Model info (future multi-model support)
DEFINE FIELD IF NOT EXISTS model_name ON TABLE embedding TYPE string DEFAULT 'nomic-embed-text-v1.5';

-- Chunking support (for content > 512 tokens)
DEFINE FIELD IF NOT EXISTS chunk_index ON TABLE embedding TYPE int DEFAULT 0;
DEFINE FIELD IF NOT EXISTS chunk_start ON TABLE embedding TYPE int DEFAULT 0;
DEFINE FIELD IF NOT EXISTS chunk_end ON TABLE embedding TYPE option<int>;
DEFINE FIELD IF NOT EXISTS total_chunks ON TABLE embedding TYPE int DEFAULT 1;

-- Content tracking (for change detection)
DEFINE FIELD IF NOT EXISTS content_hash ON TABLE embedding TYPE option<string>;
DEFINE FIELD IF NOT EXISTS token_count ON TABLE embedding TYPE option<int>;

-- Staleness tracking (for re-embedding queue)
DEFINE FIELD IF NOT EXISTS stale ON TABLE embedding TYPE bool DEFAULT true;

-- Error tracking
DEFINE FIELD IF NOT EXISTS error_count ON TABLE embedding TYPE int DEFAULT 0;
DEFINE FIELD IF NOT EXISTS last_error ON TABLE embedding TYPE option<string>;

-- Timestamps
DEFINE FIELD IF NOT EXISTS created_at ON TABLE embedding TYPE datetime DEFAULT time::now();
DEFINE FIELD IF NOT EXISTS modified_at ON TABLE embedding TYPE datetime DEFAULT time::now();

-- Indexes for efficient lookups
DEFINE INDEX IF NOT EXISTS idx_embedding_node ON TABLE embedding COLUMNS node;
DEFINE INDEX IF NOT EXISTS idx_embedding_stale ON TABLE embedding COLUMNS stale;
-- Unique constraint: one embedding per (node, model, chunk_index) combination
DEFINE INDEX IF NOT EXISTS idx_embedding_unique ON TABLE embedding COLUMNS node, model_name, chunk_index UNIQUE;
-- MTREE vector index for fast semantic similarity search (Issue #776)
-- Uses cosine distance for nomic-embed-text-v1.5 embeddings
DEFINE INDEX IF NOT EXISTS idx_embedding_vector ON TABLE embedding COLUMNS vector MTREE DIMENSION 768 DIST COSINE TYPE F32;

-- ============================================================================
-- Schema Version Tracking
-- ============================================================================

-- Issue #794: Schema version is now stored in namespaced properties
-- Format: properties[node_type]._schema_version
-- This allows version tracking per-type while supporting type changes
-- The MigrationRegistry reads/writes to this namespaced location
