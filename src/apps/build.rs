//! Dependency selection and analysis of installed wheel metadata for application builds.
use super::deploy::AppDefinition;
pub use super::installer::{
    BuildProgress, BuildTools, MANYLINUX_PLATFORMS, VendorOptions, sync_running_package,
};
use crate::Result;
use pep508_rs::{ExtraName, Requirement, VerbatimUrl};
use regex::Regex;
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs, io,
    path::{Component, Path, PathBuf},
    str::FromStr,
    sync::LazyLock,
};

/// Large distributions supplied by the built-in GPU images.
pub const SIZE_PROHIBITIVE_PACKAGES: &[&str] = &["torch", "torchvision", "torchaudio", "triton"];

/// Runtime dependency subset imported by the Python-compatible workers.
pub const RUNTIME_REQUIREMENTS: &[&str] = &[
    "aiohttp[speedups]",
    "aiohttp-retry",
    "backoff",
    "cloudpickle",
    "py-cpuinfo",
    "requests",
    "tomli",
    "tomlkit",
    "tqdm-loggable",
];
/// Additional serving dependencies for API resources.
pub const API_RUNTIME_REQUIREMENTS: &[&str] = &["fastapi[standard]", "uvicorn>=0.30"];
/// Maximum compressed artifact size accepted by the upstream deployment pipeline, in MiB.
pub const MAX_ARTIFACT_MB: u64 = 1500;
/// Exact upstream SDK source used when no explicit SDK tree or package override is provided.
pub const PINNED_SDK_SOURCE: &str = "https://github.com/runpod/runpod-python/archive/ba938cfa4e9486a2dc2c1d3264334a1f689c7ea5.tar.gz";

/// Inputs for constructing an environment for the Python-compatible remote runtime.
#[derive(Clone)]
pub struct BuildOptions {
    /// Remote interpreter version; local build tools can use a different version.
    pub python_version: String,
    /// User-excluded distributions, in requirement syntax.
    pub exclude: Vec<String>,
    /// Caller-owned scratch directory; automatic storage is owned by [`BuildResult`].
    pub scratch_dir: Option<PathBuf>,
    /// Explicit Python SDK package directory to overlay when `package_spec` is absent/empty.
    pub sdk_source: Option<PathBuf>,
    /// Captured `RUNPOD_PACKAGE_SPEC` override; takes precedence over `sdk_source` when nonempty.
    pub package_spec: Option<String>,
}
impl Default for BuildOptions {
    fn default() -> Self {
        Self {
            python_version: super::images::DEFAULT_PYTHON_VERSION.into(),
            exclude: Vec::new(),
            scratch_dir: None,
            sdk_source: None,
            package_spec: std::env::var("RUNPOD_PACKAGE_SPEC").ok(),
        }
    }
}
impl std::fmt::Debug for BuildOptions {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BuildOptions")
            .field("python_version", &self.python_version)
            .field("exclude_count", &self.exclude.len())
            .field("scratch_dir", &self.scratch_dir)
            .field("sdk_source", &self.sdk_source)
            .field("has_package_spec", &self.package_spec.is_some())
            .finish()
    }
}

/// Constructed dependency tree and the temporary storage owner needed while packaging it.
#[derive(Debug)]
pub struct BuildResult {
    env_dir: PathBuf,
    /// Runtime and retained user requirement strings, in installation order.
    pub requirements: Vec<String>,
    /// Sorted exclusions to record in the deployment manifest.
    pub excluded: Vec<String>,
    scratch: Option<tempfile::TempDir>,
}
impl BuildResult {
    /// Borrows the dependency directory while keeping temporary build storage alive.
    #[must_use]
    pub fn env_dir(&self) -> &Path {
        &self.env_dir
    }
    /// Retains automatic storage and transfers responsibility for deleting it to the caller.
    #[must_use]
    pub fn persist(mut self) -> PathBuf {
        if let Some(scratch) = self.scratch.take() {
            let _ = scratch.keep();
        }
        self.env_dir
    }
}

fn has_gpu(resource: &super::ResourceSpec) -> bool {
    resource
        .gpu
        .as_ref()
        .is_some_and(|gpu| !gpu.values().is_empty())
}

