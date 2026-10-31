use super::{FileMeta, timestamp};
use std::{
    fs::{self, File, FileTimes, Metadata, OpenOptions},
    io::{self, Read, Seek, SeekFrom, Write},
    path::{Component, Path, PathBuf},
    sync::{
        Mutex,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::{Duration, SystemTime},
};

pub(super) fn realpath(path: &Path) -> io::Result<PathBuf> {
    let mut resolved = PathBuf::new();
    for component in std::path::absolute(path)?.components() {
        match component {
            Component::ParentDir => {
                resolved.pop();
            }
            Component::CurDir => {}
            component => {
                resolved.push(component.as_os_str());
                match fs::canonicalize(&resolved) {
                    Ok(path) => resolved = normal_path(path),
                    Err(e) if e.kind() == io::ErrorKind::NotFound => {}
                    Err(e) => return Err(e),
                }
            }
        }
    }
    Ok(resolved)
}
#[cfg(not(windows))]
fn normal_path(path: PathBuf) -> PathBuf {
    path
}
#[cfg(windows)]
fn normal_path(path: PathBuf) -> PathBuf {
    use std::path::Prefix;
    if !matches!(path.components().next(),Some(Component::Prefix(prefix)) if matches!(prefix.kind(),Prefix::VerbatimDisk(_)|Prefix::VerbatimUNC(_, _)))
    {
        return path;
    }
    let mut result = PathBuf::new();
    for component in path.components() {
        match component {
            Component::Prefix(prefix) => match prefix.kind() {
                Prefix::VerbatimDisk(drive) => result.push(format!("{}:", char::from(drive))),
                Prefix::VerbatimUNC(server, share) => {
                    result.push(r"\\");
                    result.push(server);
                    result.push(share);
                }
                _ => result.push(prefix.as_os_str()),
            },
            other => result.push(other.as_os_str()),
        }
    }
    result
}
pub(super) fn within(base: &Path, path: &Path) -> bool {
    realpath(base)
        .ok()
        .zip(realpath(path).ok())
        .is_some_and(|(base, path)| {
            path == base || (base.parent().is_some() && path.starts_with(base))
        })
}

pub(super) fn temporary(destination: &Path) -> io::Result<tempfile::NamedTempFile> {
    tempfile::Builder::new()
        .prefix(".runpod-")
        .suffix(".rpvc.tmp")
        .tempfile_in(
            destination
                .parent()
                .ok_or_else(|| io::Error::other("cache destination has no parent"))?,
        )
}
pub(super) fn copy_metadata(src: &Path, dst: &Path, mtime: Option<f64>) {
    let times = (|| -> io::Result<_> {
        if let Some(mtime) = mtime {
            let time = timestamp(mtime)?;
            Ok(FileTimes::new().set_accessed(time).set_modified(time))
        } else {
            let meta = fs::metadata(src)?;
            Ok(FileTimes::new()
                .set_accessed(meta.accessed()?)
                .set_modified(meta.modified()?))
        }
    })();
    if let Err(error) =
        times.and_then(|times| OpenOptions::new().write(true).open(dst)?.set_times(times))
    {
        tracing::debug!(%error,path=%dst.display(),"VolumeCache: kept copy-time mtime");
    }
    if let Err(error) =
        fs::metadata(src).and_then(|meta| fs::set_permissions(dst, meta.permissions()))
    {
        tracing::debug!(%error,path=%dst.display(),"VolumeCache: kept default mode");
    }
}
pub(super) fn copy_file(src: &Path, dst: &Path) -> bool {
    let result = (|| -> io::Result<()> {
        fs::create_dir_all(
            dst.parent()
                .ok_or_else(|| io::Error::other("cache destination has no parent"))?,
        )?;
        let mut temp = temporary(dst)?;
        if !fs::metadata(src)?.is_file() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "cache copy source is not a regular file",
            ));
        }
        let mut source = File::open(src)?;
        io::copy(&mut source, temp.as_file_mut())?;
        copy_metadata(src, temp.path(), None);
        temp.persist(dst).map_err(|e| e.error)?;
        Ok(())
    })();
    if let Err(error) = result {
        tracing::warn!(%error,source=%src.display(),destination=%dst.display(),"VolumeCache: failed to copy");
        false
    } else {
        true
    }
}

