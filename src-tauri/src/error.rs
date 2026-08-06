use serde::Serialize;

pub type Result<T> = std::result::Result<T, FaroError>;

/// Errors surfaced to the UI.
///
/// These serialize to a tagged JSON object rather than a bare string so the
/// frontend can react differently to, say, a bad password versus a syntax
/// error, and so a database's own message survives intact.
#[derive(Debug, thiserror::Error)]
pub enum FaroError {
    #[error("could not connect: {0}")]
    Connection(String),

    /// A database-reported error. `message` is the engine's own words, which
    /// are almost always more useful than anything Faro could paraphrase.
    #[error("{message}")]
    Database { message: String, code: Option<String> },

    #[error("no connection is open with id {0}")]
    NotConnected(String),

    #[error("query cancelled")]
    Cancelled,

    #[error("{0} support is not implemented yet")]
    UnsupportedEngine(String),

    #[error("could not read or write saved settings: {0}")]
    Store(String),

    #[error("could not reach the system keychain: {0}")]
    Keychain(String),

    #[error("invalid SQL: {0}")]
    Sql(String),

    #[error("{0}")]
    Io(String),

    #[error("{0}")]
    Other(String),
}

impl FaroError {
    /// Short machine-readable discriminant for the frontend.
    fn kind(&self) -> &'static str {
        match self {
            FaroError::Connection(_) => "connection",
            FaroError::Database { .. } => "database",
            FaroError::NotConnected(_) => "notConnected",
            FaroError::Cancelled => "cancelled",
            FaroError::UnsupportedEngine(_) => "unsupportedEngine",
            FaroError::Store(_) => "store",
            FaroError::Keychain(_) => "keychain",
            FaroError::Sql(_) => "sql",
            FaroError::Io(_) => "io",
            FaroError::Other(_) => "other",
        }
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct SerializedError {
    kind: &'static str,
    message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    code: Option<String>,
}

impl Serialize for FaroError {
    fn serialize<S: serde::Serializer>(&self, s: S) -> std::result::Result<S::Ok, S::Error> {
        let code = match self {
            FaroError::Database { code, .. } => code.clone(),
            _ => None,
        };
        SerializedError { kind: self.kind(), message: self.to_string(), code }.serialize(s)
    }
}

impl From<sqlx::Error> for FaroError {
    fn from(e: sqlx::Error) -> Self {
        match &e {
            // Preserve SQLSTATE so the UI can tell "relation does not exist"
            // from "permission denied" without string matching.
            sqlx::Error::Database(db) => FaroError::Database {
                message: db.message().to_string(),
                code: db.code().map(|c| c.to_string()),
            },
            sqlx::Error::PoolTimedOut => {
                FaroError::Connection("timed out waiting for a free connection".into())
            }
            sqlx::Error::Io(io) => FaroError::Connection(io.to_string()),
            _ => FaroError::Database { message: e.to_string(), code: None },
        }
    }
}

impl From<rusqlite::Error> for FaroError {
    fn from(e: rusqlite::Error) -> Self {
        FaroError::Store(e.to_string())
    }
}

impl From<keyring::Error> for FaroError {
    fn from(e: keyring::Error) -> Self {
        FaroError::Keychain(e.to_string())
    }
}

impl From<std::io::Error> for FaroError {
    fn from(e: std::io::Error) -> Self {
        FaroError::Io(e.to_string())
    }
}

impl From<serde_json::Error> for FaroError {
    fn from(e: serde_json::Error) -> Self {
        FaroError::Other(e.to_string())
    }
}