fn cpu_requirements_closure(
    project_root: &Path,
    app: &AppDefinition,
    env: &Path,
) -> Result<BTreeSet<String>> {
    let mut roots = requirements_txt(project_root)?;
    roots.extend(
        app.resources
            .iter()
            .filter(|r| !has_gpu(&r.spec))
            .flat_map(|r| r.spec.dependencies.iter().flatten())
            .cloned(),
    );
    installed_closure(env, &roots)
}

fn exclusions_for_cpu_workers(
    excluded: Vec<String>,
    user_exclude: &[String],
    cpu_needs: &BTreeSet<String>,
) -> Vec<String> {
    let user_excluded: BTreeSet<_> = user_exclude
        .iter()
        .map(|s| canonical_name(&requirement_name(s)))
        .collect();
    let kept: Vec<_> = excluded
        .into_iter()
        .filter(|name| {
            user_excluded.contains(name)
                || !SIZE_PROHIBITIVE_PACKAGES.contains(&name.as_str())
                || cpu_needs.contains(name)
        })
        .collect();
    for name in &kept {
        if SIZE_PROHIBITIVE_PACKAGES.contains(&name.as_str()) {
            tracing::warn!(package=%name,"cpu resources need an unbundled package; their workers install it at cold start, so give them container disk for it");
        }
    }
    kept
}

/// Installs and prunes a complete environment for the Python-compatible artifact format.
///
/// Runs external pip tools synchronously; use a blocking executor from async code. A
/// nonempty package override installs that SDK without its dependencies. Otherwise an
/// explicit SDK source is overlaid, or the pinned upstream revision is built/installed.
/// Runtime dependencies, user requirements, image exclusions, and CPU cold-start needs
/// follow the pinned Python SDK. Dropping automatic results removes their scratch tree.
///
/// # Errors
/// Returns unsupported-version, installer, callback, metadata, pruning, or overlay failures.
pub fn build_environment(
    app: &AppDefinition,
    project_root: &Path,
    options: &BuildOptions,
    tools: &BuildTools,
    mut progress: Option<&mut BuildProgress<'_>>,
) -> Result<BuildResult> {
    if !super::images::SUPPORTED_PYTHON_VERSIONS.contains(&options.python_version.as_str()) {
        return Err(super::AppError::Build(format!(
            "python {} is not supported (supported: {})",
            options.python_version,
            super::images::SUPPORTED_PYTHON_VERSIONS.join(", ")
        ))
        .into());
    }
    let (scratch_path, scratch) = if let Some(path) = &options.scratch_dir {
        (path.clone(), None)
    } else {
        let temp = tempfile::Builder::new().prefix("rp-build-").tempdir()?;
        (temp.path().to_owned(), Some(temp))
    };
    let env_dir = scratch_path.join("env");
    fs::create_dir_all(&env_dir)?;
    let user_requirements = collect_requirements(project_root, app)?;
    let auto_exclude = !app
        .resources
        .iter()
        .any(|r| r.spec.image.as_ref().is_some_and(|image| !image.is_empty()))
        && app.resources.iter().any(|r| has_gpu(&r.spec));
    let (vendored, mut excluded) = split_exclusions(
        &user_requirements,
        &ExclusionOptions {
            auto_exclude,
            extra_excludes: options.exclude.clone(),
        },
    );
    let mut requirements: Vec<String> = RUNTIME_REQUIREMENTS
        .iter()
        .map(|s| (*s).to_owned())
        .collect();
    if app
        .resources
        .iter()
        .any(|r| r.spec.kind == super::ResourceKind::Api)
    {
        requirements.extend(API_RUNTIME_REQUIREMENTS.iter().map(|s| (*s).to_owned()));
    }
    requirements.extend(vendored);
    let package_spec = options.package_spec.as_deref().filter(|s| !s.is_empty());
    let sdk_spec =
        package_spec.or_else(|| options.sdk_source.is_none().then_some(PINNED_SDK_SOURCE));
    let mut forward = |count, name: &str| match progress.as_mut() {
        Some(callback) => callback(count, name),
        None => Ok(()),
    };
    if let Some(spec) = sdk_spec {
        let requirement = tools.runpod_requirement(&scratch_path, Some(spec))?;
        tools.vendor(
            &env_dir,
            &[requirement],
            &options.python_version,
            &VendorOptions { no_deps: true },
            Some(&mut forward),
        )?;
    }
    tracing::info!(packages=requirements.len(),python_version=%options.python_version,excluded=?excluded,"vendoring runtime dependencies");
    tools.vendor(
        &env_dir,
        &requirements,
        &options.python_version,
        &VendorOptions::default(),
        Some(&mut forward),
    )?;
    let mut prune_names: BTreeSet<_> = options
        .exclude
        .iter()
        .map(|s| requirement_name(s))
        .collect();
    if auto_exclude {
        prune_names.extend(SIZE_PROHIBITIVE_PACKAGES.iter().map(|s| (*s).to_owned()));
    }
    let cpu_needs = if auto_exclude && app.resources.iter().any(|r| !has_gpu(&r.spec)) {
        Some(cpu_requirements_closure(project_root, app, &env_dir)?)
    } else {
        None
    };
    let provided: Vec<String> = if auto_exclude {
        SIZE_PROHIBITIVE_PACKAGES
            .iter()
            .map(|s| (*s).to_owned())
            .collect()
    } else {
        Vec::new()
    };
    let pruned = prune_excluded(
        &env_dir,
        &prune_names.iter().cloned().collect::<Vec<_>>(),
        &requirements,
        &provided,
    )?;
    if !pruned.is_empty() {
        tracing::info!(packages=?pruned,"removed image-provided or excluded dependencies");
        let pruned: BTreeSet<_> = pruned.into_iter().collect();
        excluded = excluded
            .into_iter()
            .chain(
                prune_names
                    .iter()
                    .map(|name| canonical_name(name))
                    .filter(|name| pruned.contains(name)),
            )
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect();
    }
    if let Some(needs) = cpu_needs {
        excluded = exclusions_for_cpu_workers(excluded, &options.exclude, &needs);
    }
    if package_spec.is_none()
        && let Some(source) = &options.sdk_source
    {
        sync_running_package(&env_dir, source)?;
    }
    Ok(BuildResult {
        env_dir,
        requirements,
        excluded,
        scratch,
    })
}

