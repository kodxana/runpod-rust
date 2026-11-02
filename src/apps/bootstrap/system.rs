use super::PhaseError;
use crate::{Value, client::truthy};
use std::{ffi::OsString, io, path::PathBuf, process::Stdio};

/// Installs the selected resource's operating-system packages using an explicit apt command.
///
/// Runs only when the first matching manifest entry requests nonempty system dependencies.
/// Child processes inherit the environment with `DEBIAN_FRONTEND=noninteractive`; arguments
/// are passed directly without shell evaluation. Dropping installation kills its direct child;
/// descendant processes follow the package manager's own lifecycle.
#[derive(Debug, Clone)]
pub struct SystemPackageInstaller {
    /// Package-manager executable; defaults to `apt-get` resolved through PATH.
    pub program: PathBuf,
    /// Arguments placed before the apt subcommand, for an explicit launcher or test double.
    pub arguments: Vec<OsString>,
}
impl Default for SystemPackageInstaller {
    fn default() -> Self {
        Self {
            program: "apt-get".into(),
            arguments: Vec::new(),
        }
    }
}
impl SystemPackageInstaller {
    /// Updates apt metadata, then installs only this resource's declared system packages.
    ///
    /// Executes `update -qq` followed by `install -y -qq --no-install-recommends` and the
    /// package list in declaration order. A failed update prevents installation. Duplicate
    /// resource names select the first entry, including when it requests no packages.
    ///
    /// # Errors
    /// Returns a system-phase error for malformed declarations, unavailable commands or
    /// nonzero exits. Process I/O errors retain their native cause; exit diagnostics retain
    /// the final 2,000 Unicode scalar values of stderr, matching the runtime's tail limit.
    pub async fn install(&self, manifest: &Value, resource_name: &str) -> Result<(), PhaseError> {
        let packages = packages(manifest, resource_name)?;
        if packages.is_empty() {
            return Ok(());
        }
        self.run(&["update", "-qq"], &packages, "apt-get update failed")
            .await?;
        let mut args = vec!["install", "-y", "-qq", "--no-install-recommends"];
        args.extend(packages.iter().copied());
        self.run(
            &args,
            &packages,
            &format!("failed to install system dependencies {packages:?}"),
        )
        .await
    }

    async fn run(&self, args: &[&str], packages: &[&str], failure: &str) -> Result<(), PhaseError> {
        let mut command = tokio::process::Command::new(&self.program);
        command
            .args(&self.arguments)
            .args(args)
            .env("DEBIAN_FRONTEND", "noninteractive")
            .stdin(Stdio::null())
            .kill_on_drop(true);
        #[cfg(windows)]
        command.creation_flags(0x0800_0000);
        let output=command.output().await.map_err(|error| {
            if error.kind()==io::ErrorKind::NotFound {
                PhaseError::caused_by("system", &format!("system dependencies {packages:?} requested but apt-get is not available in this image; use a debian-based image or bake them in"), error)
            } else { PhaseError::caused_by("system",failure,error) }
        })?;
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            let offset = stderr
                .char_indices()
                .rev()
                .nth(1999)
                .map_or(0, |(index, _)| index);
            return Err(PhaseError::new(
                "system",
                format!("{failure}: {}", &stderr[offset..]),
            ));
        }
        Ok(())
    }
}

fn packages<'a>(manifest: &'a Value, resource_name: &str) -> Result<Vec<&'a str>, PhaseError> {
    let invalid = |detail| PhaseError::new("system", detail);
    let manifest = manifest
        .as_object()
        .ok_or_else(|| invalid("manifest must be an object"))?;
    let Some(resources) = manifest.get("resources") else {
        return Ok(Vec::new());
    };
    let resources = resources
        .as_array()
        .ok_or_else(|| invalid("manifest resources must be an array"))?;
    for entry in resources {
        let entry = entry
            .as_object()
            .ok_or_else(|| invalid("manifest resources must contain objects"))?;
        if entry.get("name").and_then(Value::as_str) != Some(resource_name) {
            continue;
        }
        let Some(packages) = entry
            .get("systemDependencies")
            .filter(|value| truthy(value))
        else {
            return Ok(Vec::new());
        };
        return packages
            .as_array()
            .ok_or_else(|| invalid("systemDependencies must be an array of strings"))?
            .iter()
            .map(|value| {
                value
                    .as_str()
                    .ok_or_else(|| invalid("systemDependencies must be an array of strings"))
            })
            .collect();
    }
    Ok(Vec::new())
}
