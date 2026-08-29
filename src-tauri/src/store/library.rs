//! Saved queries and query history.

use rusqlite::{params, OptionalExtension};

use super::{map_saved_query, Store};
use crate::error::Result;
use crate::model::{HistoryEntry, NewHistoryEntry, SavedQuery};

impl Store {
    // -- Saved queries -----------------------------------------------------

    /// All saved queries: foldered ones first grouped by folder, then loose
    /// ones, each alphabetical. Matches how file trees group, and keeps the
    /// sidebar order stable regardless of when things were created.
    ///
    /// `folder IS NULL` sorts 0 before 1, which is what puts foldered rows
    /// ahead of unfoldered ones.
    pub fn list_saved_queries(&self) -> Result<Vec<SavedQuery>> {
        let conn = self.lock();
        let mut stmt = conn.prepare(
            "SELECT id, name, folder, sql, connection_id, created_at, updated_at
             FROM saved_queries
             ORDER BY folder IS NULL, folder COLLATE NOCASE, name COLLATE NOCASE",
        )?;
        let rows = stmt.query_map([], map_saved_query)?;
        Ok(rows.collect::<std::result::Result<Vec<_>, _>>()?)
    }

    pub fn get_saved_query(&self, id: &str) -> Result<Option<SavedQuery>> {
        let conn = self.lock();
        let mut stmt = conn.prepare(
            "SELECT id, name, folder, sql, connection_id, created_at, updated_at
             FROM saved_queries WHERE id = ?1",
        )?;
        Ok(stmt.query_row(params![id], map_saved_query).optional()?)
    }

    /// Insert or update. `updated_at` moves on every write; `created_at` is
    /// preserved so the original save time survives an edit.
    pub fn upsert_saved_query(&self, q: &SavedQuery) -> Result<()> {
        let conn = self.lock();
        conn.execute(
            r#"
            INSERT INTO saved_queries (id, name, folder, sql, connection_id)
            VALUES (?1, ?2, ?3, ?4, ?5)
            ON CONFLICT(id) DO UPDATE SET
                name = excluded.name,
                folder = excluded.folder,
                sql = excluded.sql,
                connection_id = excluded.connection_id,
                updated_at = datetime('now')
            "#,
            params![q.id, q.name, q.folder, q.sql, q.connection_id],
        )?;
        Ok(())
    }

    pub fn delete_saved_query(&self, id: &str) -> Result<()> {
        self.lock()
            .execute("DELETE FROM saved_queries WHERE id = ?1", params![id])?;
        Ok(())
    }

    // -- History -----------------------------------------------------------

    /// Record one execution. Returns the new row id.
    pub fn add_history(&self, entry: &NewHistoryEntry) -> Result<i64> {
        let conn = self.lock();
        conn.execute(
            r#"
            INSERT INTO query_history
                (sql, connection_id, connection_name, duration_ms, row_count, error, succeeded)
            VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
            "#,
            params![
                entry.sql,
                entry.connection_id,
                entry.connection_name,
                entry.duration_ms,
                entry.row_count,
                entry.error,
                entry.error.is_none() as i64,
            ],
        )?;
        Ok(conn.last_insert_rowid())
    }

    /// Most recent first, optionally narrowed by a substring of the SQL.
    ///
    /// `search` matches the SQL text only. Matching timestamps or durations
    /// would surprise more than it would help.
    pub fn list_history(&self, search: Option<&str>, limit: i64) -> Result<Vec<HistoryEntry>> {
        let conn = self.lock();
        // The term is escaped and the escape character declared, so searching
        // for `50%` finds the literal text rather than everything starting
        // with 50. SQLite has no default LIKE escape, so the clause is not
        // optional here. Same convention as the browse filters, which reuse
        // this escaper — the two searches behaving differently was the bug.
        let pattern = search.map(crate::sql::escape_like);
        let mut stmt = conn.prepare(
            r#"
            SELECT id, sql, connection_id, connection_name, executed_at,
                   duration_ms, row_count, error, succeeded
            FROM query_history
            WHERE (?1 IS NULL OR sql LIKE '%' || ?1 || '%' ESCAPE '\')
            ORDER BY id DESC
            LIMIT ?2
            "#,
        )?;
        let rows = stmt.query_map(params![pattern, limit], |r| {
            Ok(HistoryEntry {
                id: r.get("id")?,
                sql: r.get("sql")?,
                connection_id: r.get("connection_id")?,
                connection_name: r.get("connection_name")?,
                executed_at: r.get("executed_at")?,
                duration_ms: r.get("duration_ms")?,
                row_count: r.get("row_count")?,
                error: r.get("error")?,
                succeeded: r.get::<_, i64>("succeeded")? != 0,
            })
        })?;
        Ok(rows.collect::<std::result::Result<Vec<_>, _>>()?)
    }

    pub fn clear_history(&self) -> Result<()> {
        self.lock().execute("DELETE FROM query_history", [])?;
        Ok(())
    }

    pub fn delete_history_entry(&self, id: i64) -> Result<()> {
        self.lock()
            .execute("DELETE FROM query_history WHERE id = ?1", params![id])?;
        Ok(())
    }

    /// Drop the oldest rows beyond `keep`.
    ///
    /// History is written on every run, so without a cap it grows without
    /// bound for the lifetime of the install.
    pub fn prune_history(&self, keep: i64) -> Result<usize> {
        let conn = self.lock();
        Ok(conn.execute(
            "DELETE FROM query_history WHERE id NOT IN (
                 SELECT id FROM query_history ORDER BY id DESC LIMIT ?1
             )",
            params![keep],
        )?)
    }
}
