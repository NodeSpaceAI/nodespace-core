//! `SqliteStore` methods — embeddings concern (split from the god-object per ADR-053 prep).
use super::*;

impl SqliteStore {
    /// Replace a node's embeddings with locally-generated vectors (`origin =
    /// 'local'`). This is what the embedding generation path uses; the cloud-push
    /// sweep reads only `'local'` rows.
    pub async fn upsert_embeddings(
        &self,
        node_id: &str,
        embeddings: Vec<crate::models::NewEmbedding>,
    ) -> Result<()> {
        self.upsert_embeddings_with_origin(node_id, embeddings, "local")
            .await
    }

    /// Replace a node's embeddings with vectors PULLED from another device
    /// (`origin = 'remote'`). Identical to `upsert_embeddings` except
    /// for the provenance tag, which keeps the push sweep from re-pushing a vector
    /// this device merely received (no cross-device re-push loop).
    pub async fn apply_remote_embeddings(
        &self,
        node_id: &str,
        embeddings: Vec<crate::models::NewEmbedding>,
    ) -> Result<()> {
        self.upsert_embeddings_with_origin(node_id, embeddings, "remote")
            .await
    }

    async fn upsert_embeddings_with_origin(
        &self,
        node_id: &str,
        embeddings: Vec<crate::models::NewEmbedding>,
        origin: &str,
    ) -> Result<()> {
        if embeddings.is_empty() {
            return Ok(());
        }

        // Replace the node's embeddings atomically across both `embedding` and the
        // `vec_embeddings` vec0 mirror. vec0 is keyed by embedding_id, so the leading
        // DELETE must clear the node's existing vec rows via its current embedding ids
        // BEFORE the rows disappear from `embedding`.
        let db = self.write().await;
        let tx = db
            .transaction()
            .await
            .context("Failed to begin upsert_embeddings transaction")?;

        tx.execute(
            "DELETE FROM vec_embeddings WHERE embedding_id IN (SELECT id FROM embedding WHERE node_id = ?1)",
            libsql::params![node_id.to_string()],
        )
        .await
        .context("Failed to clear vec_embeddings for node")?;

        tx.execute(
            "DELETE FROM embedding WHERE node_id = ?1",
            libsql::params![node_id.to_string()],
        )
        .await
        .context("Failed to delete existing embeddings")?;

        let now = Utc::now().to_rfc3339();
        let rows: Vec<(String, Vec<u8>, Vec<libsql::Value>)> = embeddings
            .into_iter()
            .map(|emb| {
                let id = uuid::Uuid::new_v4().to_string();
                let dimension = emb.vector.len() as i64;
                let vector_blob: Vec<u8> =
                    emb.vector.iter().flat_map(|f| f.to_le_bytes()).collect();
                let model_name = emb
                    .model_name
                    .unwrap_or_else(|| "nomic-embed-text-v1.5".to_string());

                let params = vec![
                    libsql::Value::Text(id.clone()),
                    libsql::Value::Text(emb.node_id.clone()),
                    libsql::Value::Blob(vector_blob.clone()),
                    libsql::Value::Integer(dimension),
                    libsql::Value::Text(model_name),
                    libsql::Value::Integer(emb.chunk_index as i64),
                    libsql::Value::Integer(emb.chunk_start as i64),
                    libsql::Value::Integer(emb.chunk_end as i64),
                    libsql::Value::Integer(emb.total_chunks as i64),
                    libsql::Value::Text(emb.content_hash),
                    libsql::Value::Integer(emb.token_count as i64),
                    libsql::Value::Text(origin.to_string()),
                    libsql::Value::Text(now.clone()),
                    libsql::Value::Text(now.clone()),
                ];

                (id, vector_blob, params)
            })
            .collect();

        // Batch into multi-row INSERTs, chunked so each statement's bound
        // parameter count stays under SQLite's compiled SQLITE_MAX_VARIABLE_NUMBER (32766).
        const EMBEDDING_CHUNK: usize = 60; // 14 params/row
        for chunk in rows.chunks(EMBEDDING_CHUNK) {
            let placeholders: Vec<String> = (0..chunk.len())
                .map(|i| {
                    let base = i * 14;
                    format!(
                        "(?{}, ?{}, ?{}, ?{}, ?{}, ?{}, ?{}, ?{}, ?{}, ?{}, ?{}, 0, 0, NULL, ?{}, ?{}, ?{})",
                        base + 1,
                        base + 2,
                        base + 3,
                        base + 4,
                        base + 5,
                        base + 6,
                        base + 7,
                        base + 8,
                        base + 9,
                        base + 10,
                        base + 11,
                        base + 12,
                        base + 13,
                        base + 14
                    )
                })
                .collect();
            let sql = format!(
                "INSERT INTO embedding (id, node_id, vector, dimension, model_name, chunk_index, chunk_start, chunk_end, total_chunks, content_hash, token_count, stale, error_count, last_error, origin, created_at, modified_at) VALUES {}",
                placeholders.join(", ")
            );
            let params: Vec<libsql::Value> = chunk.iter().flat_map(|(_, _, p)| p.clone()).collect();

            tx.execute(&sql, params)
                .await
                .context("Failed to insert embeddings batch")?;
        }

        // Mirror the (non-stale) vectors into vec0 for KNN search, using the same ids.
        const VEC_CHUNK: usize = 400; // 2 params/row
        for chunk in rows.chunks(VEC_CHUNK) {
            let placeholders: Vec<String> = (0..chunk.len())
                .map(|i| format!("(?{}, ?{})", i * 2 + 1, i * 2 + 2))
                .collect();
            let sql = format!(
                "INSERT INTO vec_embeddings (embedding_id, vector) VALUES {}",
                placeholders.join(", ")
            );
            let params: Vec<libsql::Value> = chunk
                .iter()
                .flat_map(|(id, vector_blob, _)| {
                    vec![
                        libsql::Value::Text(id.clone()),
                        libsql::Value::Blob(vector_blob.clone()),
                    ]
                })
                .collect();

            tx.execute(&sql, params)
                .await
                .context("Failed to insert into vec_embeddings batch")?;
        }

        tx.commit()
            .await
            .context("Failed to commit upsert_embeddings transaction")?;

        Ok(())
    }