static REQUIREMENT_NAME: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^\s*([A-Za-z0-9][A-Za-z0-9._-]*)").expect("static requirement name pattern")
});
static NAME_SEPARATOR: LazyLock<Regex> =
    LazyLock::new(|| Regex::new("[-_.]+").expect("static distribution separator pattern"));
static ARCHIVE_NAME: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)^[a-z0-9][a-z0-9._-]*\.(whl|zip|tar\.gz|tgz)$")
        .expect("static archive-name pattern")
});

/// Extracts the distribution name used for first-requirement-wins deduplication.
#[must_use]
pub fn requirement_name(requirement: &str) -> String {
    REQUIREMENT_NAME
        .captures(requirement)
        .and_then(|c| c.get(1))
        .map_or(requirement, |m| m.as_str())
        .to_lowercase()
        .replace('_', "-")
}

fn canonical_name(name: &str) -> String {
    NAME_SEPARATOR
        .replace_all(&name.to_lowercase(), "-")
        .into_owned()
}

fn requirements_txt(root: &Path) -> Result<Vec<String>> {
    let path = root.join("requirements.txt");
    if !path.exists() {
        return Ok(Vec::new());
    }
    Ok(fs::read_to_string(path)?
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with(['#', '-']))
        .map(str::to_owned)
        .collect())
}

/// Collects project requirements followed by resource dependencies, keeping each name's first entry.
///
/// # Errors
/// Returns errors reading the UTF-8 requirements file.
pub fn collect_requirements(project_root: &Path, app: &AppDefinition) -> Result<Vec<String>> {
    let mut requirements = requirements_txt(project_root)?;
    requirements.extend(
        app.resources
            .iter()
            .flat_map(|r| r.spec.dependencies.iter().flatten())
            .cloned(),
    );
    let mut seen = BTreeSet::new();
    requirements.retain(|requirement| seen.insert(requirement_name(requirement)));
    Ok(requirements)
}

/// User exclusions and whether built-in GPU image packages may be omitted automatically.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ExclusionOptions {
    /// Explicit distribution specifications to omit from the artifact.
    pub extra_excludes: Vec<String>,
    /// Enable only when no custom image is used and a built-in GPU resource exists.
    pub auto_exclude: bool,
}
impl Default for ExclusionOptions {
    fn default() -> Self {
        Self {
            extra_excludes: Vec::new(),
            auto_exclude: true,
        }
    }
}

