//! `SqliteStore` methods — search concern (split from the god-object per ADR-053 prep).
use super::*;

impl SqliteStore {
    pub async fn mention_autocomplete(
        &self,
        search_query: &str,
        limit: Option<i64>,
    ) -> Result<Vec<Node>> {
        let effective_limit = limit.unwrap_or(10);
        // Escaped so a `%` or `_` typed into the mention picker matches
        // literally instead of acting as a LIKE wildcard — see
        // `SqliteStore::like_contains_pattern`.
        let search_lower = Self::like_contains_pattern(search_query);
        // Which types the picker leaves out is the registry's `mentionable`
        // rule, and it follows `extends`: a subtype of an unmentionable type
        // is left out too. A schema is titled by its type name, but a type is
        // not something to @mention; an ai-chat can never be a mention target
        // (ADR-061 §8). Date pages stay: a date link is a real mention. An
        // archived node participates in nothing (ADR-087 §2), so only active
        // nodes are offered.
        let mentionable = crate::db::schema::is_not_a_sql(
            "node_type",
            &crate::models::CoreNodeType::not_mentionable(),
        );
        let sql = format!(
            "SELECT * FROM node WHERE title IS NOT NULL AND lifecycle_status = 'active' AND {mentionable} AND LOWER(title) LIKE ?1 ESCAPE '\\' LIMIT {}",
            effective_limit
        );
        self.query_nodes_from_sql(&sql, libsql::params![search_lower])
            .await
    }
}
