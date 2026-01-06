-- ============================================================================
-- NodeSpace SurrealDB Schema - Universal Graph Architecture (Issue #783)
-- ============================================================================
--
-- This schema implements the Universal Graph Architecture where:
-- 1. Node table - Universal storage for ALL node types with embedded properties
-- 2. Properties - Type-specific data stored in node.properties field (JSON)
-- 3. Graph Edges - has_child, mentions, and member_of for node relationships
--
-- Key Pattern: ALL node data (including schemas) in single node table
-- Schema nodes use node_type='schema' with fields/relationships in properties
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

-- Indexes for performance
DEFINE INDEX IF NOT EXISTS idx_node_type ON TABLE node COLUMNS node_type;
DEFINE INDEX IF NOT EXISTS idx_node_modified ON TABLE node COLUMNS modified_at;

-- Note: Partial indexes (WHERE clause) are not supported in SurrealDB 2.x
-- Properties are queried via node_type filter + properties.field access

-- NO beforeSiblingId - structure lives in has_child edges!
-- NO parentId - hierarchy lives in has_child edges!

-- ============================================================================
-- GRAPH RELATIONS (SCHEMAFULL - Node-to-node relationships)
-- ============================================================================

-- Has_child relation: Hierarchical relationships with fractional ordering
DEFINE TABLE IF NOT EXISTS has_child SCHEMAFULL TYPE RELATION IN node OUT node;

-- Required fields for hierarchy
DEFINE FIELD IF NOT EXISTS order ON TABLE has_child TYPE float ASSERT $value != NONE;
DEFINE FIELD IF NOT EXISTS created_at ON TABLE has_child TYPE datetime DEFAULT time::now();
DEFINE FIELD IF NOT EXISTS version ON TABLE has_child TYPE int DEFAULT 1;  -- OCC for concurrent edits

-- Index for efficient ordering queries
DEFINE INDEX IF NOT EXISTS idx_child_order ON TABLE has_child COLUMNS in, order;

-- Ensure unique parent-child relationships
DEFINE INDEX IF NOT EXISTS idx_unique_child ON TABLE has_child COLUMNS in, out UNIQUE;

-- ============================================================================

-- Mentions relation: Bidirectional references between nodes
DEFINE TABLE IF NOT EXISTS mentions SCHEMAFULL TYPE RELATION IN node OUT node;

-- Required base fields
DEFINE FIELD IF NOT EXISTS created_at ON TABLE mentions TYPE datetime DEFAULT time::now();
DEFINE FIELD IF NOT EXISTS context ON TABLE mentions TYPE string DEFAULT "";
DEFINE FIELD IF NOT EXISTS offset ON TABLE mentions TYPE int DEFAULT 0;
DEFINE FIELD IF NOT EXISTS root_id ON TABLE mentions TYPE option<string>;  -- Root node of mentioning node's tree

-- Indexes for efficient queries
DEFINE INDEX IF NOT EXISTS idx_mentions_in ON TABLE mentions COLUMNS in;
DEFINE INDEX IF NOT EXISTS idx_mentions_out ON TABLE mentions COLUMNS out;

-- Prevent duplicate mentions
DEFINE INDEX IF NOT EXISTS idx_unique_mention ON TABLE mentions COLUMNS in, out UNIQUE;

-- ============================================================================

-- Member_of relation: Collection membership (node belongs to collection)
-- This represents the "labeling" relationship where any node can be a member
-- of one or more collection nodes. Collections form a DAG (Directed Acyclic Graph).
--
-- Direction: member_of goes FROM member node TO collection node
-- Example: node:abc -> collection:engineering means "abc belongs to engineering"
--
-- Note: Collection hierarchy uses has_child edges, not member_of.
-- member_of is only for non-collection nodes joining collections.
DEFINE TABLE IF NOT EXISTS member_of SCHEMAFULL TYPE RELATION IN node OUT node;

-- Required base fields
DEFINE FIELD IF NOT EXISTS created_at ON TABLE member_of TYPE datetime DEFAULT time::now();

-- Indexes for efficient queries
-- "What collections does this node belong to?" - query by `in`
DEFINE INDEX IF NOT EXISTS idx_member_of_in ON TABLE member_of COLUMNS in;
-- "What nodes belong to this collection?" - query by `out`
DEFINE INDEX IF NOT EXISTS idx_member_of_out ON TABLE member_of COLUMNS out;

-- Prevent duplicate memberships (a node can only join a collection once)
DEFINE INDEX IF NOT EXISTS idx_unique_membership ON TABLE member_of COLUMNS in, out UNIQUE;

-- ============================================================================
-- EMBEDDINGS TABLE (Root-Aggregate Model for Semantic Search)
-- ============================================================================
--
-- NodeSpace uses a root-aggregate embedding model:
-- - Only ROOT nodes (no parent edge) of embeddable types get embedded
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

-- Track schema version for migrations
DEFINE FIELD IF NOT EXISTS _schema_version ON TABLE node TYPE int DEFAULT 1;
