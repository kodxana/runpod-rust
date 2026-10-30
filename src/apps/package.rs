//! Project archive construction with per-directory ignore scope and separate dependency files.
use crate::{Result, Value};
use flate2::{Compression, write::GzEncoder};
use ignore::{
    Match,
    gitignore::{Gitignore, GitignoreBuilder},
};
use std::{
    fs, io,
    path::{Path, PathBuf},
};

/// Defaults that project .gitignore and .runpodignore files may override.
pub const DEFAULT_IGNORES: &[&str] = &[
    ".venv/",
    "venv/",
    "env/",
    "__pycache__/",
    "*.pyc",
    "node_modules/",
    ".DS_Store",
    "*.tar.gz",
    "tests/",
    "test/",
    "test_*.py",
    "*_test.py",
    ".pytest_cache/",
    ".mypy_cache/",
    ".ruff_cache/",
    ".tox/",
    ".nox/",
];
/// Source paths excluded even when a user ignore file explicitly re-includes them.
pub const PROTECTED_IGNORES: &[&str] = &[
    ".git",
    ".runpod",
    ".flash",
    "/env",
    "/runpod_manifest.json",
    ".env",
    ".env.*",
    "*.env",
    "*.env.*",
    "*.pem",
    "*.key",
    "id_rsa",
    "id_rsa.*",
    "id_dsa",
    "id_dsa.*",
    "id_ecdsa",
    "id_ecdsa.*",
    "id_ed25519",
    "id_ed25519.*",
    ".ssh",
    ".aws",
    ".azure",
    ".kube",
    ".docker/config.json",
    "**/.docker/config.json",
    ".netrc",
    ".npmrc",
    ".pypirc",
    ".git-credentials",
    ".boto",
    "service-account*.json",
    "service_account*.json",
];
/// Vendored dependencies occupy this separate archive directory.
pub const ENV_DIR_NAME: &str = "env";

/// Optional output destination and an already-built dependency directory.
#[derive(Debug, Clone, Default)]
pub struct PackageOptions {
    /// Explicit destination, owned by the caller. None creates an owned temporary artifact.
    pub output: Option<PathBuf>,
    /// Dependency tree copied under env/, independently of source ignore patterns.
    pub env_dir: Option<PathBuf>,
}

/// A completed archive and, for automatic output paths, its temporary directory owner.
///
/// Dropping an automatic artifact removes its temporary directory. Explicit output
/// destinations are retained. Call `persist` to retain an automatic artifact after use.
#[derive(Debug)]
pub struct Artifact {
    path: PathBuf,
    scratch: Option<tempfile::TempDir>,
}
impl Artifact {
    /// Borrows the output path while retaining its temporary storage owner.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }
    /// Keeps the output on disk and transfers responsibility for deleting it to the caller.
    #[must_use]
    pub fn persist(mut self) -> PathBuf {
        if let Some(scratch) = self.scratch.take() {
            let _ = scratch.keep();
        }
        self.path
    }
}
impl AsRef<Path> for Artifact {
    fn as_ref(&self) -> &Path {
        self.path()
    }
}

struct IgnoreScope {
    directory: PathBuf,
    matcher: Gitignore,
}

fn patterns(root: &Path, lines: &[&str]) -> Result<Gitignore> {
    let mut builder = GitignoreBuilder::new(root);
    for line in lines {
        builder.add_line(None, line)?;
    }
    Ok(builder.build()?)
}

fn load_ignores(directory: &Path, name: &str) -> Result<Gitignore> {
    let path = directory.join(name);
    let mut builder = GitignoreBuilder::new(directory);
    match fs::symlink_metadata(&path) {
        Ok(metadata) if metadata.is_file() && !metadata.file_type().is_symlink() => {
            for line in fs::read_to_string(&path)?.lines() {
                builder.add_line(Some(path.clone()), line)?;
            }
        }
        Ok(_) => {}
        Err(error)
            if matches!(
                error.kind(),
                io::ErrorKind::NotFound | io::ErrorKind::NotADirectory
            ) => {}
        Err(error) => return Err(error.into()),
    }
    Ok(builder.build()?)
}

fn is_ignored(path: &Path, is_dir: bool, scopes: &[IgnoreScope], runpod: &Gitignore) -> bool {
    let mut ignored = false;
    for matcher in scopes
        .iter()
        .map(|scope| &scope.matcher)
        .chain(std::iter::once(runpod))
    {
        match matcher.matched_path_or_any_parents(path, is_dir) {
            Match::None => {}
            Match::Ignore(_) => ignored = true,
            Match::Whitelist(_) => ignored = false,
        }
    }
    ignored
}

fn directory_entries(path: &Path) -> Vec<fs::DirEntry> {
    // Like os.walk, an unreadable directory contributes no members. File reads below
    // still propagate errors, so a file selected for shipping cannot silently truncate.
    match fs::read_dir(path) {
        Ok(entries) => {
            let mut entries = entries
                .filter_map(std::result::Result::ok)
                .collect::<Vec<_>>();
            entries.sort_by_key(fs::DirEntry::file_name);
            entries
        }
        Err(_) => Vec::new(),
    }
}