/// Returns retained requirement strings and sorted, deduplicated names actually excluded.
#[must_use]
pub fn split_exclusions(
    requirements: &[String],
    options: &ExclusionOptions,
) -> (Vec<String>, Vec<String>) {
    let mut excluded: BTreeSet<String> = options
        .extra_excludes
        .iter()
        .map(|s| requirement_name(s))
        .collect();
    if options.auto_exclude {
        excluded.extend(SIZE_PROHIBITIVE_PACKAGES.iter().map(|s| (*s).to_owned()));
    }
    let mut kept = Vec::new();
    let mut dropped = BTreeSet::new();
    for requirement in requirements {
        let name = requirement_name(requirement);
        if excluded.contains(&name) {
            dropped.insert(name);
        } else {
            kept.push(requirement.clone());
        }
    }
    (kept, dropped.into_iter().collect())
}

type ParsedRequirement = Requirement<VerbatimUrl>;
fn parse_requirement(input: &str) -> Option<ParsedRequirement> {
    if ARCHIVE_NAME.is_match(input.trim()) {
        return None;
    }
    ParsedRequirement::from_str(input).ok()
}

fn installed_dist_infos(env: &Path) -> Result<BTreeMap<String, PathBuf>> {
    let mut result = BTreeMap::new();
    let entries = match fs::read_dir(env) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(result),
        Err(error) => return Err(error.into()),
    };
    for entry in entries {
        let entry = entry?;
        let path = entry.path();
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if let Some(stem) = name.strip_suffix(".dist-info").filter(|_| path.is_dir()) {
            let boundary = stem
                .as_bytes()
                .windows(2)
                .position(|pair| pair[0] == b'-' && pair[1].is_ascii_digit())
                .unwrap_or(stem.len());
            result.insert(canonical_name(&stem[..boundary]), path);
        }
    }
    Ok(result)
}

fn requires_dist(dist_info: &Path) -> Result<Option<Vec<ParsedRequirement>>> {
    let metadata = dist_info.join("METADATA");
    if !metadata.is_file() {
        return Ok(None);
    }
    let data = fs::read(metadata)?;
    let text = String::from_utf8_lossy(&data);
    let mut headers: Vec<String> = Vec::new();
    let mut in_requires = false;
    for line in text.lines() {
        if line.is_empty() {
            break;
        }
        if line.starts_with([' ', '\t']) {
            if in_requires && let Some(last) = headers.last_mut() {
                last.push(' ');
                last.push_str(line.trim());
            }
        } else if let Some((name, value)) = line.split_once(':') {
            in_requires = name.eq_ignore_ascii_case("Requires-Dist");
            if in_requires {
                headers.push(value.trim().to_owned());
            }
        } else {
            break;
        }
    }
    Ok(headers.iter().map(|line| parse_requirement(line)).collect())
}

fn marker_applies(requirement: &ParsedRequirement, extras: &BTreeSet<ExtraName>) -> bool {
    // Each extra is evaluated independently, plus the empty-extra context. Combining
    // extras at once would incorrectly activate `extra == "a" and extra == "b"`.
    requirement.marker.evaluate_optional_environment(None, &[])
        || extras.iter().any(|extra| {
            requirement
                .marker
                .evaluate_optional_environment(None, std::slice::from_ref(extra))
        })
}

/// Computes installed dependencies pulled in by roots, preserving transitive extra activation.
///
/// Non-extra environment markers are conservatively true: local Python/OS information
/// does not describe the worker. Missing/invalid metadata or unnamed roots conservatively
/// include every installed distribution, together with known root names.
///
/// # Errors
/// Returns errors reading installed distribution directories or metadata files.
pub fn installed_closure(env_dir: &Path, roots: &[String]) -> Result<BTreeSet<String>> {
    let mut requires = BTreeMap::new();
    for (name, path) in installed_dist_infos(env_dir)? {
        requires.insert(name, requires_dist(&path)?);
    }
    let parsed: Vec<_> = roots.iter().map(|r| parse_requirement(r)).collect();
    let mut stack: Vec<_> = parsed
        .iter()
        .flatten()
        .map(|r| {
            (
                r.name.to_string(),
                r.extras.iter().cloned().collect::<BTreeSet<_>>(),
            )
        })
        .collect();
    if parsed.iter().any(Option::is_none) || requires.values().any(Option::is_none) {
        return Ok(requires
            .into_keys()
            .chain(stack.into_iter().map(|(name, _)| name))
            .collect());
    }
    let mut activated: BTreeMap<String, BTreeSet<ExtraName>> = BTreeMap::new();
    while let Some((name, extras)) = stack.pop() {
        if activated
            .get(&name)
            .is_some_and(|known| extras.is_subset(known))
        {
            continue;
        }
        let active = activated.entry(name.clone()).or_default();
        active.extend(extras);
        for req in requires
            .get(&name)
            .and_then(Option::as_ref)
            .into_iter()
            .flatten()
        {
            if marker_applies(req, active) {
                stack.push((req.name.to_string(), req.extras.iter().cloned().collect()));
            }
        }
    }
    Ok(activated.into_keys().collect())
}

