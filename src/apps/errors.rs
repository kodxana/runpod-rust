//! Errors specific to application definitions and remote function execution.

/// Application-level failures with native Rust variants for the Python exception hierarchy.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum AppError {
    /// A named application resource has no deployed endpoint.
    #[error(
        "'{resource_name}' in app '{app_name}' is not deployed. run `rp flash deploy` first, or use .local() to run it here."
    )]
    EndpointNotFound {
        /// Application routing name.
        app_name: String,
        /// Resource routing name.
        resource_name: String,
    },
    /// A remote function or its transport envelope reported an execution failure.
    #[error("{0}")]
    RemoteExecution(String),
    /// Recurring schedules remain unsupported by the upstream platform.
    #[error(
        "recurring schedules are not yet supported by the runpod api. the @schedule decorator records intent but cannot be deployed."
    )]
    ScheduleNotSupported,
    /// An application resource has invalid configuration or an unsupported target.
    #[error("{0}")]
    InvalidResource(String),
}
