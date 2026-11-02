use super::{Binary, DiscoveredApp, DiscoveryError, cargo_program, diagnostic, load_error, output};
use crate::{Result, Value};
use std::{ffi::OsStr, path::PathBuf};
use tokio::process::Command;

/// Cargo target and profile for a discovered native worker's deployment build.
///
/// Cargo manages compiler/linker setup and its existing target cache. These options do
/// not install toolchains, invoke containers, or change project configuration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkerBuildOptions {
    /// Rust target triple or custom target specification. Defaults to Linux `x86_64` GNU.
    /// Upload through `deploy_native_app` separately validates the Linux `x86_64` ELF header.
    pub target: String,
    /// Cargo build profile, including project-defined profiles. Defaults to `release`.
    pub profile: String,
}
impl Default for WorkerBuildOptions {
    fn default() -> Self {
        Self {
            target: "x86_64-unknown-linux-gnu".into(),
            profile: "release".into(),
        }
    }
}

impl DiscoveredApp {
    /// Builds this app's selected Cargo binary for a worker target without executing it.
    ///
    /// Retains the discovered package identity, manifest, binary name and required features.
    /// Runs Cargo with `--locked`, an explicit target and profile, and returns its reported
    /// executable path rather than guessing a target directory. Cargo owns that output/cache.
    /// The selected toolchain, target standard library, linker and native dependencies must
    /// already support the requested target. No compiler tools or target components are installed.
    ///
    /// Keep project inputs stable between discovery and deployment. Target-conditional app
    /// declarations must match host discovery; a cross-target worker is never executed here.
    /// Build scripts and compilers run with the caller's permissions. Ctrl-C kills and waits
    /// for the direct Cargo child; dropping the future kills that child with normal Tokio
    /// reaping. Compiler descendants follow Cargo's lifecycle. Parent environment is unchanged.
    ///
    /// # Errors
    /// Returns `DiscoveryError` for empty options, compiler failure, interruption, or a missing
    /// executable record. Process/filesystem errors retain their source and app source path.
    pub async fn build_worker(&self, options: &WorkerBuildOptions) -> Result<PathBuf> {
        for (name, value) in [("target", &options.target), ("profile", &options.profile)] {
            if value.is_empty() {
                return Err(DiscoveryError::InvalidTarget(format!(
                    "worker build {name} must not be empty"
                ))
                .into());
            }
        }
        compile(&cargo_program(), &self.binary, Some(options))
            .await
            .map_err(|error| match error {
                crate::Error::Discovery(_) => error,
                error => DiscoveryError::Source {
                    target: self.source.clone(),
                    source: Box::new(error),
                }
                .into(),
            })
    }
}

pub(super) async fn compile(
    cargo: &OsStr,
    binary: &Binary,
    options: Option<&WorkerBuildOptions>,
) -> Result<PathBuf> {
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
    if let Some(options) = options {
        build
            .arg("--locked")
            .arg(format!("--target={}", options.target))
            .arg(format!("--profile={}", options.profile));
    }
    let result = output(&mut build).await?;
    if !result.status.success() {
        return Err(load_error(&binary.target.src_path, diagnostic(&result)).into());
    }
    result
        .stdout
        .split(|b| *b == b'\n')
        .filter_map(|line| serde_json::from_slice::<Value>(line).ok())
        .find_map(|value| {
            if value["reason"] == "compiler-artifact"
                && value["package_id"] == binary.package
                && value["target"]["name"] == binary.target.name
                && value["target"]["kind"]
                    .as_array()
                    .is_some_and(|kinds| kinds.iter().any(|kind| kind == "bin"))
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
            .into()
        })
}
