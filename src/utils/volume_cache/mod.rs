//! Directory mirrors on a mounted volume, with packed small files and ranged hydration.
mod archive;
#[cfg(test)]
mod tests;
mod transfer;

use crate::{Error, Result, Value};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    io::{self, Write},
    num::NonZeroUsize,
    path::{Component, Path, PathBuf},
    sync::{Arc, Mutex, mpsc},
    thread,
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use transfer::{copy_file, hydrate_ranged, realpath, run_bounded, within};

const THRESHOLD: u64 = 256 * 1024;
const TOLERANCE: f64 = 2.0;
const JOIN_TIMEOUT: Duration = Duration::from_secs(30);

/// Configuration captured when constructing a [`VolumeCache`].
#[derive(Debug, Clone)]
pub struct VolumeCacheConfig {
    /// Isolation component; an absent/empty value falls back to `RUNPOD_ENDPOINT_ID`.
    pub namespace: Option<String>,
    /// Existing mounted directory; defaults to `/runpod-volume`.
    pub volume_path: PathBuf,
    /// Log operation errors and return zero instead of propagating them. Defaults to true.
    pub best_effort: bool,
    /// Concurrent large-file uploads; default min(32, available CPUs * 4).
    pub max_workers: Option<NonZeroUsize>,
    /// Concurrent hydrate ranges; defaults to explicit `max_workers`, otherwise 64.
    pub hydrate_workers: Option<NonZeroUsize>,
    /// Bytes per hydrate range; defaults to 8 MiB. Memory is bounded by workers * chunk.
    pub hydrate_chunk_size: Option<NonZeroUsize>,
}
impl Default for VolumeCacheConfig {
    fn default() -> Self {
        Self {
            namespace: None,
            volume_path: "/runpod-volume".into(),
            best_effort: true,
            max_workers: None,
            hydrate_workers: None,
            hydrate_chunk_size: None,
        }
    }
}

/// Owned warm cache for one or more local directories.
///
/// `hydrate` and `sync_inline` perform blocking filesystem I/O; call them outside async
/// executor threads. `sync` starts background work. Clones share pending syncs; dropping
/// the last clone waits up to 30 seconds per pending sync, then detaches unfinished work.
/// Keep the cache alive and call `flush` before process exit. Rust process termination
/// does not run destructors or a Python-style global exit hook.
#[derive(Debug, Clone)]
pub struct VolumeCache {
    inner: Arc<Cache>,
    pending: Arc<Pending>,
}
#[derive(Debug)]
struct Cache {
    dirs: Vec<PathBuf>,
    namespace: String,
    volume: PathBuf,
    best_effort: bool,
    workers: usize,
    hydrate_workers: usize,
    chunk: usize,
}

/// Hydrates on creation and schedules a background sync on drop, including unwinding.
#[derive(Debug)]
pub struct VolumeCacheGuard<'a> {
    cache: &'a VolumeCache,
}
impl std::ops::Deref for VolumeCacheGuard<'_> {
    type Target = VolumeCache;
    fn deref(&self) -> &Self::Target {
        self.cache
    }
}
impl Drop for VolumeCacheGuard<'_> {
    fn drop(&mut self) {
        if let Err(error) = self.cache.sync() {
            tracing::warn!(%error,"VolumeCache: could not schedule scope sync");
        }
    }
}

#[derive(Debug, Default)]
struct Pending {
    threads: Mutex<Vec<PendingSync>>,
}
#[derive(Debug)]
struct PendingSync {
    thread: thread::JoinHandle<()>,
    done: mpsc::Receiver<()>,
}
impl Pending {
    fn flush(&self, timeout: Duration) -> Result<usize> {
        let pending =
            std::mem::take(&mut *self.threads.lock().map_err(|_| {
                Error::InvalidInput("volume-cache pending sync state poisoned".into())
            })?);
        let mut unfinished = 0;
        for sync in pending {
            if sync.done.recv_timeout(timeout) == Err(mpsc::RecvTimeoutError::Timeout) {
                unfinished += 1;
                tracing::warn!(
                    ?timeout,
                    "VolumeCache: background sync did not finish; cache may be incomplete"
                );
            } else if sync.thread.join().is_err() {
                tracing::warn!("VolumeCache: background sync panicked");
            }
        }
        Ok(unfinished)
    }
}
impl Drop for Pending {
    fn drop(&mut self) {
        if let Err(error) = self.flush(JOIN_TIMEOUT) {
            tracing::warn!(%error,"VolumeCache: sync cleanup failed");
        }
    }
}

