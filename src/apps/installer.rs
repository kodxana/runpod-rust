//! Explicit external build tools for the Python-compatible artifact format.
use super::AppError;
use crate::{Error, Result};
use regex::Regex;
use std::{
    collections::VecDeque,
    ffi::OsString,
    fs,
    io::{self, BufRead, BufReader},
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::LazyLock,
};

/// Worker-compatible platform tags, preferred newest first.
pub const MANYLINUX_PLATFORMS: &[&str] = &[
    "manylinux_2_28_x86_64",
    "manylinux_2_17_x86_64",
    "manylinux2014_x86_64",
];

/// Fallible observer for the installer's one-based package count and distribution name.
pub type BuildProgress<'a> = dyn FnMut(usize, &str) -> Result<()> + 'a;

static PINNABLE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^[A-Za-z0-9._-]+(\[[^\]]+\])?([<>=!~;].*)?$")
        .expect("static pinnable requirement pattern")
});

/// External interpreter command used only for wheel building and dependency installation.
///
/// Operations are synchronous and may run for the lifetime of the installer process.
/// Invoke them on a blocking executor in async applications. All SDK orchestration and
/// metadata analysis stays in Rust; this command runs pip/ensurepip, not the Python SDK.
#[derive(Debug, Clone)]
pub struct BuildTools {
    /// Interpreter executable, such as a virtual environment's python binary.
    pub python: PathBuf,
    /// Arguments preceding `-m pip` or `-m ensurepip`, useful for launcher commands.
    pub arguments: Vec<OsString>,
}
impl Default for BuildTools {
    fn default() -> Self {
        Self {
            python: if cfg!(windows) { "python" } else { "python3" }.into(),
            arguments: Vec::new(),
        }
    }
}

/// Flags controlling a dependency installation.
#[derive(Debug, Clone, Default)]
pub struct VendorOptions {
    /// Install only the supplied wheels/specifications without their dependencies.
    pub no_deps: bool,
}

fn tail(text: &str, limit: usize) -> String {
    text.chars()
        .rev()
        .take(limit)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect()
}

