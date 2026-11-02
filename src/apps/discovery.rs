//! Discovers declarations from compiled Cargo binaries without sharing runtime globals.
//!
//! A project's binary registers its apps, calls [`export_if_requested`], and returns when
//! that function returns true, before running local entrypoints. Each scan builds and runs
//! fresh processes. The returned definitions are metadata; live Rust handles remain inside
//! the project executable. Cargo's normal target directory is reused between scans.
use super::{AppRegistry, deploy::AppDefinition};
use crate::{Result, Value};
use serde::{Deserialize, Serialize};
use std::{
    collections::HashSet,
    ffi::{OsStr, OsString},
    path::{Path, PathBuf},
    process::{Output, Stdio},
};
use tokio::{io::AsyncReadExt, process::Command};

const SCAN: &str = "RUNPOD_DISCOVERY_SCAN";
const OUTPUT: &str = "RUNPOD_DISCOVERY_OUTPUT";

/// A native project could not be inspected, compiled or loaded for discovery.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum DiscoveryError {
    /// The requested path is not a Cargo project or binary entry source.
    #[error("{0}")]
    InvalidTarget(String),
    /// A compiler or project process failed, or its export was invalid.
    #[error("loading {} failed: {message}", target.display())]
    Load {
        /// Cargo binary source or project path being loaded.
        target: PathBuf,
        /// Compiler, process or protocol diagnostic.
        message: String,
    },
    /// An underlying filesystem, process or SDK error while loading one target.
    #[error("loading {} failed: {source}", target.display())]
    Source {
        /// Binary source associated with the failing operation.
        target: PathBuf,
        /// Original error, retained for programmatic inspection.
        #[source]
        source: Box<crate::Error>,
    },
    /// Every relevant target failed and none returned an app.
    #[error("no runpod.App found; some targets failed to load:\n  {}", .0.join("\n  "))]
    NoApps(Vec<String>),
    /// Ctrl-C stopped discovery; later binaries are not started.
    #[error("discovery interrupted")]
    Interrupted,
}

/// One discovered definition together with its compiled project entry point.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct DiscoveredApp {
    /// Validated app metadata, in registration order within its executable.
    pub definition: AppDefinition,
    /// Binary entry source, which may register declarations from several Rust modules.
    pub source: PathBuf,
    /// Host-runnable executable reported by Cargo's artifact stream.
    pub executable: PathBuf,
    /// Directory containing the owning package's Cargo manifest.
    pub project_root: PathBuf,
}

#[derive(Serialize, Deserialize)]
struct Export {
    version: u32,
    apps: Vec<AppDefinition>,
}

/// Exports registered declarations when this process was started by discovery.
///
/// Call after registration and return from the binary's main function when this returns
/// true. No handler or local entrypoint is invoked. A false result performs no filesystem
/// writes. Ordinary registration output cannot corrupt the separate JSON export file.
///
/// # Errors
/// Returns registration, serialization or filesystem errors, or a missing export path.
pub fn export_if_requested(registry: &AppRegistry) -> Result<bool> {
    if std::env::var_os(SCAN).is_none_or(|value| value.is_empty()) {
        return Ok(false);
    }
    let output = std::env::var_os(OUTPUT)
        .filter(|v| !v.is_empty())
        .ok_or_else(|| {
            DiscoveryError::InvalidTarget("discovery output path was not supplied".into())
        })?;
    let apps = registry
        .apps()
        .iter()
        .map(super::App::definition)
        .collect::<Result<Vec<_>>>()?;
    let export = serde_json::to_vec(&Export { version: 1, apps })?;
    std::fs::write(output, export)?;
    Ok(true)
}

#[derive(Deserialize)]
struct Metadata {
    packages: Vec<Package>,
    workspace_members: HashSet<String>,
}
#[derive(Deserialize)]
struct Package {
    id: String,
    manifest_path: PathBuf,
    targets: Vec<CargoTarget>,
}
#[derive(Deserialize)]
struct CargoTarget {
    name: String,
    kind: Vec<String>,
    src_path: PathBuf,
    #[serde(default, rename = "required-features")]
    features: Vec<String>,
}
struct Binary {
    package: String,
    manifest: PathBuf,
    target: CargoTarget,
}

