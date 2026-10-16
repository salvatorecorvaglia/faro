//! Recent query and page results, held so they can be exported by id.
//!
//! Exporting a query's results used to send the rows back from the UI: the
//! frontend held every row and posted the whole result set to `export_result`.
//! That made the UI's copy the source of truth for an export — so anything it
//! did not hold exactly (large binary values, which it receives only as a size)
//! could not be exported faithfully, and every export pushed up to the whole
//! result back across IPC. Now the backend keeps what it sent and exports from
//! that.
//!
//! Shared rather than copied: each entry is the same `Arc` the command
//! returned, so caching costs no second copy of the rows.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use crate::commands::RunResult;
use crate::model::{QueryOutcome, ResultSet};

/// Results kept at once. Enough for the latest run in each of several tabs.
const MAX_ENTRIES: usize = 16;

/// Total rows kept across all entries. A run returns at most 100 000 rows, so
/// this holds several large results or many ordinary ones, and bounds memory
/// either way.
const MAX_ROWS: usize = 500_000;

/// One cached result: a query tab's run, or a table tab's page.
///
/// Cloning copies the `Arc`, not the rows.
#[derive(Clone)]
pub enum Cached {
    Run(Arc<RunResult>),
    Page(Arc<ResultSet>),
}

impl Cached {
    /// The result set of one statement — always statement 0 for a page.
    pub fn result_set(&self, statement: usize) -> Option<&ResultSet> {
        match self {
            Cached::Run(run) => match &run.statements.get(statement)?.outcome {
                Some(QueryOutcome::Rows(rs)) => Some(rs),
                _ => None,
            },
            Cached::Page(page) => (statement == 0).then_some(page.as_ref()),
        }
    }

    fn rows(&self) -> usize {
        match self {
            Cached::Run(run) => run
                .statements
                .iter()
                .filter_map(|s| match &s.outcome {
                    Some(QueryOutcome::Rows(rs)) => Some(rs.rows.len()),
                    _ => None,
                })
                .sum(),
            Cached::Page(page) => page.rows.len(),
        }
    }
}

#[derive(Default)]
pub struct ResultCache {
    /// Oldest first.
    entries: Mutex<VecDeque<(String, Cached)>>,
}

impl ResultCache {
    pub fn new() -> Self {
        Self::default()
    }

    /// Keep a result under its query id, evicting the oldest past the bounds.
    pub fn insert(&self, query_id: String, result: Cached) {
        let mut entries = self.lock();
        entries.retain(|(id, _)| id != &query_id);
        entries.push_back((query_id, result));
        let mut rows: usize = entries.iter().map(|(_, c)| c.rows()).sum();
        // Always keep the newest entry, however large: it is the one the user
        // is looking at and most likely to export.
        while entries.len() > 1 && (entries.len() > MAX_ENTRIES || rows > MAX_ROWS) {
            if let Some((_, evicted)) = entries.pop_front() {
                rows -= evicted.rows();
            }
        }
    }

    /// The result kept under `query_id`, if it is still held.
    ///
    /// Returned as its own handle so a slow export does not hold the lock —
    /// and block the next query from being cached — while it writes.
    pub fn get(&self, query_id: &str) -> Option<Cached> {
        self.lock()
            .iter()
            .find(|(id, _)| id == query_id)
            .map(|(_, cached)| cached.clone())
    }

    /// Recovering from poisoning: the deque is never left half-updated.
    fn lock(&self) -> std::sync::MutexGuard<'_, VecDeque<(String, Cached)>> {
        self.entries.lock().unwrap_or_else(|e| e.into_inner())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Value;

    fn page(rows: usize) -> Cached {
        Cached::Page(Arc::new(ResultSet {
            columns: vec![],
            rows: vec![vec![Value::Int(1)]; rows],
            truncated: false,
            elapsed_ms: 0,
        }))
    }

    fn held(cache: &ResultCache, id: &str) -> Option<usize> {
        cache.get(id)?.result_set(0).map(|rs| rs.rows.len())
    }

    #[test]
    fn a_result_is_found_again_by_its_query_id() {
        let cache = ResultCache::new();
        cache.insert("q1".into(), page(3));
        assert_eq!(held(&cache, "q1"), Some(3));
        assert_eq!(held(&cache, "missing"), None);
        // A page has only statement 0.
        assert!(cache.get("q1").unwrap().result_set(1).is_none());
    }

    #[test]
    fn the_oldest_results_go_first_once_the_entry_cap_is_reached() {
        let cache = ResultCache::new();
        for i in 0..=MAX_ENTRIES {
            cache.insert(format!("q{i}"), page(1));
        }
        assert_eq!(held(&cache, "q0"), None, "the oldest should be evicted");
        assert_eq!(held(&cache, &format!("q{MAX_ENTRIES}")), Some(1));
    }

    #[test]
    fn the_row_cap_bounds_memory_but_never_evicts_the_newest() {
        let cache = ResultCache::new();
        cache.insert("old".into(), page(MAX_ROWS / 2 + 1));
        cache.insert("new".into(), page(MAX_ROWS / 2 + 1));
        assert_eq!(held(&cache, "old"), None);
        assert_eq!(held(&cache, "new"), Some(MAX_ROWS / 2 + 1));

        // A single result larger than the cap is still kept.
        cache.insert("huge".into(), page(MAX_ROWS + 1));
        assert_eq!(held(&cache, "huge"), Some(MAX_ROWS + 1));
    }

    #[test]
    fn re_running_under_the_same_id_replaces_the_entry() {
        let cache = ResultCache::new();
        cache.insert("q".into(), page(1));
        cache.insert("q".into(), page(2));
        assert_eq!(held(&cache, "q"), Some(2));
    }
}
