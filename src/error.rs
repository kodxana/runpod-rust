//! Structured errors shared by SDK transports and operations.
use crate::Value;

/// Result of an SDK operation.
pub type Result<T> = std::result::Result<T, Error>;

/// An actionable authentication, API, transport, or local validation failure.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    /// Application routing, configuration, or remote execution failed.
    #[error(transparent)]
    App(#[from] crate::apps::AppError),
    /// A Python-compatible built-in value could not be serialized or decoded.
    #[error(transparent)]
    Pickle(#[from] serde_pickle::Error),
    /// A serialized result contained invalid base64.
    #[error(transparent)]
    Base64(#[from] base64::DecodeError),
    /// An operation failed with additional context while retaining its original cause.
    #[error("{message}: {source}")]
    Context {
        /// Description of the operation that could not complete.
        message: String,
        /// Underlying SDK failure, available through std::error::Error::source.
        source: Box<Error>,
    },
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
        errors: Box<Value>,
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
    /// An Apps operation exceeded its deadline, including operation-specific context.
    #[error("{0}")]
    DeadlineExceeded(String),
}

impl Error {
    /// Returns the HTTP status carried by a query or transport error.
    #[must_use]
    pub fn status_code(&self) -> Option<u16> {
        match self {
            Self::Context { source, .. } => source.status_code(),
            Self::Query { status_code, .. } => *status_code,
            Self::Http(error) => error.status().map(|status| status.as_u16()),
            Self::Authentication(_) => Some(401),
            _ => None,
        }
    }
}
