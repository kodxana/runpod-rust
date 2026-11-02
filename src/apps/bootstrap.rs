//! Artifact preparation, deployed startup and failure reporting for application workers.
//!
//! [`BootstrapConfig`] and [`read_manifest`] provide blocking filesystem preparation.
//! [`start_deployed`] connects preparation, system packages and native resource serving.
//! [`report_error`] starts the appropriate queue/API/task responder after a bootstrap failure.
use crate::{Error, Value, json};
mod reporting;
mod startup;
mod system;
pub use reporting::{QueueErrorReporter, report_error, serve_api_error};
pub use startup::start_deployed;
use std::{
    collections::BTreeMap,
    fs::{self, File},
    io::{self, BufRead, BufReader, Read},
    path::{Component, Path, PathBuf},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
pub use system::SystemPackageInstaller;

/// Artifact manifest consumed by the coordinator and deployed workers.
pub const MANIFEST_NAME: &str = "runpod_manifest.json";
/// Warm-container marker used to skip extraction on later starts.
pub const UNPACK_MARKER: &str = ".rp-unpacked";

/// A bootstrap phase failed, retaining its public diagnostic and optional native cause.
#[derive(Debug, thiserror::Error)]
#[error("{phase}: {detail}")]
pub struct PhaseError {
    /// Phase identifier, including application-defined phases.
    pub phase: String,
    /// Human-readable failure detail.
    pub detail: String,
    #[source]
    source: Option<Box<Error>>,
}
impl PhaseError {
    /// Creates the same phase/detail diagnostic as the worker runtime's `PhaseError`.
    pub fn new(phase: impl Into<String>, detail: impl Into<String>) -> Self {
        Self {
            phase: phase.into(),
            detail: detail.into(),
            source: None,
        }
    }

    fn caused_by(phase: &str, context: &str, cause: impl Into<Error>) -> Self {
        let source = cause.into();
        Self {
            phase: phase.into(),
            detail: format!("{context}: {source}"),
            source: Some(Box::new(source)),
        }
    }

    /// Produces the structured `BootstrapError` value used by runtime error responders.
    #[must_use]
    pub fn payload(&self) -> Value {
        json!({"error_type":"BootstrapError", "error_message":format!(
            "worker bootstrap failed during '{}': {}", self.phase, self.detail
        )})
    }
}

/// Owned artifact paths and wait limit, replacing bootstrap module-level environment state.
#[derive(Debug, Clone)]
pub struct BootstrapConfig {
    /// Host-mounted artifact tarball; compression is detected from its contents.
    pub artifact_path: PathBuf,
    /// Directory into which the artifact is extracted.
    pub app_dir: PathBuf,
    /// Existing host-unpacked directory, preferred over the marker and tarball.
    pub prebuilt_app_dir: Option<PathBuf>,
    /// Maximum time waiting for the artifact file; existence is checked every two seconds.
    pub artifact_wait: Duration,
}
impl Default for BootstrapConfig {
    fn default() -> Self {
        Self {
            artifact_path: "/root/.runpod/artifact.tar.gz".into(),
            app_dir: "/app".into(),
            prebuilt_app_dir: None,
            artifact_wait: Duration::from_secs(300),
        }
    }
}
impl BootstrapConfig {
    /// Captures platform artifact paths and `RUNPOD_ARTIFACT_WAIT` without mutating environment.
    ///
    /// # Errors
    /// Returns a locate-phase error for an invalid integer wait limit.
    pub fn from_env() -> Result<Self, PhaseError> {
        Self::from_values(&std::env::vars().collect())
    }

    /// Uses explicit environment values. Negative waits expire immediately; empty paths
    /// retain the source's meaning, except an empty prebuilt path disables that override.
    ///
    /// # Errors
    /// Returns a locate-phase error if the wait does not parse as a signed 64-bit integer.
    pub fn from_values(values: &BTreeMap<String, String>) -> Result<Self, PhaseError> {
        let mut config = Self::default();
        if let Some(value) = values.get("FLASH_BUILD_ARTIFACT_PATH") {
            config.artifact_path = value.into();
        }
        if let Some(value) = values.get("RUNPOD_APP_DIR") {
            config.app_dir = value.into();
        }
        config.prebuilt_app_dir = values
            .get("RUNPOD_PREBUILT_APP_DIR")
            .filter(|s| !s.is_empty())
            .map(PathBuf::from);
        if let Some(value) = values.get("RUNPOD_ARTIFACT_WAIT") {
            let seconds = value.trim().parse::<i64>().map_err(|_| {
                PhaseError::new("locate", "RUNPOD_ARTIFACT_WAIT must be an integer")
            })?;
            config.artifact_wait = Duration::from_secs(u64::try_from(seconds).unwrap_or(0));
        }
        Ok(config)
    }

    /// Locates a pre-unpacked app tree or waits for and extracts the platform artifact.
    ///
    /// A prebuilt directory takes precedence over an existing unpack marker. The marker
    /// skips extraction even if the artifact is missing or has changed. Extraction writes
    /// in archive order and retains earlier files when a later entry fails, as upstream does.
    /// This method blocks during the two-second polling waits and filesystem work; async
    /// callers should use their blocking executor. Dropping that executor's future does not
    /// cancel an already-running operation. Call once per worker startup, not concurrently.
    ///
    /// Native artifacts accept files/directories with relative paths, including GNU/PAX
    /// long names. Links, special files and paths containing traversal or Windows-special
    /// separators are rejected. Tar, gzip, bzip2 and xz compression are supported.
    ///
    /// # Errors
    /// Returns a locate-phase timeout, unsafe member, filesystem or decompression error.
    pub fn locate(&self) -> Result<PathBuf, PhaseError> {
        if let Some(path) = self.prebuilt_app_dir.as_ref().filter(|path| path.is_dir()) {
            tracing::info!(path=%path.display(), "using pre-unpacked app tree");
            return Ok(path.clone());
        }
        let marker = self.app_dir.join(UNPACK_MARKER);
        if marker.is_file() {
            return Ok(self.app_dir.clone());
        }
        let started = Instant::now();
        while !self.artifact_path.is_file() {
            if started.elapsed() >= self.artifact_wait {
                return Err(PhaseError::new(
                    "locate",
                    format!(
                        "artifact not found at {} after {}s; is this endpoint part of an app deployment?",
                        self.artifact_path.display(),
                        self.artifact_wait.as_secs_f64()
                    ),
                ));
            }
            std::thread::sleep(Duration::from_secs(2));
        }
        fs::create_dir_all(&self.app_dir)
            .map_err(|e| PhaseError::caused_by("locate", "failed to create app directory", e))?;
        extract(&self.artifact_path, &self.app_dir)
            .map_err(|e| PhaseError::caused_by("locate", "failed to extract artifact", e))?;
        let timestamp = match SystemTime::now().duration_since(UNIX_EPOCH) {
            Ok(duration) => duration.as_secs_f64(),
            Err(error) => -error.duration().as_secs_f64(),
        };
        fs::write(marker, timestamp.to_string())
            .map_err(|e| PhaseError::caused_by("locate", "failed to write unpack marker", e))?;
        Ok(self.app_dir.clone())
    }
}

/// Returns the vendored environment first when present, followed by the app directory.
/// Does not modify process search paths or import anything.
#[must_use]
pub fn dependency_paths(app_dir: &Path) -> Vec<PathBuf> {
    let env = app_dir.join("env");
    if env.is_dir() {
        vec![env, app_dir.to_owned()]
    } else {
        vec![app_dir.to_owned()]
    }
}

/// Reads the artifact manifest without imposing resource validation at this phase.
/// All valid JSON values are accepted, matching the source; dispatch validates structure.
///
/// # Errors
/// Returns an attach-phase error for a missing/unreadable file or malformed JSON.
pub fn read_manifest(app_dir: &Path) -> Result<Value, PhaseError> {
    let path = app_dir.join(MANIFEST_NAME);
    if !path.is_file() {
        return Err(PhaseError::new(
            "attach",
            format!("manifest not found at {}", path.display()),
        ));
    }
    let file = File::open(&path)
        .map_err(|e| PhaseError::caused_by("attach", "failed to read manifest", e))?;
    serde_json::from_reader(BufReader::new(file))
        .map_err(|e| PhaseError::caused_by("attach", "failed to read manifest", e))
}

fn extract(artifact: &Path, app_dir: &Path) -> io::Result<()> {
    let root = fs::canonicalize(app_dir)?;
    let mut file = BufReader::new(File::open(artifact)?);
    let magic = file.fill_buf()?;
    let mut reader: Box<dyn Read> = if magic.starts_with(&[0x1f, 0x8b]) {
        Box::new(flate2::read::MultiGzDecoder::new(file))
    } else if magic.starts_with(b"BZh") {
        Box::new(bzip2::read::MultiBzDecoder::new(file))
    } else if magic.starts_with(b"\xfd7zXZ\0") {
        Box::new(lzma_rust2::XzReader::new(file, true))
    } else {
        Box::new(file)
    };
    // tar::Archive treats a zero-byte reader as an empty archive; Python rejects it.
    let mut first = [0; 512];
    reader.read_exact(&mut first)?;
    let mut archive = tar::Archive::new(io::Cursor::new(first).chain(reader));
    for entry in archive.entries()? {
        let mut entry = entry?;
        let path = entry.path()?.into_owned();
        let text = path.to_string_lossy();
        if text.contains(['\\', ':'])
            || path.components().any(|c| {
                matches!(
                    c,
                    Component::Prefix(_) | Component::RootDir | Component::ParentDir
                )
            })
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("unsafe tar member path: {}", path.display()),
            ));
        }
        let kind = entry.header().entry_type();
        if !kind.is_file() && !kind.is_dir() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("unsupported tar member type: {}", path.display()),
            ));
        }
        if kind.is_file() && !path.components().any(|c| matches!(c, Component::Normal(_))) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "regular tar file cannot replace app directory",
            ));
        }
        let destination = root.join(&path);
        validate_destination(&root, &destination)?;
        if kind.is_dir() {
            // Python's data filter ignores directory ownership/mode so children remain writable.
            fs::create_dir_all(destination)?;
        } else {
            entry.set_mask(0o7022);
            if !entry.unpack_in(&root)? {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("unsafe tar member path: {}", path.display()),
                ));
            }
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let mut mode = entry.header().mode()? & 0o755;
                if mode & 0o100 == 0 {
                    mode &= !0o111;
                }
                fs::set_permissions(destination, fs::Permissions::from_mode(mode | 0o600))?;
            }
            #[cfg(windows)]
            {
                // Python's data filter always adds owner read/write to regular files.
                let mut permissions = fs::metadata(&destination)?.permissions();
                #[expect(
                    clippy::permissions_set_readonly_false,
                    reason = "Windows-only readonly attribute; Unix uses explicit owner modes above"
                )]
                permissions.set_readonly(false);
                fs::set_permissions(destination, permissions)?;
            }
        }
    }
    Ok(())
}

fn validate_destination(root: &Path, destination: &Path) -> io::Result<()> {
    let mut existing = destination;
    loop {
        match fs::canonicalize(existing) {
            Ok(path) if path.starts_with(root) => return Ok(()),
            Ok(_) => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!(
                        "tar destination is outside app directory: {}",
                        destination.display()
                    ),
                ));
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                existing = existing.parent().ok_or(error)?;
            }
            Err(error) => return Err(error),
        }
    }
}