#[derive(Debug, PartialEq, Eq, Clone, Copy)]
struct Identity {
    size: u64,
    mtime: SystemTime,
}
fn identity(meta: &Metadata) -> io::Result<Identity> {
    Ok(Identity {
        size: meta.len(),
        mtime: meta.modified()?,
    })
}
struct RangedFile {
    src: PathBuf,
    entry: FileMeta,
    temp: tempfile::NamedTempFile,
    identity: Identity,
    failed: AtomicBool,
}
impl RangedFile {
    fn prepare(src: &Path, entry: &FileMeta) -> io::Result<Self> {
        let identity = identity(&fs::metadata(src)?)?;
        if identity.size != entry.size {
            return Err(io::Error::other(format!(
                "mirror size {} != manifest size {}",
                identity.size, entry.size
            )));
        }
        fs::create_dir_all(
            entry
                .path
                .parent()
                .ok_or_else(|| io::Error::other("cache destination has no parent"))?,
        )?;
        let temp = temporary(&entry.path)?;
        temp.as_file().set_len(entry.size)?;
        Ok(Self {
            src: src.into(),
            entry: entry.clone(),
            temp,
            identity,
            failed: AtomicBool::new(false),
        })
    }
    fn finish(self) -> bool {
        if self.failed.load(Ordering::Acquire) {
            return false;
        }
        copy_metadata(&self.src, self.temp.path(), Some(self.entry.mtime));
        match self.temp.persist(&self.entry.path) {
            Ok(_) => true,
            Err(error) => {
                tracing::warn!(%error,path=%self.entry.path.display(),"VolumeCache: failed to publish hydrate");
                false
            }
        }
    }
}

pub(super) fn hydrate_ranged(
    entries: &[(PathBuf, FileMeta)],
    workers: usize,
    chunk: usize,
) -> io::Result<usize> {
    hydrate_with(entries, workers, chunk, |file, offset, length| {
        with_retry(
            || copy_range(&file.src, file.temp.path(), offset, length, file.identity),
            thread::sleep,
        )
    })
}
fn hydrate_with(
    entries: &[(PathBuf, FileMeta)],
    workers: usize,
    chunk: usize,
    copy: impl Fn(&RangedFile, u64, u64) -> io::Result<()> + Sync,
) -> io::Result<usize> {
    // Each prepared temp is RAII-owned before any later preparation can fail or unwind.
    let files: Vec<_> = entries
        .iter()
        .filter_map(|(src, entry)| match RangedFile::prepare(src, entry) {
            Ok(file) => Some(file),
            Err(error) => {
                tracing::warn!(
                    %error,
                    source = %src.display(),
                    "VolumeCache: failed to prepare hydrate"
                );
                None
            }
        })
        .collect();
    let sizes: Vec<_> = files.iter().map(|f| (&f.src, f.entry.size)).collect();
    run_bounded(
        Ranges::new(&sizes, u64::try_from(chunk).map_err(io::Error::other)?),
        workers,
        |(offset, index, length)| {
            let file = &files[index];
            if file.failed.load(Ordering::Acquire) {
                return Ok(());
            }
            if let Err(error) = copy(file, offset, length) {
                file.failed.store(true, Ordering::Release);
                tracing::warn!(%error,source=%file.src.display(),offset,"VolumeCache: failed to hydrate range");
            }
            Ok(())
        },
    )?;
    Ok(files
        .into_iter()
        .filter_map(|f| f.finish().then_some(()))
        .count())
}

struct Ranges<'a> {
    sizes: &'a [(&'a PathBuf, u64)],
    active: Vec<usize>,
    offset: u64,
    index: usize,
    chunk: u64,
}
impl<'a> Ranges<'a> {
    fn new(sizes: &'a [(&'a PathBuf, u64)], chunk: u64) -> Self {
        let mut active: Vec<_> = (0..sizes.len()).filter(|&i| sizes[i].1 > 0).collect();
        active.sort_by_key(|&i| sizes[i].0);
        Self {
            sizes,
            active,
            offset: 0,
            index: 0,
            chunk,
        }
    }
}
impl Iterator for Ranges<'_> {
    type Item = (u64, usize, u64);
    fn next(&mut self) -> Option<Self::Item> {
        if self.index == self.active.len() {
            self.offset = self.offset.checked_add(self.chunk)?;
            self.active.retain(|&i| self.offset < self.sizes[i].1);
            self.index = 0;
        }
        let &index = self.active.get(self.index)?;
        self.index += 1;
        Some((
            self.offset,
            index,
            self.chunk.min(self.sizes[index].1 - self.offset),
        ))
    }
}