impl BuildTools {
    fn command(&self) -> Command {
        let mut command = Command::new(&self.python);
        command.args(&self.arguments).stdin(Stdio::null());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            // CREATE_NO_WINDOW: build tools must not open console windows.
            command.creation_flags(0x0800_0000);
        }
        command
    }

    fn ensure_pip(&self) -> Result<()> {
        let probe = self.command().args(["-m", "pip", "--version"]).output()?;
        if probe.status.success() {
            return Ok(());
        }
        let result = self
            .command()
            .args(["-m", "ensurepip", "--upgrade"])
            .output()?;
        if !result.status.success() {
            return Err(AppError::Build(format!(
                "pip is not available in this environment and ensurepip failed: {}",
                tail(&String::from_utf8_lossy(&result.stderr), 500)
            ))
            .into());
        }
        Ok(())
    }

    /// Returns an index requirement unchanged or builds a wheel for a URL/path override.
    ///
    /// `None` selects `runpod`; pass a captured `RUNPOD_PACKAGE_SPEC` value to override.
    /// Wheel builds inherit the environment and default the setuptools-scm version only
    /// when that variable is absent. Existing wheel names participate in sorted selection.
    ///
    /// # Errors
    /// Returns process, bootstrap, wheel-build, missing-wheel, or filesystem failures.
    pub fn runpod_requirement(&self, scratch: &Path, package_spec: Option<&str>) -> Result<String> {
        let spec = package_spec.unwrap_or("runpod");
        if !spec.contains(['/', '\\', ':', '@'])
            || (spec
                .as_bytes()
                .first()
                .is_some_and(u8::is_ascii_alphanumeric)
                && PINNABLE.is_match(spec))
        {
            return Ok(spec.to_owned());
        }
        Ok(self
            .build_wheel(spec, scratch)?
            .to_string_lossy()
            .into_owned())
    }

    fn build_wheel(&self, spec: &str, scratch: &Path) -> Result<PathBuf> {
        self.ensure_pip()?;
        let wheel_dir = scratch.join("wheels");
        fs::create_dir_all(&wheel_dir)?;
        let mut command = self.command();
        command
            .args(["-m", "pip", "wheel", "--no-deps", "-q", "-w"])
            .arg(&wheel_dir)
            .arg(spec);
        if std::env::var_os("SETUPTOOLS_SCM_PRETEND_VERSION_FOR_RUNPOD").is_none() {
            command.env("SETUPTOOLS_SCM_PRETEND_VERSION_FOR_RUNPOD", "0.0.0.dev0");
        }
        let result = command.output()?;
        if !result.status.success() {
            return Err(AppError::Build(format!(
                "failed to build wheel from RUNPOD_PACKAGE_SPEC={spec:?}: {}",
                tail(&String::from_utf8_lossy(&result.stderr), 2000)
            ))
            .into());
        }
        let mut all_wheels = Vec::new();
        for entry in fs::read_dir(&wheel_dir)? {
            let entry = entry?;
            if entry.path().extension().is_some_and(|ext| ext == "whl") {
                all_wheels.push(entry.path());
            }
        }
        all_wheels.sort();
        if let Some(path) = all_wheels.iter().rev().find(|path| {
            path.file_name()
                .is_some_and(|name| name.to_string_lossy().starts_with("runpod-"))
        }) {
            return Ok(path.clone());
        }
        Err(AppError::Build(format!(
            "pip wheel produced no runpod wheel from {spec:?} (found: {:?})",
            all_wheels
                .iter()
                .filter_map(|path| path.file_name())
                .collect::<Vec<_>>()
        ))
        .into())
    }

    /// Installs wheel-only dependencies for the selected interpreter and worker platforms.
    ///
    /// Progress receives the one-based package count and parsed name for each `Collecting`
    /// line. Its returned error stops and reaps the installer before returning. Empty
    /// requirements do not probe or launch tools. stdout/stderr share a pipe; failures
    /// retain only the final thirty lines and three thousand Unicode characters.
    ///
    /// # Errors
    /// Returns bootstrap, spawn, output-read, observer, or nonzero installer-exit errors.
    pub fn vendor(
        &self,
        env_dir: &Path,
        requirements: &[String],
        python_version: &str,
        options: &VendorOptions,
        mut progress: Option<&mut BuildProgress<'_>>,
    ) -> Result<()> {
        if requirements.is_empty() {
            return Ok(());
        }
        self.ensure_pip()?;
        let mut command = self.command();
        command
            .args([
                "-m",
                "pip",
                "install",
                "--upgrade",
                "--no-color",
                "--target",
            ])
            .arg(env_dir)
            .args([
                "--python-version",
                python_version,
                "--implementation",
                "cp",
                "--only-binary",
                ":all:",
            ]);
        for platform in MANYLINUX_PLATFORMS {
            command.args(["--platform", platform]);
        }
        if options.no_deps {
            command.arg("--no-deps");
        }
        command.args(requirements);
        let (reader, writer) = io::pipe()?;
        command.stdout(writer.try_clone()?).stderr(writer);
        let mut child = ChildCleanup(command.spawn()?);
        // Command retains its configured handles after spawn; close them for EOF.
        drop(command);
        let mut output = VecDeque::new();
        let mut reader = BufReader::new(reader);
        let mut count = 0;
        loop {
            let mut line = String::new();
            if reader.read_line(&mut line)? == 0 {
                break;
            }
            let trimmed = line.trim();
            if let Some(package) = trimmed.strip_prefix("Collecting ") {
                count += 1;
                let name = package
                    .split(['>', '<', '=', '!', '~', '[', ' ', '('])
                    .next()
                    .unwrap_or("");
                if let Some(callback) = progress.as_deref_mut() {
                    callback(count, name)?;
                }
            }
            if output.len() == 30 {
                output.pop_front();
            }
            output.push_back(line);
        }
        let status = child.0.wait()?;
        if !status.success() {
            return Err(AppError::Build(format!("dependency vendoring failed: {}\npackages without linux wheels cannot be vendored; exclude them (they must then come from the image)",tail(&output.into_iter().collect::<String>(),3000))).into());
        }
        Ok(())
    }
}

// This is a synchronous subprocess API. Reap after kill on read/callback failure or
// unwinding so an abandoned installer cannot keep mutating the build environment.
struct ChildCleanup(Child);
impl Drop for ChildCleanup {
    fn drop(&mut self) {
        if !matches!(self.0.try_wait(), Ok(Some(_))) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
}

/// Overlays an explicitly supplied SDK package tree, retaining unrelated destination files.
///
/// The source is the `runpod` package directory, not its repository root. Bytecode files
/// and cache directories are excluded. Rust has no importable Python package of its own,
/// so callers supply the corresponding Python-compatible runtime source explicitly.
///
/// # Errors
/// Returns filesystem failures or rejects overlapping source/destination trees.
pub fn sync_running_package(env_dir: &Path, source: &Path) -> Result<()> {
    let source = fs::canonicalize(source)?;
    fs::create_dir_all(env_dir)?;
    let env = fs::canonicalize(env_dir)?;
    let destination = env.join("runpod");
    let resolved = if destination.exists() {
        fs::canonicalize(&destination)?
    } else {
        destination.clone()
    };
    if resolved.starts_with(&source) || source.starts_with(&resolved) {
        return Err(Error::InvalidInput(
            "SDK source and destination must not overlap".into(),
        ));
    }
    let mut pending = vec![(source, destination)];
    while let Some((source, dest)) = pending.pop() {
        fs::create_dir_all(&dest)?;
        for entry in fs::read_dir(source)? {
            let entry = entry?;
            let name = entry.file_name();
            if name == "__pycache__" || name.to_string_lossy().ends_with(".pyc") {
                continue;
            }
            let path = entry.path();
            let target = dest.join(name);
            if path.is_dir() {
                pending.push((path, target));
            } else {
                fs::copy(path, target)?;
            }
        }
    }
    Ok(())
}
