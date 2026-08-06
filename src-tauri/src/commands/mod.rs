//! The IPC surface.
//!
//! These functions deserialize, delegate, and serialize. Any real logic belongs
//! in `driver`, `store` or `sql` so it stays unit-testable without Tauri.

mod backup;
mod connection;
mod data;
pub(crate) mod library;
mod query;
mod schema;
mod transfer;

pub use backup::*;
pub use connection::*;
pub use data::*;
pub use library::*;
pub use query::*;
pub use schema::*;
pub use transfer::*;

use crate::registry::Registry;
use crate::store::Store;

/// Shared application state, injected by Tauri into every command.
pub struct AppState {
    pub store: Store,
    pub registry: Registry,
}
