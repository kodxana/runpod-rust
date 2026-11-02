//! Deployment of compiled queue/API workers through the existing artifact contract.
use super::{
    AppsApiClient, ResourceKind,
    deploy::{AppDefinition, DeployResult, DeploymentConfig, deploy_prepared},
    events::EventSink,
    native_artifact::{native_manifest, prepare_native_artifact},
};
use crate::{Client, Error, Result};
use std::{
    fs::File,
    io::{self, Read},
    path::Path,
};

fn validate_target(executable: &Path) -> Result<()> {
    let invalid = || {
        Error::InvalidInput(format!(
            "native deployment requires a Linux x86_64 ELF executable: {}",
            executable.display()
        ))
    };
    if !std::fs::metadata(executable)?.is_file() {
        return Err(invalid());
    }
    let mut file = File::open(executable)?;
    let mut header = [0; 64];
    if let Err(error) = file.read_exact(&mut header) {
        return Err(if error.kind() == io::ErrorKind::UnexpectedEof {
            invalid()
        } else {
            error.into()
        });
    }
    if &header[..7] != b"\x7fELF\x02\x01\x01"
        || !matches!(header[7], 0 | 3)
        || !matches!(u16::from_le_bytes([header[16], header[17]]), 2 | 3)
        || u16::from_le_bytes([header[18], header[19]]) != 62
        || header[20..24] != 1_u32.to_le_bytes()
        || header[52..54] != 64_u16.to_le_bytes()
    {
        return Err(invalid());
    }
    Ok(())
}

/// Packages, uploads and deploys an already-compiled native queue/API worker.
///
/// The binary must call [`super::bootstrap::start_deployed`] and register this app's symbols.
/// A Linux `x86_64` ELF header is required; this is a target check, not a full ELF or dynamic
/// library validator. Compile against the selected image's libraries and keep inputs stable
/// during packaging. The built-in image family uses the existing default Python-version tag
/// as an OS carrier; its interpreter is not invoked. The coordinator manifest retains that
/// image-version field. A custom image needs `sh`, `tar` with gzip/stdout extraction, `chmod`,
/// `mkdir`, `sleep`, and a command-compatible entrypoint, plus the binary's runtime libraries.
///
/// The generated launcher selects a prebuilt tree, warm worker, or mounted artifact. Native
/// task deployment is rejected before mutations: coordinator task launch wiring is separate.
/// Explicit secrets are validated before packaging. Local work runs on the blocking pool;
/// cancellation prevents subsequent cloud calls but does not interrupt ongoing packaging.
/// Completed cloud mutations are not rolled back after later failure, as with `deploy_app`.
/// An absent/empty environment selects the app default. No compiler or Python installer runs.
///
/// # Errors
/// Returns invalid resource/symbol/target, secret, packaging, upload, activation or endpoint
/// reconciliation errors. Target-header validation cannot guarantee container execution.
pub async fn deploy_native_app(
    client: &Client,
    app: &AppDefinition,
    project_root: &Path,
    executable: &Path,
    environment: Option<&str>,
    events: Option<&EventSink>,
) -> Result<DeployResult> {
    native_manifest(app, None)?;
    if app
        .resources
        .iter()
        .any(|resource| resource.spec.kind == ResourceKind::Task)
    {
        return Err(Error::InvalidInput(
            "native task deployment requires coordinator task launch support; deploy queue/API resources separately until that path is implemented".into()
        ));
    }
    let api = AppsApiClient::new(client.clone())?;
    let referenced = app
        .resources
        .iter()
        .flat_map(|r| r.referenced_secrets.iter().cloned())
        .collect::<Vec<_>>();
    super::secret::validate_secrets(&referenced, &api).await?;
    let config = DeploymentConfig::native(client);
    let owned_app = app.clone();
    let root = project_root.to_owned();
    let executable = executable.to_owned();
    let image_version = config.python_version.clone();
    let build_events = events.cloned();
    let prepared = tokio::task::spawn_blocking(move || {
        validate_target(&executable)?;
        prepare_native_artifact(
            &owned_app,
            &root,
            &executable,
            None,
            Some(&image_version),
            build_events.as_ref(),
        )
    })
    .await??;
    deploy_prepared(
        &api,
        app,
        prepared,
        environment
            .filter(|name| !name.is_empty())
            .unwrap_or(&app.environment),
        &config,
        events,
    )
    .await
}
