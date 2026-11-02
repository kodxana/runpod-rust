//! Archives a compiled worker and project assets without constructing a Python environment.
use super::{
    AppError,
    deploy::{AppDefinition, Artifact, PreparedArtifact, build_manifest},
    events::{EventSink, emit},
};
use crate::{Error, Result, json};
use std::path::Path;

/// Fixed executable path inside a native application artifact, independent of its source filename.
pub const NATIVE_WORKER_PATH: &str = "runpod-native/worker";

/// Packages an already-compiled worker, project assets and native application manifest as tar.gz.
///
/// The executable must register the supplied app and call [`super::bootstrap::start_deployed`].
/// It is copied byte-for-byte with mode 0755 to [`NATIVE_WORKER_PATH`]. Compile it for the
/// intended worker OS/architecture and container libraries; this function neither builds nor
/// executes it and does not validate its binary format. Host executables are useful for local
/// testing but do not thereby become deployable Linux binaries.
///
/// Source ignore/credential rules match [`super::deploy::package_project`]. Root `target/`
/// and `runpod-native/` are additionally reserved and cannot be re-included. The selected
/// executable is omitted at its original source path. Ignore any other build directories
/// through `.runpodignore`. Assets remain at their project-relative paths. The manifest
/// retains resource/symbol metadata but omits Python interpreter and package-exclusion fields;
/// crate dependencies belong in the compiled worker, not a vendored Python environment.
///
/// This blocking operation does no cloud work. A returned automatic artifact owns its storage;
/// explicit output remains caller-owned. Platform upload/image/launch wiring is separate.
///
/// # Errors
/// Returns invalid app/symbol/root/worker input, archive or filesystem failures, or an artifact
/// exceeding the platform size limit. Output may not alias the executable, including hard links.
pub fn build_native_artifact(
    app: &AppDefinition,
    project_root: &Path,
    executable: &Path,
    output: Option<&Path>,
    events: Option<&EventSink>,
) -> Result<Artifact> {
    Ok(prepare_native_artifact(app, project_root, executable, output, None, events)?.artifact)
}

pub(super) fn native_manifest(
    app: &AppDefinition,
    image_version: Option<&str>,
) -> Result<crate::Value> {
    app.validate()?;
    let mut manifest = build_manifest(app, image_version.unwrap_or_default(), &[])?;
    for resource in &app.resources {
        if resource.symbol.as_ref().is_none_or(|symbol| {
            symbol.module.trim().is_empty() || symbol.qualname.trim().is_empty()
        }) {
            return Err(Error::InvalidInput(format!(
                "native resource '{}' requires a module and qualified symbol name",
                resource.spec.name
            )));
        }
    }
    // Standalone archives omit the interpreter field; deployment keeps the image-family hint.
    if image_version.is_none()
        && let Some(object) = manifest.as_object_mut()
    {
        object.remove("pythonVersion");
    }
    Ok(manifest)
}

pub(super) fn prepare_native_artifact(
    app: &AppDefinition,
    project_root: &Path,
    executable: &Path,
    output: Option<&Path>,
    image_version: Option<&str>,
    events: Option<&EventSink>,
) -> Result<PreparedArtifact> {
    let manifest = native_manifest(app, image_version)?;
    if !std::fs::metadata(project_root)?.is_dir() {
        return Err(Error::InvalidInput(format!(
            "native project root must be a directory: {}",
            project_root.display()
        )));
    }
    emit(events, "phase", &[json!("package"), json!("")]);
    let artifact =
        super::package::package_native_project(project_root, &manifest, executable, output)?;
    let size = std::fs::metadata(artifact.path())?.len();
    if size > super::build::MAX_ARTIFACT_MB * 1024 * 1024 {
        return Err(AppError::Build(format!(
            "native artifact exceeds {} MB; strip the worker binary or exclude project assets with .runpodignore",
            super::build::MAX_ARTIFACT_MB
        )).into());
    }
    tracing::info!(app=%app.name, size_bytes=size, "packaged native application artifact");
    Ok(PreparedArtifact {
        artifact,
        manifest,
        size,
    })
}