impl VolumeCache {
    /// Normalizes directory paths and validates the namespace without creating the volume.
    /// Pass a single path as `[path]`; native path and positive-integer types prevent Python
    /// scalar/bytes iteration and invalid worker-count ambiguities.
    ///
    /// # Errors
    /// Returns unsafe namespace or path-resolution errors, even in best-effort mode.
    pub fn new<P: AsRef<Path>>(
        dirs: impl IntoIterator<Item = P>,
        config: VolumeCacheConfig,
    ) -> Result<Self> {
        let namespace = config
            .namespace
            .filter(|s| !s.is_empty())
            .or_else(|| std::env::var("RUNPOD_ENDPOINT_ID").ok())
            .unwrap_or_default();
        if !namespace.is_empty()
            && (Path::new(&namespace).is_absolute()
                || Path::new(&namespace)
                    .components()
                    .any(|c| !matches!(c, Component::Normal(_)))
                || namespace.contains(['/', '\\'])
                || matches!(namespace.as_str(), "." | ".."))
        {
            return Err(Error::InvalidInput(format!(
                "namespace must be a single safe path component, got {namespace:?}"
            )));
        }
        let workers = config.max_workers.map_or_else(
            || {
                thread::available_parallelism()
                    .map_or(4, NonZeroUsize::get)
                    .saturating_mul(4)
                    .min(32)
            },
            NonZeroUsize::get,
        );
        let inner = Cache {
            dirs: dirs
                .into_iter()
                .map(|p| realpath(p.as_ref()))
                .collect::<std::io::Result<_>>()?,
            namespace,
            volume: config.volume_path,
            best_effort: config.best_effort,
            workers,
            hydrate_workers: config
                .hydrate_workers
                .or(config.max_workers)
                .map_or(64, NonZeroUsize::get),
            chunk: config
                .hydrate_chunk_size
                .map_or(8 * 1024 * 1024, NonZeroUsize::get),
        };
        Ok(Self {
            inner: Arc::new(inner),
            pending: Arc::new(Pending::default()),
        })
    }
    /// Whether a namespace is configured and the volume directory currently exists.
    #[must_use]
    pub fn available(&self) -> bool {
        self.inner.available()
    }
    /// Restore missing or older local files, returning the number restored.
    ///
    /// # Errors
    /// With best-effort disabled, propagates operation-level errors. Individual failed
    /// files are always logged/skipped, matching the source cache.
    pub fn hydrate(&self) -> Result<usize> {
        if !self.available() {
            return Ok(0);
        }
        self.inner.guard(|| self.inner.hydrate())
    }
    /// Copy local changes synchronously, returning the number of changed entries copied.
    /// Unchanged archive members repacked after a deletion do not increase this count.
    ///
    /// # Errors
    /// With best-effort disabled, propagates operation-level failures. Failed individual
    /// copies are omitted from the manifest instead of claiming a completed transfer.
    pub fn sync_inline(&self) -> Result<usize> {
        if !self.available() {
            return Ok(0);
        }
        self.inner.guard(|| self.inner.sync())
    }
    /// Schedule a background sync; unavailable caches are a no-op.
    ///
    /// # Errors
    /// Returns thread-start or poisoned registry errors. Background operation errors are logged.
    pub fn sync(&self) -> Result<()> {
        if !self.available() {
            return Ok(());
        }
        let cache = self.inner.clone();
        let (sent, done) = mpsc::channel();
        let thread = thread::Builder::new()
            .name("runpod-volume-sync".into())
            .spawn(move || {
                if let Err(error) = cache.guard(|| cache.sync()) {
                    tracing::warn!(%error,"VolumeCache: background sync failed");
                }
                let _ = sent.send(());
            })?;
        let mut pending =
            self.pending.threads.lock().map_err(|_| {
                Error::InvalidInput("volume-cache pending sync state poisoned".into())
            })?;
        pending.retain(|p| !p.thread.is_finished());
        pending.push(PendingSync { thread, done });
        Ok(())
    }
    /// Wait at most `timeout` per currently pending sync, returning how many were detached.
    /// Syncs scheduled concurrently after this snapshot remain registered for a later flush.
    ///
    /// # Errors
    /// Returns a poisoned background registry error; individual thread failures are logged.
    pub fn flush(&self, timeout: Duration) -> Result<usize> {
        self.pending.flush(timeout)
    }
    /// Hydrate before entering a scope; the returned guard schedules sync on drop.
    ///
    /// # Errors
    /// Returns hydration errors when best-effort is disabled.
    pub fn enter(&self) -> Result<VolumeCacheGuard<'_>> {
        self.hydrate()?;
        Ok(VolumeCacheGuard { cache: self })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct FileMeta {
    path: PathBuf,
    size: u64,
    mtime: f64,
}
#[derive(Debug, Default, Serialize, Deserialize)]
struct Manifest {
    #[serde(default)]
    small: Vec<FileMeta>,
    #[serde(default)]
    big: Vec<FileMeta>,
}
#[derive(Serialize)]
struct ManifestOutput<'a> {
    version: u32,
    threshold: u64,
    small: &'a [FileMeta],
    big: &'a [FileMeta],
}
struct AsciiJson;
impl serde_json::ser::Formatter for AsciiJson {
    fn write_string_fragment<W: ?Sized + Write>(
        &mut self,
        writer: &mut W,
        fragment: &str,
    ) -> io::Result<()> {
        for character in fragment.chars() {
            if character.is_ascii() {
                writer.write_all(character.encode_utf8(&mut [0; 4]).as_bytes())?;
            } else {
                for unit in character.encode_utf16(&mut [0; 2]) {
                    write!(writer, "\\u{unit:04x}")?;
                }
            }
        }
        Ok(())
    }
}
impl Cache {
    fn available(&self) -> bool {
        !self.namespace.is_empty() && self.volume.is_dir()
    }
    fn mirror_root(&self) -> PathBuf {
        self.volume.join(".cache").join(&self.namespace)
    }
    fn manifest_path(&self) -> PathBuf {
        self.mirror_root().join("manifest.json")
    }
    fn archive_path(&self) -> PathBuf {
        self.mirror_root().join("small.tar")
    }
    fn big_root(&self) -> PathBuf {
        self.mirror_root().join("big")
    }
    fn guard(&self, operation: impl FnOnce() -> Result<usize>) -> Result<usize> {
        match operation() {
            Err(error) if self.best_effort => {
                tracing::warn!(%error,"VolumeCache operation failed");
                Ok(0)
            }
            result => result,
        }
    }
    fn read_manifest(&self) -> Result<Option<Manifest>> {
        let Some(value) = fs::read(self.manifest_path())
            .ok()
            .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
        else {
            return Ok(None);
        };
        if !value.is_object() || value["version"].as_f64() != Some(1.0) {
            return Ok(None);
        }
        Ok(Some(serde_json::from_value(value)?))
    }
    fn write_manifest(&self, manifest: &Manifest) -> Result<()> {
        fs::create_dir_all(self.mirror_root())?;
        let mut file = transfer::temporary(&self.manifest_path())?;
        {
            // Python opens manifests using the platform encoding and writes ensure_ascii=True.
            let mut writer = io::BufWriter::new(file.as_file_mut());
            ManifestOutput {
                version: 1,
                threshold: THRESHOLD,
                small: &manifest.small,
                big: &manifest.big,
            }
            .serialize(&mut serde_json::Serializer::with_formatter(
                &mut writer,
                AsciiJson,
            ))?;
            writer.flush()?;
        }
        file.persist(self.manifest_path()).map_err(|e| e.error)?;
        Ok(())
    }
    fn safe_dest(&self, path: &Path) -> bool {
        realpath(path).is_ok_and(|target| {
            self.dirs
                .iter()
                .any(|d| target == *d || (d.parent().is_some() && target.starts_with(d)))
        })
    }
    fn sync(&self) -> Result<usize> {
        let paths = self
            .dirs
            .iter()
            .flat_map(|root| iter_files(root))
            .collect::<Vec<_>>();
        let (small, big): (Vec<_>, Vec<_>) = paths
            .into_iter()
            .filter_map(|p| file_meta(&p))
            .partition(|m| m.size < THRESHOLD);
        let mut small = small;
        let mut big: Vec<_> = big.into_iter().map(|m| m.path).collect();
        let prior = self.read_manifest()?.unwrap_or_default();
        let mut small_transferred = 0;
        if !small.is_empty() {
            let changed = changed_vs_manifest(&small, &prior.small);
            let current: BTreeSet<_> = small.iter().map(|m| &m.path).collect();
            let deleted = prior.small.iter().any(|m| !current.contains(&m.path));
            if changed > 0 || deleted {
                if self.pack_small(&small) {
                    small_transferred = changed;
                } else {
                    big.extend(small.drain(..).map(|m| m.path));
                }
            }
        }
        if small.is_empty() {
            let _ = fs::remove_file(self.archive_path());
        }
        let prior_big: BTreeMap<_, _> = prior.big.iter().map(|m| (&m.path, m)).collect();
        let pairs = big
            .iter()
            .map(|p| Ok((p, self.big_root().join(relative_root(p)?))))
            .collect::<Result<Vec<_>>>()?;
        let mut current = BTreeSet::new();
        let mut todo = Vec::new();
        for (src, dst) in pairs {
            if sync_needs_copy(src, &dst, prior_big.get(src).copied()) {
                todo.push((src, dst));
            } else {
                current.insert(src.clone());
            }
        }
        let copied = Mutex::new(BTreeSet::new());
        run_bounded(todo.into_iter(), self.workers, |(src, dst)| {
            if copy_file(src, &dst) {
                copied
                    .lock()
                    .map_err(|_| std::io::Error::other("copy state poisoned"))?
                    .insert(src.clone());
            }
            Ok(())
        })?;
        let copied = copied
            .into_inner()
            .map_err(|_| std::io::Error::other("copy state poisoned"))?;
        let total = copied.len() + small_transferred;
        current.extend(copied);
        let big = big
            .into_iter()
            .filter(|p| current.contains(p))
            .filter_map(|p| file_meta(&p))
            .collect();
        self.write_manifest(&Manifest { small, big })?;
        if total > 0 {
            tracing::info!(files=total,path=%self.mirror_root().display(),"VolumeCache: synced files");
        }
        Ok(total)
    }
    fn hydrate(&self) -> Result<usize> {
        let Some(manifest) = self.read_manifest()? else {
            return Ok(0);
        };
        let small = if manifest.small.is_empty() {
            0
        } else {
            self.extract_small()
        };
        let mut entries = Vec::new();
        for entry in manifest.big {
            let src = self.big_root().join(relative_root(&entry.path)?);
            if self.safe_dest(&entry.path)
                && within(&self.big_root(), &src)
                && src.is_file()
                && !meta_satisfied(&entry)
            {
                entries.push((src, entry));
            }
        }
        let restored = small + hydrate_ranged(&entries, self.hydrate_workers, self.chunk)?;
        if restored > 0 {
            tracing::info!(files=restored,path=%self.mirror_root().display(),"VolumeCache: hydrated files");
        }
        Ok(restored)
    }
}