    /// Decode a stored embedding row into the `Embedding` model. Vectors are
    /// persisted by `upsert_embeddings` as a little-endian f32 blob; decode it
    /// back to `Vec<f32>`. Column order must match the SELECTs below.
    fn row_to_embedding(row: &libsql::Row) -> Result<crate::models::Embedding> {
        let id: String = row.get(0)?;
        let node: String = row.get(1)?;
        let vector_blob: Vec<u8> = row.get(2)?;
        let dimension: i64 = row.get(3)?;
        let model_name: String = row.get(4)?;
        let chunk_index: i64 = row.get(5)?;
        let chunk_start: i64 = row.get(6)?;
        let chunk_end: Option<i64> = row.get(7)?;
        let total_chunks: i64 = row.get(8)?;
        let content_hash: Option<String> = row.get(9)?;
        let token_count: Option<i64> = row.get(10)?;
        let stale: i64 = row.get(11)?;
        let error_count: i64 = row.get(12)?;
        let last_error: Option<String> = row.get(13)?;
        let created_at_str: String = row.get(14)?;
        let modified_at_str: String = row.get(15)?;

        let vector: Vec<f32> = vector_blob
            .chunks_exact(4)
            .map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]]))
            .collect();
        let created_at = DateTime::parse_from_rfc3339(&created_at_str)
            .with_context(|| format!("Invalid embedding created_at: {}", created_at_str))?
            .with_timezone(&Utc);
        let modified_at = DateTime::parse_from_rfc3339(&modified_at_str)
            .with_context(|| format!("Invalid embedding modified_at: {}", modified_at_str))?
            .with_timezone(&Utc);

        Ok(crate::models::Embedding {
            id,
            node,
            vector,
            dimension: dimension as i32,
            model_name,
            chunk_index: chunk_index as i32,
            chunk_start: chunk_start as i32,
            chunk_end: chunk_end.map(|v| v as i32),
            total_chunks: total_chunks as i32,
            content_hash,
            token_count: token_count.map(|v| v as i32),
            stale: stale != 0,
            error_count: error_count as i32,
            last_error,
            created_at,
            modified_at,
        })
    }

    /// Read all locally-stored embedding records for a node (one per chunk),
    /// ordered by chunk index. Used by the Pro daemon's cloud push to
    /// mirror a node's vectors into Supabase pgvector.
    pub async fn get_embeddings(&self, node_id: &str) -> Result<Vec<crate::models::Embedding>> {
        let mut rows = self
            .read()
            .await?
            .query(
                "SELECT id, node_id, vector, dimension, model_name, chunk_index, chunk_start, \
                 chunk_end, total_chunks, content_hash, token_count, stale, error_count, \
                 last_error, created_at, modified_at \
                 FROM embedding WHERE node_id = ?1 ORDER BY chunk_index",
                libsql::params![node_id.to_string()],
            )
            .await
            .context("Failed to query embeddings for node")?;

        let mut out = Vec::new();
        while let Some(row) = rows.next().await? {
            out.push(Self::row_to_embedding(&row)?);
        }
        Ok(out)
    }

    /// Read **locally-generated** (`origin = 'local'`) embedding records modified
    /// at or after `since`, across all nodes, ordered by `modified_at`. Drives the
    /// Pro daemon's cloud-push sweep: the daemon keeps a cursor over
    /// `modified_at` and pushes newly (re)computed vectors. Stale rows are
    /// included — the caller decides whether to skip them.
    ///
    /// The `origin = 'local'` filter excludes vectors PULLED from
    /// other devices, so a received vector is never re-pushed — without it, a
    /// pull's `modified_at = now` would re-arm this sweep and bounce the vector
    /// back to cloud, amplifying writes and (on heterogeneous devices) looping.
    ///
    /// INVARIANT: assumes every writer stores `modified_at` as a UTC rfc3339
    /// string (`Utc::now().to_rfc3339()`, as `upsert_embeddings` does). The cursor
    /// compares lexicographically, which equals chronological order ONLY for that
    /// fixed `+00:00`-offset form; a `Z`-suffixed or non-UTC timestamp would break
    /// ordering and make the sweep skip rows. Served by `idx_emb_modified`.
    pub async fn embeddings_modified_since(
        &self,
        since: DateTime<Utc>,
    ) -> Result<Vec<crate::models::Embedding>> {
        let mut rows = self
            .read()
            .await?
            .query(
                "SELECT id, node_id, vector, dimension, model_name, chunk_index, chunk_start, \
                 chunk_end, total_chunks, content_hash, token_count, stale, error_count, \
                 last_error, created_at, modified_at \
                 FROM embedding WHERE origin = 'local' AND modified_at >= ?1 \
                 ORDER BY modified_at, node_id, chunk_index",
                libsql::params![since.to_rfc3339()],
            )
            .await
            .context("Failed to query embeddings modified since")?;

        let mut out = Vec::new();
        while let Some(row) = rows.next().await? {
            out.push(Self::row_to_embedding(&row)?);
        }
        Ok(out)
    }

    pub async fn mark_root_embedding_stale(&self, node_id: &str) -> Result<()> {
        let now = Utc::now().to_rfc3339();
        let db = self.write().await;
        let tx = db
            .transaction()
            .await
            .context("Failed to begin mark-stale transaction")?;

        // Stale vectors must not be searchable: drop them from the vec0 mirror.
        // `upsert_embeddings` repopulates vec0 when the node is re-embedded.
        tx.execute(
            "DELETE FROM vec_embeddings WHERE embedding_id IN (SELECT id FROM embedding WHERE node_id = ?1)",
            libsql::params![node_id.to_string()],
        )
        .await
        .context("Failed to clear vec_embeddings for stale node")?;

        tx.execute(
            "UPDATE embedding SET stale = 1, modified_at = ?1 WHERE node_id = ?2",
            libsql::params![now, node_id.to_string()],
        )
        .await
        .context("Failed to mark embedding stale")?;

        tx.commit()
            .await
            .context("Failed to commit mark-stale transaction")?;
        Ok(())
    }

    pub async fn get_stale_embedding_root_ids(
        &self,
        limit: Option<i64>,
        debounce_secs: u64,
        max_retries: u8,
    ) -> Result<Vec<String>> {
        let cutoff = Utc::now() - chrono::Duration::seconds(debounce_secs as i64);
        let cutoff_str = cutoff.to_rfc3339();
        let max_retries_i = max_retries as i64;

        let sql = if let Some(l) = limit {
            format!(
                "SELECT DISTINCT node_id FROM embedding WHERE stale = 1 AND error_count < ?1 AND modified_at < ?2 LIMIT {}",
                l
            )
        } else {
            "SELECT DISTINCT node_id FROM embedding WHERE stale = 1 AND error_count < ?1 AND modified_at < ?2".to_string()
        };

        let mut rows = self
            .read()
            .await?
            .query(&sql, libsql::params![max_retries_i, cutoff_str])
            .await
            .context("Failed to get stale embedding root IDs")?;

        let mut ids = Vec::new();
        while let Some(row) = rows.next().await? {
            ids.push(row.get(0)?);
        }
        Ok(ids)
    }

    pub async fn has_pending_stale_embeddings(
        &self,
        debounce_secs: u64,
        max_retries: u8,
    ) -> Result<bool> {
        let cutoff = Utc::now() - chrono::Duration::seconds(debounce_secs as i64);
        let cutoff_str = cutoff.to_rfc3339();
        let max_retries_i = max_retries as i64;

        let mut rows = self.read().await?.query(
            "SELECT COUNT(*) FROM embedding WHERE stale = 1 AND error_count < ?1 AND modified_at >= ?2",
            libsql::params![max_retries_i, cutoff_str],
        ).await.context("Failed to check for pending stale embeddings")?;

        if let Some(row) = rows.next().await? {
            let count: i64 = row.get(0)?;
            Ok(count > 0)
        } else {
            Ok(false)
        }
    }

    pub async fn has_embeddings(&self, node_id: &str) -> Result<bool> {
        let mut rows = self
            .read()
            .await?
            .query(
                "SELECT COUNT(*) FROM embedding WHERE node_id = ?1",
                libsql::params![node_id.to_string()],
            )
            .await
            .context("Failed to check for embeddings")?;

        if let Some(row) = rows.next().await? {
            let count: i64 = row.get(0)?;
            Ok(count > 0)
        } else {
            Ok(false)
        }
    }

    pub async fn delete_embeddings(&self, node_id: &str) -> Result<()> {
        let db = self.write().await;
        let tx = db
            .transaction()
            .await
            .context("Failed to begin delete_embeddings transaction")?;

        // Clear the vec0 mirror first (keyed by embedding_id, so resolve via embedding).
        tx.execute(
            "DELETE FROM vec_embeddings WHERE embedding_id IN (SELECT id FROM embedding WHERE node_id = ?1)",
            libsql::params![node_id.to_string()],
        )
        .await
        .context("Failed to clear vec_embeddings for node")?;

        tx.execute(
            "DELETE FROM embedding WHERE node_id = ?1",
            libsql::params![node_id.to_string()],
        )
        .await
        .context("Failed to delete embeddings")?;

        tx.commit()
            .await
            .context("Failed to commit delete_embeddings transaction")?;
        Ok(())
    }

    pub async fn record_embedding_error(
        &self,
        node_id: &str,
        error: &str,
        max_retries: u8,
    ) -> Result<()> {
        let now = Utc::now().to_rfc3339();
        let max_retries_i = max_retries as i64;

        // Increment error_count, set last_error, clear stale if error_count reaches max_retries
        self.write().await.execute(
            "UPDATE embedding SET error_count = error_count + 1, last_error = ?1, modified_at = ?2, stale = CASE WHEN error_count + 1 >= ?3 THEN 0 ELSE stale END WHERE node_id = ?4",
            libsql::params![error.to_string(), now, max_retries_i, node_id.to_string()],
        ).await.context("Failed to record embedding error")?;
        Ok(())
    }

    pub async fn search_embeddings(
        &self,
        query_vector: &[f32],
        limit: i64,
        threshold: Option<f64>,
    ) -> Result<Vec<crate::models::EmbeddingSearchResult>> {
        let min_score = threshold.unwrap_or(0.5);

        // vec0 KNN over the compact vector store, then JOIN back to recover node_id /
        // total_chunks. vec0 holds only non-stale vectors (see upsert/delete/mark-stale),
        // so `e.stale = 0` is a cheap defensive guard. We over-fetch chunks because many
        // chunks map to one node and results are grouped per node.
        //
        // Note: with KNN, `matching_chunks` counts a node's chunks that landed in the
        // top-k near the query — not all of its chunks (as the old full scan did). So
        // `density` now genuinely measures "fraction of the node's chunks near the query"
        // rather than always being ~1.0; it still feeds the same composite formula.
        let query_blob: Vec<u8> = query_vector.iter().flat_map(|f| f.to_le_bytes()).collect();
        let k = (limit * EMBEDDING_KNN_OVERFETCH).max(limit);

        // Group by node_id: track max similarity, chunk counts
        let mut node_scores: HashMap<String, (f64, i64, i64)> = HashMap::new(); // node_id -> (max_sim, matching_chunks, total_chunks)

        let knn_start = std::time::Instant::now();

        // Scoped so the cursor drops before the batch node fetch below checks out a
        // second reader connection — see `ReadRows` in `connections.rs`.
        {
            let mut rows = self
                .read()
                .await?
                .query(
                    "SELECT e.node_id, e.total_chunks, v.distance \
                 FROM vec_embeddings v JOIN embedding e ON e.id = v.embedding_id \
                 WHERE v.vector MATCH ?1 AND k = ?2 AND e.stale = 0",
                    libsql::params![query_blob, k],
                )
                .await
                .context("Failed to run vec0 KNN search")?;

            while let Some(row) = rows.next().await? {
                let node_id: String = row.get(0)?;
                let total_chunks: i64 = row.get(1)?;
                let distance: f64 = row.get(2)?;
                // vec0 cosine distance_metric returns distance = 1 - cosine similarity
                let similarity = 1.0 - distance;

                let entry = node_scores.entry(node_id).or_insert((0.0, 0, total_chunks));
                if similarity > entry.0 {
                    entry.0 = similarity;
                }
                entry.1 += 1;
            }
        }

        let knn_time = knn_start.elapsed();
        let candidates = node_scores.len();

        // Score every candidate first, then rank and truncate to `limit` BEFORE any
        // node data is fetched. Two things matter here, and both used to be wrong:
        //
        //   1. The fetch is a single batched `IN (...)` query rather than one
        //      `get_node` per surviving candidate. Each `get_node` checks a reader
        //      connection out of the pool, and on a pool miss that *opens a new
        //      SQLite connection* — so a serial loop paid a connection checkout
        //      plus a round trip for every result.
        //   2. Only the `limit` rows actually returned are hydrated. KNN over-fetches
        //      by `EMBEDDING_KNN_OVERFETCH`, and callers inflate `limit` further for
        //      their own post-filters, so hydrating pre-truncation fetched node data
        //      that was then immediately discarded.
        let score_start = std::time::Instant::now();
        let mut scored = rank_candidates(node_scores, min_score);
        scored.truncate(limit as usize);
        let score_time = score_start.elapsed();

        let hydrate_start = std::time::Instant::now();
        let ids: Vec<String> = scored.iter().map(|(id, ..)| id.clone()).collect();
        let mut nodes = self.get_nodes_by_ids(&ids).await?;
        let hydrate_time = hydrate_start.elapsed();

        let results: Vec<crate::models::EmbeddingSearchResult> = scored
            .into_iter()
            .map(|(node_id, score, max_similarity, matching_chunks)| {
                let node = nodes.remove(&node_id);
                crate::models::EmbeddingSearchResult {
                    node_id,
                    score,
                    max_similarity,
                    matching_chunks,
                    node,
                }
            })
            .collect();

        tracing::debug!(
            knn_ms = knn_time.as_millis() as u64,
            score_ms = score_time.as_millis() as u64,
            hydrate_ms = hydrate_time.as_millis() as u64,
            k,
            candidates,
            returned = results.len(),
            "search_embeddings phases"
        );

        Ok(results)
    }

    pub async fn search_embeddings_by_node_type(
        &self,
        query_vector: &[f32],
        node_type: &str,
        limit: i64,
        threshold: Option<f64>,
    ) -> Result<Vec<crate::models::EmbeddingSearchResult>> {
        let min_score = threshold.unwrap_or(0.5);

        // Same vec0 KNN as `search_embeddings`, with the node-type filter folded into the
        // JOIN. The type filter is applied AFTER KNN, so use a larger over-fetch to keep
        // enough surviving candidates of the requested type.
        let query_blob: Vec<u8> = query_vector.iter().flat_map(|f| f.to_le_bytes()).collect();
        let k = (limit * EMBEDDING_KNN_OVERFETCH * 5).max(limit);

        let mut node_scores: HashMap<String, (f64, i64, i64)> = HashMap::new();

        // Scoped so the cursor drops before the per-result `get_node` calls below
        // check out a second reader connection — see `ReadRows` in `connections.rs`.
        {
            let mut rows = self
                .read()
                .await?
                .query(
                    "SELECT e.node_id, e.total_chunks, v.distance \
             FROM vec_embeddings v \
             JOIN embedding e ON e.id = v.embedding_id \
             JOIN node n ON n.id = e.node_id \
             WHERE v.vector MATCH ?1 AND k = ?2 AND e.stale = 0 AND n.node_type = ?3",
                    libsql::params![query_blob, k, node_type.to_string()],
                )
                .await
                .context("Failed to run typed vec0 KNN search")?;

            while let Some(row) = rows.next().await? {
                let node_id: String = row.get(0)?;
                let total_chunks: i64 = row.get(1)?;
                let distance: f64 = row.get(2)?;
                let similarity = 1.0 - distance;

                let entry = node_scores.entry(node_id).or_insert((0.0, 0, total_chunks));
                if similarity > entry.0 {
                    entry.0 = similarity;
                }
                entry.1 += 1;
            }
        }

        // Rank and truncate before hydrating, then fetch the surviving nodes in one
        // batched query — see `search_embeddings` for why a per-result `get_node` loop
        // is the expensive shape.
        let mut scored = rank_candidates(node_scores, min_score);

        // The node_type filter runs after the global top-k, so a type that is rare
        // relative to the corpus can be crowded out of the KNN window — surfacing as
        // fewer than `limit` results. Counted before truncation, so it reports how many
        // candidates actually cleared the threshold. Surface that as a debug signal
        // rather than failing silently; raising EMBEDDING_KNN_OVERFETCH is the lever if
        // recall suffers.
        if (scored.len() as i64) < limit {
            tracing::debug!(
                node_type,
                returned = scored.len(),
                limit,
                k,
                "typed embedding search returned fewer than `limit` results; node_type may be under-represented in the KNN window"
            );
        }

        scored.truncate(limit as usize);

        let ids: Vec<String> = scored.iter().map(|(id, ..)| id.clone()).collect();
        let mut nodes = self.get_nodes_by_ids(&ids).await?;

        Ok(scored
            .into_iter()
            .map(|(node_id, score, max_similarity, matching_chunks)| {
                let node = nodes.remove(&node_id);
                crate::models::EmbeddingSearchResult {
                    node_id,
                    score,
                    max_similarity,
                    matching_chunks,
                    node,
                }
            })
            .collect())
    }

    /// Keyword half of general search: bm25 over `node_title_fts`, best first.
    ///
    /// Only titled roots are in that index, so every hit is already a document —
    /// a child line has no title and cannot match. Body text is reachable through
    /// the root's embedding, not through this index.
    ///
    /// Schemas are left out. A schema's title is its type name ("Task",
    /// "Project"), a word that turns up in ordinary queries, so matching it
    /// would rank a type above the user's own documents on "project roadmap".
    /// A schema is still found by meaning, through its embedding.
    ///
    /// Returns `(node_id, bm25)` pairs in rank order. FTS5 bm25 is NEGATIVE, more
    /// negative = better, matching `resolve_entities_by_title`.
    pub async fn bm25_search_titles(
        &self,
        query: &str,
        candidate_limit: i64,
    ) -> Result<Vec<(String, f64)>> {
        let tokens: Vec<String> = query
            .split_whitespace()
            .map(|t| {
                t.trim_matches(|c: char| !c.is_alphanumeric())
                    .to_lowercase()
            })
            .filter(|t| !t.is_empty() && !BM25_STOP_WORDS.contains(&t.as_str()))
            .take(BM25_MAX_TOKENS)
            .collect();

        if tokens.is_empty() {
            return Ok(Vec::new());
        }

        // Build FTS5 query: "token1" OR "token2" OR ...
        let fts_query = tokens
            .iter()
            .map(|t| format!("\"{}\"", t.replace('"', "")))
            .collect::<Vec<_>>()
            .join(" OR ");

        let sql = format!(
            "SELECT f.id, bm25(node_title_fts) FROM node_title_fts f \
             JOIN node n ON n.id = f.id \
             WHERE node_title_fts MATCH ?1 AND n.node_type != 'schema' \
             ORDER BY rank LIMIT {}",
            candidate_limit
        );

        let mut rows = self
            .read()
            .await?
            .query(&sql, libsql::params![fts_query])
            .await
            .context("Failed to execute BM25 title search")?;

        let mut hits = Vec::new();
        while let Some(row) = rows.next().await? {
            hits.push((row.get(0)?, row.get(1)?));
        }
        Ok(hits)
    }

    pub async fn create_stale_embedding_marker(&self, node_id: &str) -> Result<()> {
        let id = uuid::Uuid::new_v4().to_string();
        let now = Utc::now().to_rfc3339();
        // Deliberately NOT mirrored into vec_embeddings: this is a stale (stale=1)
        // placeholder with a dummy vector and must never surface in KNN results.
        // Unit vector [1, 0, 0, ...] as 768×f32 LE bytes
        let mut vector_bytes = vec![0u8; 768 * 4];
        vector_bytes[0..4].copy_from_slice(&1.0f32.to_le_bytes());

        self.write().await.execute(
            "INSERT OR IGNORE INTO embedding (id, node_id, vector, dimension, model_name, chunk_index, chunk_start, chunk_end, total_chunks, content_hash, token_count, stale, error_count, last_error, created_at, modified_at) VALUES (?1, ?2, ?3, 768, 'nomic-embed-text-v1.5', 0, 0, NULL, 1, NULL, NULL, 1, 0, NULL, ?4, ?5)",
            libsql::params![id, node_id.to_string(), vector_bytes, now.clone(), now],
        ).await.context("Failed to create stale embedding marker")?;
        Ok(())
    }

    pub async fn create_stale_embedding_markers_bulk(&self, node_ids: &[String]) -> Result<usize> {
        if node_ids.is_empty() {
            return Ok(0);
        }

        let start = std::time::Instant::now();
        let now = Utc::now().to_rfc3339();
        // Stale placeholders are deliberately NOT mirrored into vec_embeddings (see
        // create_stale_embedding_marker) — they must never appear in KNN results.
        let mut vector_bytes = vec![0u8; 768 * 4];
        vector_bytes[0..4].copy_from_slice(&1.0f32.to_le_bytes());

        let db = self.write().await;
        let tx = db
            .transaction()
            .await
            .context("Failed to begin markers transaction")?;

        // Batch into multi-row INSERTs, chunked so each statement's bound
        // parameter count stays under SQLite's compiled SQLITE_MAX_VARIABLE_NUMBER (32766) (5 params/row).
        const ID_CHUNK: usize = 180;
        for chunk in node_ids.chunks(ID_CHUNK) {
            let placeholders: Vec<String> = (0..chunk.len())
                .map(|i| {
                    let base = i * 5;
                    format!(
                        "(?{}, ?{}, ?{}, 768, 'nomic-embed-text-v1.5', 0, 0, NULL, 1, NULL, NULL, 1, 0, NULL, ?{}, ?{})",
                        base + 1,
                        base + 2,
                        base + 3,
                        base + 4,
                        base + 5
                    )
                })
                .collect();
            let sql = format!(
                "INSERT OR IGNORE INTO embedding (id, node_id, vector, dimension, model_name, chunk_index, chunk_start, chunk_end, total_chunks, content_hash, token_count, stale, error_count, last_error, created_at, modified_at) VALUES {}",
                placeholders.join(", ")
            );

            let mut params: Vec<libsql::Value> = Vec::with_capacity(chunk.len() * 5);
            for node_id in chunk {
                params.push(libsql::Value::Text(uuid::Uuid::new_v4().to_string()));
                params.push(libsql::Value::Text(node_id.clone()));
                params.push(libsql::Value::Blob(vector_bytes.clone()));
                params.push(libsql::Value::Text(now.clone()));
                params.push(libsql::Value::Text(now.clone()));
            }

            tx.execute(&sql, params)
                .await
                .context("Failed to insert stale embedding markers batch")?;
        }
        tx.commit()
            .await
            .context("Failed to commit markers transaction")?;

        tracing::debug!(
            "create_stale_embedding_markers_bulk: {} markers in {:?}",
            node_ids.len(),
            start.elapsed()
        );

        Ok(node_ids.len())
    }
}

/// Score, threshold and rank embedding candidates.
///
/// Both `search_embeddings` and `search_embeddings_by_node_type` reduce their KNN
/// candidates the same way: a composite of peak chunk similarity and the share of
/// a node's chunks that matched, thresholded, then ranked best-first. They diverge
/// only in what they do next — the typed variant counts survivors before
/// truncating — so ranking is shared and truncation is not.
///
/// Returns `(node_id, composite_score, max_similarity, matching_chunks)`.
fn rank_candidates(
    node_scores: std::collections::HashMap<String, (f64, i64, i64)>,
    min_score: f64,
) -> Vec<(String, f64, f64, i64)> {
    let mut scored: Vec<(String, f64, f64, i64)> = node_scores
        .into_iter()
        .filter_map(
            |(node_id, (max_similarity, matching_chunks, total_chunks))| {
                let density = if total_chunks > 0 {
                    matching_chunks as f64 / total_chunks as f64
                } else {
                    1.0
                };
                let composite_score = max_similarity * (1.0 + 0.3 * density);
                (composite_score > min_score).then_some((
                    node_id,
                    composite_score,
                    max_similarity,
                    matching_chunks,
                ))
            },
        )
        .collect();

    scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    scored
}
