//! Structured errors shared by SDK transports and operations.
use crate::Value;

/// Result of an SDK operation.
pub type Result<T> = std::result::Result<T, Error>;

/// An actionable authentication, API, transport, or local validation failure.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    /// Credentials were missing or rejected.
    #[error("{0}")]
    Authentication(String),
    /// The API rejected a query or returned a malformed page.
    #[error("{message}")]
    Query {
        /// Server-provided explanation.
        message: String,
        /// HTTP operation or GraphQL query that failed.
        query: String,
        /// HTTP status, when provided by the server.
        status_code: Option<u16>,
        /// Structured validation details, without discarding unknown fields.
        errors: Value,
    },
    /// Local argument validation failed before the request was sent.
    #[error("{0}")]
    InvalidInput(String),
    /// A server response did not contain the expected field or type.
    #[error("invalid response: {0}")]
    InvalidResponse(String),
    /// A request could not be delivered or read.
    #[error(transparent)]
    Http(#[from] reqwest::Error),
    /// A JSON value could not be decoded or encoded.
    #[error(transparent)]
    Json(#[from] serde_json::Error),
    /// A local file or process operation failed.
    #[error(transparent)]
    Io(#[from] std::io::Error),
    /// A job exceeded the caller's waiting deadline.
    #[error("Job timed out.")]
    Timeout,
}

impl Error {
    /// Returns the HTTP status carried by a query or transport error.
    #[must_use]
    pub fn status_code(&self) -> Option<u16> {
        match self {
            Self::Query { status_code, .. } => *status_code,
            Self::Http(error) => error.status().map(|status| status.as_u16()),
            Self::Authentication(_) => Some(401),
            _ => None,
        }
    }
}