fn relative_root(path: &Path) -> Result<PathBuf> {
    let path = std::path::absolute(path)?;
    #[cfg(windows)]
    {
        let cwd = std::env::current_dir()?;
        let drive = |p: &Path| {
            p.components()
                .next()
                .map(|c| c.as_os_str().to_string_lossy().to_ascii_lowercase())
        };
        if drive(&path) != drive(&cwd) {
            return Err(Error::InvalidInput(
                "cache paths must be on the current filesystem drive".into(),
            ));
        }
    }
    let mut relative = PathBuf::new();
    for component in path.components() {
        match component {
            Component::Normal(name) => relative.push(name),
            Component::ParentDir => {
                relative.pop();
            }
            _ => {}
        }
    }
    Ok(relative)
}
fn file_meta(path: &Path) -> Option<FileMeta> {
    let meta = fs::metadata(path).ok()?;
    Some(FileMeta {
        path: path.into(),
        size: meta.len(),
        mtime: seconds(meta.modified().ok()?),
    })
}
fn seconds(time: SystemTime) -> f64 {
    match time.duration_since(UNIX_EPOCH) {
        Ok(d) => d.as_secs_f64(),
        Err(e) => -e.duration().as_secs_f64(),
    }
}
fn timestamp(seconds: f64) -> std::io::Result<SystemTime> {
    let duration = Duration::try_from_secs_f64(seconds.abs()).map_err(std::io::Error::other)?;
    if seconds < 0.0 {
        UNIX_EPOCH.checked_sub(duration)
    } else {
        UNIX_EPOCH.checked_add(duration)
    }
    .ok_or_else(|| std::io::Error::other("cache timestamp out of range"))
}
fn meta_satisfied(meta: &FileMeta) -> bool {
    !needs_copy(Some(meta), file_meta(&meta.path).as_ref())
}
fn needs_copy(source: Option<&FileMeta>, destination: Option<&FileMeta>) -> bool {
    source.is_some_and(|source| {
        destination.is_none_or(|destination| {
            source.size != destination.size || source.mtime > destination.mtime + TOLERANCE
        })
    })
}
fn changed_vs_manifest(metas: &[FileMeta], prior: &[FileMeta]) -> usize {
    let prior: BTreeMap<_, _> = prior.iter().map(|m| (&m.path, m)).collect();
    metas
        .iter()
        .filter(|m| {
            prior
                .get(&m.path)
                .is_none_or(|p| p.size != m.size || m.mtime > p.mtime + TOLERANCE)
        })
        .count()
}
fn sync_needs_copy(src: &Path, dst: &Path, prior: Option<&FileMeta>) -> bool {
    let Some(source) = file_meta(src) else {
        return false;
    };
    let Some(mirrored) = file_meta(dst) else {
        return true;
    };
    source.size != mirrored.size
        || prior.map_or((source.mtime - mirrored.mtime).abs() > TOLERANCE, |p| {
            p.size != source.size || source.mtime > p.mtime + TOLERANCE
        })
}
fn iter_files(root: &Path) -> Vec<PathBuf> {
    fn walk(root: &Path, out: &mut Vec<PathBuf>) {
        let Ok(entries) = fs::read_dir(root) else {
            return;
        };
        let mut dirs = Vec::new();
        for entry in entries.flatten() {
            let Ok(kind) = entry.file_type() else {
                continue;
            };
            let path = entry.path();
            if kind.is_symlink() {
                continue;
            }
            if kind.is_dir() {
                dirs.push(path);
                continue;
            }
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if name.ends_with(".lock")
                || name.ends_with(".rpvc.tmp")
                || root
                    .components()
                    .any(|c| c.as_os_str() == "refs" || c.as_os_str() == ".no_exist")
            {
                continue;
            }
            out.push(path);
        }
        for dir in dirs {
            walk(&dir, out);
        }
    }
    let mut files = Vec::new();
    walk(root, &mut files);
    files
}
