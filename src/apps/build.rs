//! Dependency selection and analysis of installed wheel metadata for application builds.
use super::deploy::AppDefinition;
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
