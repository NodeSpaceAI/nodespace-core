-- ============================================================================
-- NodeSpace SurrealDB Schema - Hub-and-Spoke Architecture with Record Links
-- ============================================================================
--
-- This schema implements a hub-and-spoke design where:
-- 1. Hub (node table) - Universal metadata for ALL node types
-- 2. Spokes (task, date, schema tables) - Type-specific queryable data
-- 3. Record Links - Bidirectional pointers for 1-to-1 composition (NOT RELATE)
-- 4. Graph Edges - has_child and mentions for node-to-node relationships
--
-- Key Pattern: Use Record Links for composition, RELATE for relationships
-- ============================================================================

-- ============================================================================
-- HUB TABLE (SCHEMAFULL - Universal metadata)
-- ============================================================================

-- Node table: Universal metadata for ALL node types
DEFINE TABLE IF NOT EXISTS node SCHEMAFULL;

-- Required universal fields
-- NOTE: 'id' is the Record ID (primary key) and is automatically managed by SurrealDB
-- Do NOT redefine it - let SurrealDB handle it natively as a Thing type
DEFINE FIELD IF NOT EXISTS content ON TABLE node TYPE string DEFAULT "";
DEFINE FIELD IF NOT EXISTS nodeType ON TABLE node TYPE string ASSERT $value != NONE;
DEFINE FIELD IF NOT EXISTS data ON TABLE node TYPE option<record>;  -- Record Link to spoke (task, date, schema, or NULL)
DEFINE FIELD IF NOT EXISTS version ON TABLE node TYPE int DEFAULT 1 ASSERT $value >= 1;
DEFINE FIELD IF NOT EXISTS createdAt ON TABLE node TYPE datetime DEFAULT time::now();
DEFINE FIELD IF NOT EXISTS modifiedAt ON TABLE node TYPE datetime DEFAULT time::now();

-- Indexes for performance
DEFINE INDEX IF NOT EXISTS idx_node_type ON TABLE node COLUMNS nodeType;
DEFINE INDEX IF NOT EXISTS idx_node_modified ON TABLE node COLUMNS modifiedAt;

-- NO beforeSiblingId - structure lives in has_child edges!
-- NO parentId - hierarchy lives in has_child edges!

-- ============================================================================
-- SPOKE TABLES (SCHEMAFULL + FLEXIBLE - Type-specific queryable data)
-- ============================================================================

-- Task spoke: Status, priority, due dates with efficient indexing
DEFINE TABLE IF NOT EXISTS task SCHEMAFULL;
DEFINE FIELD IF NOT EXISTS node ON TABLE task TYPE option<record>;  -- Reverse link to hub node

-- Core task fields (indexed for queries)
DEFINE FIELD IF NOT EXISTS status ON TABLE task TYPE string DEFAULT 'todo';
DEFINE FIELD IF NOT EXISTS priority ON TABLE task TYPE option<string>;
DEFINE FIELD IF NOT EXISTS due_date ON TABLE task TYPE option<datetime>;
DEFINE FIELD IF NOT EXISTS assignee ON TABLE task TYPE option<record>;

-- Indexes for efficient spoke queries
DEFINE INDEX IF NOT EXISTS idx_task_status ON TABLE task COLUMNS status;
DEFINE INDEX IF NOT EXISTS idx_task_priority ON TABLE task COLUMNS priority;
DEFINE INDEX IF NOT EXISTS idx_task_due_date ON TABLE task COLUMNS due_date;

-- Note: SCHEMAFULL allows undefined fields by default (flexible)
-- Only defined fields above have type checking

-- ============================================================================

-- Date spoke: Timezone, holiday tracking
DEFINE TABLE IF NOT EXISTS date SCHEMAFULL;
DEFINE FIELD IF NOT EXISTS node ON TABLE date TYPE option<record>;  -- Reverse link to hub node

-- Core date fields
DEFINE FIELD IF NOT EXISTS timezone ON TABLE date TYPE string DEFAULT 'UTC';
DEFINE FIELD IF NOT EXISTS is_holiday ON TABLE date TYPE bool DEFAULT false;

-- Note: SCHEMAFULL allows undefined fields by default (flexible)

-- ============================================================================

-- Schema spoke: Type definitions
DEFINE TABLE IF NOT EXISTS schema SCHEMAFULL;
DEFINE FIELD IF NOT EXISTS node ON TABLE schema TYPE option<record>;  -- Reverse link to hub node