/// Builds and discovers apps from Cargo binary targets under the requested path.
///
/// A directory scan tolerates failed targets if another target exports apps. A specific
/// Cargo.toml or binary `.rs` entry is strict. Sources are processed in sorted path order;
/// the first app with each name wins. Nonexistent paths and directories without a Cargo
/// project return no apps. Rust modules are compiled in their package context, and each
/// child owns its registration state, so failed or previous scans cannot leak handles.
///
/// The project uses [`export_if_requested`] before local entrypoint execution. Cargo must
/// produce executables runnable on the host. Build scripts and declaration code execute
/// with the caller's permissions, as import-time Python code does in the source SDK.
/// Ctrl-C terminates and waits for the active child; dropping the future kills that child
/// with Tokio's normal process-reaping behavior. Parent environment variables are unchanged.
///
/// # Errors
/// Returns [`DiscoveryError`] for unsupported targets, build/load/export failures, or
/// interruption. Filesystem and process I/O errors retain their native error sources.
pub async fn discover_apps(target: &Path) -> Result<Vec<DiscoveredApp>> {
    if !target.try_exists()? {
        return Ok(Vec::new());
    }
    let target = std::fs::canonicalize(target)?;
    let strict = target.is_file();
    if strict
        && target.file_name().is_none_or(|n| n != "Cargo.toml")
        && target.extension().is_none_or(|e| e != "rs")
    {
        return Err(DiscoveryError::InvalidTarget(format!(
            "{} is not a Cargo manifest or Rust binary source",
            target.display()
        ))
        .into());
    }
    let directory = if strict {
        target.parent().ok_or_else(|| {
            DiscoveryError::InvalidTarget(format!("{} has no parent directory", target.display()))
        })?
    } else {
        &target
    };
    let Some(manifest) = directory
        .ancestors()
        .map(|p| p.join("Cargo.toml"))
        .find(|p| p.is_file())
    else {
        return if strict {
            Err(DiscoveryError::InvalidTarget(format!(
                "no Cargo project contains {}",
                target.display()
            ))
            .into())
        } else {
            Ok(Vec::new())
        };
    };
    let cargo = cargo_program();
    let metadata = output(
        Command::new(&cargo)
            .args([
                "metadata",
                "--no-deps",
                "--format-version",
                "1",
                "--manifest-path",
            ])
            .arg(&manifest)
            .current_dir(directory),
    )
    .await?;
    if !metadata.status.success() {
        return Err(load_error(&target, diagnostic(&metadata)).into());
    }
    let metadata: Metadata = serde_json::from_slice(&metadata.stdout)
        .map_err(|e| load_error(&target, format!("invalid Cargo metadata: {e}")))?;
    let mut binaries = Vec::new();
    for package in metadata.packages {
        if !metadata.workspace_members.contains(&package.id) {
            continue;
        }
        for mut candidate in package.targets {
            if !candidate.kind.iter().any(|k| k == "bin") {
                continue;
            }
            candidate.src_path = std::fs::canonicalize(&candidate.src_path)?;
            let selected = if strict && target.file_name().is_some_and(|n| n != "Cargo.toml") {
                candidate.src_path == target
            } else {
                candidate.src_path.starts_with(directory)
            };
            if selected {
                binaries.push(Binary {
                    package: package.id.clone(),
                    manifest: package.manifest_path.clone(),
                    target: candidate,
                });
            }
        }
    }
    binaries.sort_by(|a, b| {
        a.target
            .src_path
            .cmp(&b.target.src_path)
            .then_with(|| a.target.name.cmp(&b.target.name))
    });
    if strict && binaries.is_empty() {
        return Err(DiscoveryError::InvalidTarget(format!(
            "no Cargo binary entry found for {}",
            target.display()
        ))
        .into());
    }
    let mut found = Vec::new();
    let mut names = HashSet::new();
    let mut failures = Vec::new();
    for binary in binaries {
        match load(&cargo, &binary).await {
            Ok(apps) => found.extend(
                apps.into_iter()
                    .filter(|app| names.insert(app.definition.name.clone())),
            ),
            Err(crate::Error::Discovery(error))
                if !strict && !matches!(error, DiscoveryError::Interrupted) =>
            {
                tracing::warn!(%error, "application discovery failed");
                failures.push(error.to_string());
            }
            Err(error) => return Err(error),
        }
    }
    if found.is_empty() && !failures.is_empty() {
        return Err(DiscoveryError::NoApps(failures).into());
    }
    Ok(found)
}

fn cargo_program() -> OsString {
    if let Some(cargo) = std::env::var_os("CARGO").filter(|s| !s.is_empty()) {
        return cargo;
    }
    let home = std::env::var_os("CARGO_HOME")
        .map(PathBuf::from)
        .or_else(|| {
            std::env::var_os("HOME")
                .or_else(|| std::env::var_os("USERPROFILE"))
                .map(|p| PathBuf::from(p).join(".cargo"))
        });
    if let Some(home) = home {
        let cargo = home
            .join("bin")
            .join(if cfg!(windows) { "cargo.exe" } else { "cargo" });
        if cargo.is_file() {
            return cargo.into_os_string();
        }
    }
    "cargo".into()
}
fn load_error(target: &Path, message: String) -> DiscoveryError {
    DiscoveryError::Load {
        target: target.into(),
        message,
    }
}
fn diagnostic(result: &Output) -> String {
    let mut message = String::from_utf8_lossy(&result.stderr).trim().to_owned();
    for line in result.stdout.split(|b| *b == b'\n') {
        if let Ok(value) = serde_json::from_slice::<Value>(line)
            && let Some(rendered) = value["message"]["rendered"].as_str()
        {
            message.push('\n');
            message.push_str(rendered.trim());
        }
    }
    if message.is_empty() {
        format!("process exited with {}", result.status)
    } else {
        message
    }
}
async fn load(cargo: &OsStr, binary: &Binary) -> Result<Vec<DiscoveredApp>> {
    load_target(cargo, binary)
        .await
        .map_err(|error| match error {
            crate::Error::Discovery(_) => error,
            error => DiscoveryError::Source {
                target: binary.target.src_path.clone(),
                source: Box::new(error),
            }
            .into(),
        })
}