fn normalized_record_path(path: &str) -> Option<String> {
    let path = path.replace('\\', "/");
    // Preserve the distinction between rooted and relative records before collapsing
    // separators; otherwise `/keep.txt` could incorrectly delete `env/keep.txt`.
    if path.starts_with('/') || path.as_bytes().get(1) == Some(&b':') {
        return None;
    }
    let mut parts = Vec::new();
    for part in path.split('/') {
        match part {
            "" | "." => {}
            ".." if parts.last().is_some_and(|p| *p != "..") => {
                parts.pop();
            }
            part => parts.push(part),
        }
    }
    let path = parts.join("/");
    let path = path.strip_prefix("../../").unwrap_or(&path);
    if path.is_empty() || path == ".." || path.starts_with("../") {
        None
    } else {
        Some(path.to_owned())
    }
}

fn read_record(dist_info: &Path) -> Result<Option<BTreeSet<String>>> {
    let path = dist_info.join("RECORD");
    if !path.is_file() {
        return Ok(None);
    }
    let bytes = fs::read(path)?;
    let text = String::from_utf8_lossy(&bytes);
    let mut reader = csv::ReaderBuilder::new()
        .has_headers(false)
        .flexible(true)
        .from_reader(text.as_bytes());
    let mut paths = BTreeSet::new();
    for row in reader.records() {
        if let Some(path) = row?.get(0).and_then(normalized_record_path) {
            paths.insert(path);
        }
    }
    Ok(Some(paths))
}

fn dependency_closure(
    requires: &BTreeMap<String, BTreeSet<String>>,
    start: &BTreeSet<String>,
    stop: &BTreeSet<String>,
) -> BTreeSet<String> {
    let mut seen = BTreeSet::new();
    let mut stack: Vec<_> = start
        .iter()
        .filter(|name| requires.contains_key(*name))
        .cloned()
        .collect();
    while let Some(name) = stack.pop() {
        if seen.insert(name.clone()) {
            stack.extend(
                requires[&name]
                    .iter()
                    .filter(|name| requires.contains_key(*name) && !stop.contains(*name))
                    .cloned(),
            );
        }
    }
    seen
}

fn distributions_to_drop(
    infos: &BTreeMap<String, PathBuf>,
    records: &BTreeMap<String, Option<BTreeSet<String>>>,
    excluded: &BTreeSet<String>,
    roots: &[String],
    image_provided: &[String],
) -> Result<BTreeSet<String>> {
    let installed: BTreeSet<_> = infos.keys().cloned().collect();
    let targets: BTreeSet<_> = installed.intersection(excluded).cloned().collect();
    let parsed: Option<Vec<_>> = roots.iter().map(|r| parse_requirement(r)).collect();
    let Some(parsed) = parsed.filter(|_| records.values().all(Option::is_some)) else {
        return Ok(targets);
    };
    let root_names: BTreeSet<_> = parsed.iter().map(|r| r.name.to_string()).collect();
    let mut requires = BTreeMap::new();
    for (name, path) in infos {
        let Some(deps) = requires_dist(path)? else {
            return Ok(targets);
        };
        requires.insert(
            name.clone(),
            deps.into_iter().map(|r| r.name.to_string()).collect(),
        );
    }
    let provided: BTreeSet<_> = image_provided.iter().map(|s| canonical_name(s)).collect();
    let pulled = dependency_closure(
        &requires,
        &installed.intersection(&provided).cloned().collect(),
        &BTreeSet::new(),
    );
    let mut seed: BTreeSet<_> = installed
        .difference(&pulled)
        .chain(installed.intersection(&root_names))
        .cloned()
        .collect();
    seed.extend(
        installed
            .intersection(&excluded.difference(&provided).cloned().collect())
            .cloned(),
    );
    let kept = dependency_closure(
        &requires,
        &seed.difference(&provided).cloned().collect(),
        &provided,
    );
    Ok(targets
        .into_iter()
        .chain(installed.difference(&kept).cloned())
        .collect())
}