fn warn_excluded(relative: &Path, is_dir: bool) {
    let mut path = relative.to_string_lossy().replace('\\', "/");
    if is_dir {
        path.push('/');
    }
    tracing::warn!(path=%path,"excluded source path from deployment artifact");
}

fn append_source(
    archive: &mut tar::Builder<GzEncoder<fs::File>>,
    root: &Path,
    output: &Path,
    env: Option<&Path>,
) -> Result<()> {
    let runpod = load_ignores(root, ".runpodignore")?;
    let protected = patterns(root, PROTECTED_IGNORES)?;
    let mut scopes = vec![IgnoreScope {
        directory: root.into(),
        matcher: patterns(root, DEFAULT_IGNORES)?,
    }];
    let mut pending = vec![root.to_owned()];
    while let Some(directory) = pending.pop() {
        let entries = directory_entries(&directory);
        while scopes
            .last()
            .is_some_and(|scope| !directory.starts_with(&scope.directory))
        {
            scopes.pop();
        }
        scopes.push(IgnoreScope {
            directory: directory.clone(),
            matcher: load_ignores(&directory, ".gitignore")?,
        });
        let mut dirs = Vec::new();
        for entry in entries {
            let path = entry.path();
            let relative = path.strip_prefix(root).map_err(io::Error::other)?;
            let Ok(kind) = entry.file_type() else {
                warn_excluded(relative, false);
                continue;
            };
            let is_dir = kind.is_dir() || (kind.is_symlink() && path.is_dir());
            if kind.is_symlink()
                || (!kind.is_file() && !kind.is_dir())
                || path == output
                || (is_dir && env == Some(path.as_path()))
                || protected
                    .matched_path_or_any_parents(&path, is_dir)
                    .is_ignore()
                || is_ignored(&path, is_dir, &scopes, &runpod)
            {
                warn_excluded(relative, is_dir);
                continue;
            }
            if is_dir {
                dirs.push(path);
            } else {
                archive.append_path_with_name(&path, relative)?;
            }
        }
        pending.extend(dirs.into_iter().rev());
    }
    Ok(())
}

fn append_environment(
    archive: &mut tar::Builder<GzEncoder<fs::File>>,
    env: &Path,
    output: &Path,
) -> Result<()> {
    let mut pending = vec![env.to_owned()];
    while let Some(directory) = pending.pop() {
        let mut dirs = Vec::new();
        for entry in directory_entries(&directory) {
            let path = entry.path();
            let Ok(kind) = entry.file_type() else {
                continue;
            };
            if kind.is_symlink() || path == output {
                continue;
            }
            if kind.is_dir() {
                dirs.push(path);
            } else if kind.is_file() {
                let relative = path.strip_prefix(env).map_err(io::Error::other)?;
                archive.append_path_with_name(&path, Path::new(ENV_DIR_NAME).join(relative))?;
            }
        }
        pending.extend(dirs.into_iter().rev());
    }
    Ok(())
}

/// Packages source files, optional dependency files, and the supplied manifest as tar.gz.
///
/// Nested .gitignore files apply in order; root .runpodignore wins over them. Protected
/// source paths cannot be re-included. Symlinks are omitted from both trees. Dependency
/// files use their own env/ namespace and bypass source ignore/credential filters.
///
/// # Errors
/// Returns ignore-pattern, JSON, path-resolution, file-read, or archive-write errors.
pub fn package_project(
    project_root: &Path,
    manifest: &Value,
    options: &PackageOptions,
) -> Result<Artifact> {
    let scratch = if options.output.is_none() {
        Some(tempfile::tempdir()?)
    } else {
        None
    };
    let output = match (&options.output, &scratch) {
        (Some(path), _) => path.clone(),
        (None, Some(scratch)) => scratch.path().join("artifact.tar.gz"),
        _ => unreachable!("automatic output always creates a temporary directory"),
    };
    let root = match fs::canonicalize(project_root) {
        Ok(path) => path,
        Err(error) if error.kind() == io::ErrorKind::NotFound => std::path::absolute(project_root)?,
        Err(error) => return Err(error.into()),
    };
    let env = options
        .env_dir
        .as_ref()
        .map(fs::canonicalize)
        .transpose()
        .or_else(|error| {
            if error.kind() == io::ErrorKind::NotFound {
                Ok(None)
            } else {
                Err(error)
            }
        })?;
    let file = fs::File::create(&output)?;
    let output_resolved = fs::canonicalize(&output)?;
    let mut archive = tar::Builder::new(GzEncoder::new(file, Compression::default()));
    archive.follow_symlinks(false);
    archive.sparse(false);
    append_source(&mut archive, &root, &output_resolved, env.as_deref())?;
    if let Some(env) = env.as_ref().filter(|p| p.is_dir()) {
        append_environment(&mut archive, env, &output_resolved)?;
    }
    let data = serde_json::to_vec_pretty(manifest)?;
    let mut header = tar::Header::new_gnu();
    header.set_size(data.len() as u64);
    header.set_mode(0o644);
    header.set_mtime(0);
    archive.append_data(&mut header, "runpod_manifest.json", data.as_slice())?;
    archive.into_inner()?.finish()?;
    Ok(Artifact {
        path: output,
        scratch,
    })
}
