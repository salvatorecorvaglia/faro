//! The "one connection for user SQL, one for catalog reads" pair every
//! sqlx-backed driver opens.
//!
//! Repeated identically in the Postgres, MySQL and SQLite drivers before this
//! existed — same reasoning, same five lines, three copies. A generic
//! function rather than a macro (unlike `fetch.rs`/`tx.rs`) because building a
//! pool genuinely is generic over `sqlx::Database`; nothing here needs a
//! per-engine row-decode function the way reading a result set does.

use sqlx::{Connection, Database, Pool};

use crate::error::{FaroError, Result};

type ConnectOptions<DB> = <<DB as Database>::Connection as Connection>::Options;

/// Open both connections from the same `options`.
///
/// `acquire_timeout`, when set, fails fast on a wrong host or an unreachable
/// server rather than making the user wait out the OS-level TCP timeout —
/// worth it for a networked engine, not for a local SQLite file.
///
/// Each pool gets exactly one connection. Temp tables, `SET`/session
/// variables, and open transactions all live on a single connection; a
/// larger pool would scatter a session's statements across connections and
/// fail with something like "relation does not exist" for no reason visible
/// to the user. Splitting catalog reads onto their own connection means
/// schema browsing and autocomplete stay responsive while a long user query
/// runs on the other one.
pub(crate) async fn dual_pool<DB>(
    options: ConnectOptions<DB>,
    acquire_timeout: Option<std::time::Duration>,
) -> Result<(Pool<DB>, Pool<DB>)>
where
    DB: Database,
    ConnectOptions<DB>: Clone,
{
    let pool = one::<DB>(options.clone(), acquire_timeout).await?;
    let meta = one::<DB>(options, acquire_timeout).await?;
    Ok((pool, meta))
}

async fn one<DB: Database>(
    options: ConnectOptions<DB>,
    acquire_timeout: Option<std::time::Duration>,
) -> Result<Pool<DB>> {
    let mut builder = sqlx::pool::PoolOptions::<DB>::new().max_connections(1);
    if let Some(timeout) = acquire_timeout {
        builder = builder.acquire_timeout(timeout);
    }
    builder
        .connect_with(options)
        .await
        .map_err(|e| FaroError::Connection(e.to_string()))
}