fn contained_path(root: &Path, path: &Path) -> bool {
    let resolved = fs::canonicalize(path).or_else(|error| {
        if error.kind() != io::ErrorKind::NotFound {
            return Err(error);
        }
        match (path.parent(), path.file_name()) {
            (Some(parent), Some(name)) => fs::canonicalize(parent).map(|parent| parent.join(name)),
            _ => Err(error),
        }
    });
    resolved.is_ok_and(|path| path.starts_with(root) && path != root)
}

fn remove_files(root: &Path, files: &BTreeSet<String>) -> Result<BTreeSet<String>> {
    let mut touched = BTreeSet::new();
    for rel in files {
        // Wheel records are relative paths. Reject rooted/drive paths before joining.
        if Path::new(rel)
            .components()
            .any(|c| !matches!(c, Component::Normal(_)))
        {
            continue;
        }
        let path = root.join(rel);
        if contained_path(root, &path) && (path.is_file() || path.is_symlink()) {
            fs::remove_file(path)?;
            if let Some(top) = rel.split('/').next() {
                touched.insert(top.to_owned());
            }
        }
    }
    Ok(touched)
}

fn remove_empty_dirs(root: &Path, tops: &BTreeSet<String>) -> Result<()> {
    let mut dirs = Vec::new();
    let mut pending: Vec<_> = tops.iter().map(|top| root.join(top)).collect();
    while let Some(path) = pending.pop() {
        if !path.is_dir() || path.is_symlink() || !contained_path(root, &path) {
            continue;
        }
        for entry in fs::read_dir(&path)? {
            let entry = entry?;
            if entry.file_type()?.is_dir() {
                pending.push(entry.path());
            }
        }
        dirs.push(path);
    }
    dirs.sort_by_key(|path| std::cmp::Reverse(path.components().count()));
    for dir in dirs {
        if fs::read_dir(&dir)?.next().is_none() {
            fs::remove_dir(dir)?;
        }
    }
    Ok(())
}

/// Prunes excluded wheels and dependencies needed only by packages supplied by the image.
///
/// Files shared with retained distributions survive. Incomplete metadata limits removal
/// to named exclusions. All deletion targets must resolve inside the supplied environment.
///
/// # Errors
/// Returns directory, metadata, CSV record, or file deletion errors.
pub fn prune_excluded(
    env_dir: &Path,
    excluded: &[String],
    roots: &[String],
    image_provided: &[String],
) -> Result<Vec<String>> {
    let infos = installed_dist_infos(env_dir)?;
    let excluded: BTreeSet<_> = excluded.iter().map(|s| canonical_name(s)).collect();
    if !infos.keys().any(|name| excluded.contains(name)) {
        return Ok(Vec::new());
    }
    let root = fs::canonicalize(env_dir)?;
    let mut records = BTreeMap::new();
    for (name, path) in &infos {
        records.insert(name.clone(), read_record(path)?);
    }
    let dropped = distributions_to_drop(&infos, &records, &excluded, roots, image_provided)?;
    let mut drop_files = BTreeSet::new();
    let mut keep_files = BTreeSet::new();
    for (name, paths) in &records {
        let set = if dropped.contains(name) {
            &mut drop_files
        } else {
            &mut keep_files
        };
        set.extend(paths.iter().flatten().cloned());
    }
    let touched = remove_files(
        &root,
        &drop_files.difference(&keep_files).cloned().collect(),
    )?;
    for name in &dropped {
        let info = &infos[name];
        if !info.is_symlink() && contained_path(&root, info) {
            let _ = fs::remove_dir_all(info);
        }
        let fallback = root.join(name.replace('-', "_"));
        if records[name].is_none()
            && fallback.is_dir()
            && !fallback.is_symlink()
            && contained_path(&root, &fallback)
        {
            let _ = fs::remove_dir_all(fallback);
        }
    }
    remove_empty_dirs(&root, &touched)?;
    Ok(dropped.into_iter().collect())
}