-- Core schema fields
-- NOTE: Storing entire schema as JSON string due to SurrealDB binary protocol limitations
-- with nested arrays containing enums. The properties binding converts the object to a
-- JSON string on CREATE/UPDATE, and we deserialize it on retrieval.
DEFINE FIELD IF NOT EXISTS is_core ON TABLE schema TYPE bool DEFAULT false;
DEFINE FIELD IF NOT EXISTS version ON TABLE schema TYPE int DEFAULT 1;
DEFINE FIELD IF NOT EXISTS description ON TABLE schema TYPE string DEFAULT "";
DEFINE FIELD IF NOT EXISTS fields ON TABLE schema TYPE option<string> DEFAULT NONE;

-- Note: SCHEMAFULL allows undefined fields by default (flexible)

-- ============================================================================
-- GRAPH RELATIONS (SCHEMAFULL - Node-to-node relationships)
-- ============================================================================

-- Has_child relation: Hierarchical relationships with fractional ordering
DEFINE TABLE IF NOT EXISTS has_child SCHEMAFULL TYPE RELATION IN node OUT node;

-- Required fields for hierarchy
DEFINE FIELD IF NOT EXISTS order ON TABLE has_child TYPE float ASSERT $value != NONE;
DEFINE FIELD IF NOT EXISTS createdAt ON TABLE has_child TYPE datetime DEFAULT time::now();
DEFINE FIELD IF NOT EXISTS version ON TABLE has_child TYPE int DEFAULT 1;  -- OCC for concurrent edits

-- Index for efficient ordering queries
DEFINE INDEX IF NOT EXISTS idx_child_order ON TABLE has_child COLUMNS in, order;

-- Ensure unique parent-child relationships
DEFINE INDEX IF NOT EXISTS idx_unique_child ON TABLE has_child COLUMNS in, out UNIQUE;

-- ============================================================================

-- Mentions relation: Bidirectional references between nodes
DEFINE TABLE IF NOT EXISTS mentions SCHEMAFULL TYPE RELATION IN node OUT node;

-- Required base fields
DEFINE FIELD IF NOT EXISTS createdAt ON TABLE mentions TYPE datetime DEFAULT time::now();
DEFINE FIELD IF NOT EXISTS context ON TABLE mentions TYPE string DEFAULT "";
DEFINE FIELD IF NOT EXISTS offset ON TABLE mentions TYPE int DEFAULT 0;

-- Indexes for efficient queries
DEFINE INDEX IF NOT EXISTS idx_mentions_in ON TABLE mentions COLUMNS in;
DEFINE INDEX IF NOT EXISTS idx_mentions_out ON TABLE mentions COLUMNS out;

-- Prevent duplicate mentions
DEFINE INDEX IF NOT EXISTS idx_unique_mention ON TABLE mentions COLUMNS in, out UNIQUE;

-- ============================================================================
-- EMBEDDINGS TABLE (Vector storage for semantic search)
-- ============================================================================

-- Embeddings table: Stores vector embeddings for semantic search
-- Separate from node table for flexibility in multi-model support and efficient vector operations
DEFINE TABLE IF NOT EXISTS embedding SCHEMAFULL;

-- Required fields
DEFINE FIELD IF NOT EXISTS node_id ON TABLE embedding TYPE record<node> ASSERT $value != NONE;
DEFINE FIELD IF NOT EXISTS model_name ON TABLE embedding TYPE string DEFAULT 'bge-small-en-v1.5';
DEFINE FIELD IF NOT EXISTS vector ON TABLE embedding TYPE array<float> ASSERT $value != NONE;
DEFINE FIELD IF NOT EXISTS dimension ON TABLE embedding TYPE int ASSERT $value > 0;
DEFINE FIELD IF NOT EXISTS createdAt ON TABLE embedding TYPE datetime DEFAULT time::now();
DEFINE FIELD IF NOT EXISTS modifiedAt ON TABLE embedding TYPE datetime DEFAULT time::now();

-- Indexes for efficient lookups
DEFINE INDEX IF NOT EXISTS idx_embedding_node ON TABLE embedding COLUMNS node_id;
DEFINE INDEX IF NOT EXISTS idx_embedding_model ON TABLE embedding COLUMNS model_name;
-- Unique constraint: one embedding per node per model
DEFINE INDEX IF NOT EXISTS idx_embedding_unique ON TABLE embedding COLUMNS node_id, model_name UNIQUE;

-- ============================================================================
-- Schema Version Tracking
-- ============================================================================

-- Track schema version for migrations
DEFINE FIELD IF NOT EXISTS _schema_version ON TABLE node TYPE int DEFAULT 1;