struct StopOnUnwind<'a> {
    stop: &'a AtomicBool,
    armed: bool,
}
impl Drop for StopOnUnwind<'_> {
    fn drop(&mut self) {
        if self.armed {
            self.stop.store(true, Ordering::Release);
        }
    }
}
pub(super) fn run_bounded<I, F>(items: I, workers: usize, call: F) -> io::Result<()>
where
    I: Iterator + Send,
    I::Item: Send,
    F: Fn(I::Item) -> io::Result<()> + Sync,
{
    let mut items = items.peekable();
    if items.peek().is_none() {
        return Ok(());
    }
    let workers = items
        .size_hint()
        .1
        .map_or(workers, |count| workers.min(count));
    let items = Mutex::new(items);
    let stop = AtomicBool::new(false);
    thread::scope(|scope| {
        let mut handles = Vec::new();
        for _ in 0..workers {
            let items = &items;
            let stop = &stop;
            let call = &call;
            // A failed spawn stops existing workers before the scope joins them.
            let mut spawn_guard = StopOnUnwind { stop, armed: true };
            handles.push(thread::Builder::new().spawn_scoped(scope, move || {
                let mut guard = StopOnUnwind { stop, armed: true };
                while !stop.load(Ordering::Acquire) {
                    let item = {
                        let mut locked = items
                            .lock()
                            .map_err(|_| io::Error::other("cache range iterator poisoned"))?;
                        if stop.load(Ordering::Acquire) {
                            None
                        } else {
                            locked.next()
                        }
                    };
                    let Some(item) = item else {
                        break;
                    };
                    call(item)?;
                }
                guard.armed = false;
                Ok(())
            })?);
            spawn_guard.armed = false;
        }
        let mut error = None;
        let mut panic = None;
        for handle in handles {
            match handle.join() {
                Ok(Ok(())) => {}
                Ok(Err(e)) => {
                    error.get_or_insert(e);
                }
                Err(payload) => {
                    panic.get_or_insert(payload);
                }
            }
        }
        if let Some(payload) = panic {
            std::panic::resume_unwind(payload);
        }
        error.map_or(Ok(()), Err)
    })
}

fn retryable(error: &io::Error) -> bool {
    if matches!(
        error.kind(),
        io::ErrorKind::OutOfMemory | io::ErrorKind::WouldBlock
    ) {
        return true;
    }
    #[cfg(unix)]
    if error.raw_os_error() == Some(libc::EIO) {
        return true;
    }
    false
}
fn with_retry(
    mut operation: impl FnMut() -> io::Result<()>,
    mut sleep: impl FnMut(Duration),
) -> io::Result<()> {
    for attempt in 0..7 {
        match operation() {
            Ok(()) => return Ok(()),
            Err(error) if retryable(&error) => {}
            Err(error) => return Err(error),
        }
        let mut bytes = [0; 8];
        let _ = getrandom::fill(&mut bytes);
        sleep(Duration::from_nanos(
            u64::from_le_bytes(bytes) % (50_000_000_u64 << attempt),
        ));
    }
    operation()
}
fn copy_range(
    src: &Path,
    dst: &Path,
    offset: u64,
    length: u64,
    pinned: Identity,
) -> io::Result<()> {
    let mut source = File::open(src)?;
    if identity(&source.metadata()?)? != pinned {
        return Err(io::Error::other(format!(
            "source replaced during hydrate at offset {offset}"
        )));
    }
    let mut destination = OpenOptions::new().write(true).open(dst)?;
    source.seek(SeekFrom::Start(offset))?;
    destination.seek(SeekFrom::Start(offset))?;
    let mut buffer = vec![0; usize::try_from(length).map_err(io::Error::other)?];
    let mut done = 0;
    while done < buffer.len() {
        let read = match source.read(&mut buffer[done..]) {
            Ok(read) => read,
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(error) => return Err(error),
        };
        if read == 0 {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                format!("short read: {done} of {length} bytes at offset {offset}"),
            ));
        }
        destination.write_all(&buffer[done..done + read])?;
        done += read;
    }
    Ok(())
}

#[cfg(test)]
mod tests;
