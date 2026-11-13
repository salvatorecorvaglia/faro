//! The SQLx API used by Faro's network drivers. Linking the engine crates
//! directly keeps SQLx's optional SQLite dependency out of Cargo's resolver.
//! Local SQLite sessions and settings use the same rusqlite native library.
pub use sqlx_core::column::Column;
pub use sqlx_core::connection::Connection;
pub use sqlx_core::database::Database;
pub use sqlx_core::error::Error;
pub use sqlx_core::executor::Executor;
pub use sqlx_core::pool::{self, Pool};
pub use sqlx_core::query::query;
pub use sqlx_core::query_scalar::query_scalar;
pub use sqlx_core::raw_sql::raw_sql;
pub use sqlx_core::row::Row;
pub use sqlx_core::sql_str::{AssertSqlSafe, SqlSafeStr};
pub use sqlx_core::statement::Statement;
pub use sqlx_core::type_info::TypeInfo;
pub use sqlx_core::value::ValueRef;
pub use sqlx_mysql::{self as mysql, MySql, MySqlPool};
pub use sqlx_postgres::{self as postgres, PgPool, Postgres};