async fn load_target(cargo: &OsStr, binary: &Binary) -> Result<Vec<DiscoveredApp>> {
    let root = binary.manifest.parent().ok_or_else(|| {
        load_error(
            &binary.manifest,
            "Cargo manifest has no parent directory".into(),
        )
    })?;
    let mut build = Command::new(cargo);
    build
        .args(["build", "--message-format=json", "--manifest-path"])
        .arg(&binary.manifest)
        .arg("--package")
        .arg(&binary.package)
        .arg("--bin")
        .arg(&binary.target.name)
        .current_dir(root);
    if !binary.target.features.is_empty() {
        build
            .arg("--features")
            .arg(binary.target.features.join(","));
    }
    let result = output(&mut build).await?;
    if !result.status.success() {
        return Err(load_error(&binary.target.src_path, diagnostic(&result)).into());
    }
    let executable = result
        .stdout
        .split(|b| *b == b'\n')
        .filter_map(|line| serde_json::from_slice::<Value>(line).ok())
        .find_map(|value| {
            if value["reason"] == "compiler-artifact"
                && value["package_id"] == binary.package
                && value["target"]["name"] == binary.target.name
            {
                value["executable"].as_str().map(PathBuf::from)
            } else {
                None
            }
        })
        .ok_or_else(|| {
            load_error(
                &binary.target.src_path,
                "Cargo did not report an executable".into(),
            )
        })?;
    let scratch = tempfile::tempdir()?;
    let export = scratch.path().join("apps.json");
    let result = output(
        Command::new(&executable)
            .current_dir(root)
            .env(SCAN, "1")
            .env(OUTPUT, &export),
    )
    .await?;
    if !result.status.success() {
        return Err(load_error(&binary.target.src_path, diagnostic(&result)).into());
    }
    let bytes = std::fs::read(&export).map_err(|e| load_error(&binary.target.src_path, format!("project did not export app declarations: {e}; call discovery::export_if_requested after registration")))?;
    let export: Export = serde_json::from_slice(&bytes).map_err(|e| {
        load_error(
            &binary.target.src_path,
            format!("invalid discovery export: {e}"),
        )
    })?;
    if export.version != 1 {
        return Err(load_error(
            &binary.target.src_path,
            format!("unsupported discovery export version {}", export.version),
        )
        .into());
    }
    // Validate the whole target before exposing any of its declarations.
    for app in &export.apps {
        app.validate().map_err(|e| {
            load_error(
                &binary.target.src_path,
                format!("invalid app declaration: {e}"),
            )
        })?;
    }
    Ok(export
        .apps
        .into_iter()
        .map(|definition| DiscoveredApp {
            definition,
            source: binary.target.src_path.clone(),
            executable: executable.clone(),
            project_root: root.into(),
        })
        .collect())
}

async fn output(command: &mut Command) -> Result<Output> {
    let mut child = command
        .kill_on_drop(true)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    let mut out = child.stdout.take().expect("stdout is piped");
    let mut err = child.stderr.take().expect("stderr is piped");
    let (mut stdout, mut stderr) = (Vec::new(), Vec::new());
    let result = tokio::select! {
        biased;
        signal = tokio::signal::ctrl_c() => match signal {
            Ok(()) => Err(DiscoveryError::Interrupted.into()),
            Err(error) => Err(error.into()),
        },
        result = async {
            let (_, _, status) = tokio::try_join!(out.read_to_end(&mut stdout), err.read_to_end(&mut stderr), child.wait())?;
            Ok::<_, std::io::Error>(status)
        } => result.map_err(crate::Error::from),
    };
    match result {
        Ok(status) if interrupted(status) => Err(DiscoveryError::Interrupted.into()),
        Ok(status) => Ok(Output {
            status,
            stdout,
            stderr,
        }),
        Err(error) => {
            let _ = child.kill().await;
            Err(error)
        }
    }
}

fn interrupted(status: std::process::ExitStatus) -> bool {
    if matches!(status.code(), Some(130 | -1_073_741_510)) {
        return true;
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        status.signal() == Some(libc::SIGINT)
    }
    #[cfg(not(unix))]
    false
}
