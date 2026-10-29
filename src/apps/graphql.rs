//! Pinned upstream GraphQL documents (MIT; see LICENSE-UPSTREAM).

/// Upstream query cpu stock operation.
pub const QUERY_CPU_STOCK: &str = include_str!("graphql/query_cpu_stock.graphql");

/// Upstream query flash app by name operation.
pub const QUERY_FLASH_APP_BY_NAME: &str = include_str!("graphql/query_flash_app_by_name.graphql");

/// Upstream query secrets operation.
pub const QUERY_SECRETS: &str = include_str!("graphql/query_secrets.graphql");

/// Upstream query flash apps operation.
pub const QUERY_FLASH_APPS: &str = include_str!("graphql/query_flash_apps.graphql");

/// Upstream query flash environment by name operation.
pub const QUERY_FLASH_ENVIRONMENT_BY_NAME: &str =
    include_str!("graphql/query_flash_environment_by_name.graphql");

/// Upstream query flash auth request status operation.
pub const QUERY_FLASH_AUTH_REQUEST_STATUS: &str =
    include_str!("graphql/query_flash_auth_request_status.graphql");

/// Upstream mutation save endpoint operation.
pub const MUTATION_SAVE_ENDPOINT: &str = include_str!("graphql/mutation_save_endpoint.graphql");

/// Upstream mutation deploy cpu pod operation.
pub const MUTATION_DEPLOY_CPU_POD: &str = include_str!("graphql/mutation_deploy_cpu_pod.graphql");

/// Upstream mutation deploy pod operation.
pub const MUTATION_DEPLOY_POD: &str = include_str!("graphql/mutation_deploy_pod.graphql");

/// Upstream mutation create flash app operation.
pub const MUTATION_CREATE_FLASH_APP: &str =
    include_str!("graphql/mutation_create_flash_app.graphql");

/// Upstream mutation create flash environment operation.
pub const MUTATION_CREATE_FLASH_ENVIRONMENT: &str =
    include_str!("graphql/mutation_create_flash_environment.graphql");

/// Upstream mutation create secret operation.
pub const MUTATION_CREATE_SECRET: &str = include_str!("graphql/mutation_create_secret.graphql");

/// Upstream mutation delete secret operation.
pub const MUTATION_DELETE_SECRET: &str = include_str!("graphql/mutation_delete_secret.graphql");

/// Upstream mutation delete flash app operation.
pub const MUTATION_DELETE_FLASH_APP: &str =
    include_str!("graphql/mutation_delete_flash_app.graphql");

/// Upstream mutation delete flash environment operation.
pub const MUTATION_DELETE_FLASH_ENVIRONMENT: &str =
    include_str!("graphql/mutation_delete_flash_environment.graphql");

/// Upstream mutation create flash auth request operation.
pub const MUTATION_CREATE_FLASH_AUTH_REQUEST: &str =
    include_str!("graphql/mutation_create_flash_auth_request.graphql");

/// Upstream mutation prepare artifact upload operation.
pub const MUTATION_PREPARE_ARTIFACT_UPLOAD: &str =
    include_str!("graphql/mutation_prepare_artifact_upload.graphql");

/// Upstream mutation finalize artifact upload operation.
pub const MUTATION_FINALIZE_ARTIFACT_UPLOAD: &str =
    include_str!("graphql/mutation_finalize_artifact_upload.graphql");

/// Upstream mutation deploy build operation.
pub const MUTATION_DEPLOY_BUILD: &str = include_str!("graphql/mutation_deploy_build.graphql");
