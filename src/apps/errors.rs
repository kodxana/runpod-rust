//! Errors specific to application definitions and remote function execution.

/// Application-level failures with native Rust variants for the Python exception hierarchy.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum AppError {
    /// Browser approval or email verification failed, without including credentials or response bodies.
    #[error("{0}")]
    Login(String),
    /// Development endpoint deletions that failed and remain tracked for a later retry.
    #[error("dev cleanup incomplete: {}", .0.join("; "))]
    DevCleanup(Vec<String>),
    /// No application matched its logical name.
    #[error("no app named '{0}' found. run `rp flash deploy` to create one.")]
    AppNotFound(String),
    /// An application or its requested environment was absent.
    #[error("no environment '{environment_name}' in app '{app_name}'.")]
    EnvironmentNotFound {
        /// Application routing name.
        app_name: String,
        /// Requested environment name.
        environment_name: String,
    },
    /// Execution was attempted while collecting declarations for discovery.
    #[error(
        "'{0}' was invoked during discovery. move calls into a function or local entrypoint so collecting declarations has no side effects."
    )]
    DiscoveryInvocation(String),
    /// A panic poisoned shared application registration or routing state.
    #[error("application state lock was poisoned: {0}")]
    StatePoisoned(&'static str),
    /// Dependency installation or artifact environment construction failed.
    #[error("{0}")]
    Build(String),
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
