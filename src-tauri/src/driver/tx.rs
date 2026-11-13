//! Applying guarded statements inside a transaction.
//!
//! The row-count safety net lives here so there is exactly one implementation
//! rather than one per engine that could drift apart.
//!
//! It is a macro rather than a generic function because `rows_affected` is an
//! inherent method on each engine's `QueryResult` — sqlx exposes no trait that
//! reaches it — so a generic bound cannot be written. The macro keeps the
//! logic in one place and expands it per pool type.

use crate::error::FaroError;

/// Generate an `apply_guarded` for one sqlx pool type.
macro_rules! impl_apply_guarded {
    ($name:ident, $pool:ty) => {
        /// Run every statement in one transaction, verifying affected row counts.
        ///
        /// Rolls back and returns an error if any statement touches a different
        /// number of rows than it declared. A dropped transaction also rolls
        /// back, so a failure partway through never leaves half a batch applied.
        pub async fn $name(
            pool: &$pool,
            statements: &[$crate::model::GuardedStatement],
        ) -> $crate::error::Result<u64> {
            if statements.is_empty() {
                return Ok(0);
            }

            let mut tx = pool.begin().await?;
            let mut total = 0u64;

            for (index, stmt) in statements.iter().enumerate() {
                // AssertSqlSafe: this SQL comes from `dml`, which quotes every
                // identifier against the table's real column list and escapes
                // every literal. No user text is spliced in raw.
                let result = crate::sqlx::query(crate::sqlx::AssertSqlSafe(stmt.sql.clone()))
                    .execute(&mut *tx)
                    .await?;

                let affected = result.rows_affected();
                if let Some(expected) = stmt.expect {
                    if affected != expected {
                        tx.rollback().await?;
                        return Err($crate::driver::tx::guard_error(
                            index,
                            Some(statements.len()),
                            expected,
                            affected,
                        ));
                    }
                }
                total += affected;
            }

            tx.commit().await?;
            Ok(total)
        }
    };
}

impl_apply_guarded!(apply_guarded_pg, crate::sqlx::PgPool);
impl_apply_guarded!(apply_guarded_mysql, crate::sqlx::MySqlPool);

/// Generate a streaming `apply_guarded` for one sqlx pool type.
///
/// The same transaction and the same row-count guard as above, but fed from a
/// channel so the caller never has to hold every statement at once.
macro_rules! impl_apply_streaming {
    ($name:ident, $pool:ty) => {
        pub async fn $name(
            pool: &$pool,
            mut chunks: tokio::sync::mpsc::Receiver<$crate::model::ApplyChunk>,
            cancel: tokio_util::sync::CancellationToken,
        ) -> $crate::error::Result<u64> {
            use crate::sqlx::Connection;
            use $crate::model::ApplyChunk;
            let mut conn = $crate::driver::pool::acquire(pool, &cancel).await?;
            let mut tx = conn.begin().await?;
            let work = async {
                let mut total = 0u64;
                let mut index = 0usize;
                while let Some(chunk) = chunks.recv().await {
                    match chunk {
                        ApplyChunk::Done => return Ok(total),
                        ApplyChunk::Batch(batch) => {
                            for stmt in batch {
                                let result =
                                    crate::sqlx::query(crate::sqlx::AssertSqlSafe(stmt.sql))
                                        .execute(&mut *tx)
                                        .await?;
                                let affected = result.rows_affected();
                                if let Some(expected) = stmt.expect {
                                    if affected != expected {
                                        return Err($crate::driver::tx::guard_error(
                                            index, None, expected, affected,
                                        ));
                                    }
                                }
                                total += affected;
                                index += 1;
                            }
                        }
                    }
                }
                Err($crate::error::FaroError::Other(
                    "the source could not be read to the end, so nothing was applied".into(),
                ))
            };
            let result = tokio::select! {
                biased;
                _ = cancel.cancelled() => Err($crate::error::FaroError::Cancelled),
                result = work => result,
            };
            match result {
                Ok(total) if !cancel.is_cancelled() => {
                    // Never drop an in-flight commit: once issued its outcome must be reported.
                    tx.commit().await?;
                    Ok(total)
                }
                result => {
                    tx.rollback().await?;
                    Err(result.err().unwrap_or($crate::error::FaroError::Cancelled))
                }
            }
        }
    };
}

impl_apply_streaming!(apply_streaming_pg, crate::sqlx::PgPool);
impl_apply_streaming!(apply_streaming_mysql, crate::sqlx::MySqlPool);

/// `count` is `None` when the statements are still arriving and the total is
/// not yet known, as it is while streaming an import.
pub(crate) fn guard_error(
    index: usize,
    count: Option<usize>,
    expected: u64,
    affected: u64,
) -> FaroError {
    if affected == 0 {
        FaroError::Conflict(guard_message(index, count, expected, affected))
    } else {
        FaroError::Other(guard_message(index, count, expected, affected))
    }
}

/// Explain a guard failure in terms of what actually went wrong, since
/// "expected 1, got 0" means something quite different from "got 3".
fn guard_message(index: usize, count: Option<usize>, expected: u64, affected: u64) -> String {
    let position = match count {
        Some(count) if count > 1 => format!(" (change {} of {})", index + 1, count),
        // Streaming: the total is still unknown, so name the position only.
        None => format!(" (change {})", index + 1),
        _ => String::new(),
    };

    if affected == 0 {
        format!(
            "Nothing was changed{position}: the row no longer matches. \
             Someone may have edited or deleted it since this page was loaded. \
             Nothing has been saved — refresh and try again."
        )
    } else {
        format!(
            "Refusing to save{position}: this change would have affected {affected} rows \
             instead of {expected}. The primary key does not identify a single row. \
             Nothing has been saved."
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_vanished_row_is_explained_as_such() {
        let msg = guard_message(0, Some(1), 1, 0);
        assert!(msg.contains("no longer matches"), "{msg}");
        assert!(msg.contains("Nothing has been saved"), "{msg}");
    }

    #[test]
    fn too_many_rows_names_the_real_cause() {
        let msg = guard_message(0, Some(1), 1, 4);
        assert!(msg.contains("4 rows"), "{msg}");
        assert!(msg.contains("does not identify a single row"), "{msg}");
        assert!(msg.contains("Nothing has been saved"), "{msg}");
    }

    #[test]
    fn a_batch_says_which_change_failed() {
        let msg = guard_message(2, Some(5), 1, 0);
        assert!(msg.contains("change 3 of 5"), "{msg}");
    }

    #[test]
    fn a_single_change_omits_the_position() {
        let msg = guard_message(0, Some(1), 1, 0);
        assert!(!msg.contains("change 1"), "{msg}");
    }
}
