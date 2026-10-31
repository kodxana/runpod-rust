use super::{Cache, FileMeta, meta_satisfied, relative_root, timestamp, transfer};
use std::{
    collections::{HashMap, hash_map::DefaultHasher},
    fs::{self, File, FileTimes},
    hash::{Hash, Hasher},
    io::{self, Seek, SeekFrom},
    path::PathBuf,
};

impl Cache {
    pub(super) fn pack_small(&self, files: &[FileMeta]) -> bool {
        let result = (|| -> crate::Result<()> {
            fs::create_dir_all(self.mirror_root())?;
            let mut temp = transfer::temporary(&self.archive_path())?;
            {
                let mut archive = tar::Builder::new(temp.as_file_mut());
                archive.follow_symlinks(false);
                archive.sparse(false);
                let mut identities: HashMap<u64, Vec<PathBuf>> = HashMap::new();
                for file in files {
                    // Native tar replaces platform-dependent subprocess/list-file encoding.
                    let mtime = file.mtime.to_string();
                    archive.append_pax_extensions([("mtime", mtime.as_bytes())])?;
                    if !fs::symlink_metadata(&file.path)?.is_file() {
                        archive.append_path_with_name(&file.path, relative_root(&file.path)?)?;
                        continue;
                    }
                    // Match Python's hard-link entries without holding a descriptor per
                    // archived file. Confirm identity within each hash bucket before linking.
                    let mut hash = DefaultHasher::new();
                    same_file::Handle::from_path(&file.path)?.hash(&mut hash);
                    let candidates = identities.entry(hash.finish()).or_default();
                    let previous = candidates
                        .iter()
                        .find(|path| same_file::is_same_file(path, &file.path).unwrap_or(false));
                    if let Some(previous) = previous {
                        let mut header = tar::Header::new_gnu();
                        header.set_metadata(&fs::metadata(&file.path)?);
                        header.set_entry_type(tar::EntryType::Link);
                        header.set_size(0);
                        archive.append_link(
                            &mut header,
                            relative_root(&file.path)?,
                            relative_root(previous)?,
                        )?;
                    } else {
                        archive.append_path_with_name(&file.path, relative_root(&file.path)?)?;
                        candidates.push(file.path.clone());
                    }
                }
                archive.finish()?;
            }
            temp.persist(self.archive_path()).map_err(|e| e.error)?;
            Ok(())
        })();
        if let Err(error) = result {
            tracing::debug!(%error,"VolumeCache: pack small bucket failed");
            false
        } else {
            true
        }
    }
    pub(super) fn extract_small(&self) -> usize {
        // Python scans all headers before extracting; a late malformed header must not
        // cause earlier entries to be published. Keep this two-pass behavior.
        let Ok(mut file) = File::open(self.archive_path()) else {
            return 0;
        };
        let entries = match members(&mut file) {
            Ok(entries) => entries,
            Err(error) => {
                tracing::debug!(%error,"VolumeCache: archive absent or corrupt");
                return 0;
            }
        };
        let Ok(_) = file.seek(SeekFrom::Start(0)) else {
            return 0;
        };
        let mut archive = tar::Archive::new(file);
        let Ok(iter) = archive.entries() else {
            return 0;
        };
        let mut restored = 0;
        for (entry, meta) in iter.zip(entries) {
            let (Ok(mut entry), Some(meta)) = (entry, meta) else {
                continue;
            };
            if !self.safe_dest(&meta.path) || meta_satisfied(&meta) {
                continue;
            }
            let result = (|| -> io::Result<()> {
                fs::create_dir_all(
                    meta.path
                        .parent()
                        .ok_or_else(|| io::Error::other("archive member has no parent"))?,
                )?;
                let mut output = File::create(&meta.path)?;
                io::copy(&mut entry, &mut output)?;
                let time = timestamp(meta.mtime)?;
                output.set_times(FileTimes::new().set_accessed(time).set_modified(time))?;
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    let mut mode = entry.header().mode()? & 0o755;
                    if mode & 0o100 == 0 {
                        mode &= !0o111;
                    }
                    fs::set_permissions(&meta.path, fs::Permissions::from_mode(mode | 0o600))?;
                }
                Ok(())
            })();
            match result {
                Ok(()) => restored += 1,
                Err(error) => {
                    tracing::debug!(%error,path=%meta.path.display(),"VolumeCache: skip extract");
                }
            }
        }
        restored
    }
}
fn members(file: &mut File) -> io::Result<Vec<Option<FileMeta>>> {
    let cwd = std::env::current_dir()?;
    let root = cwd
        .ancestors()
        .last()
        .ok_or_else(|| io::Error::other("no filesystem root"))?;
    let mut archive = tar::Archive::new(file);
    let mut result = Vec::new();
    for entry in archive.entries()? {
        let mut entry = entry?;
        let kind = entry.header().entry_type();
        if !(kind.is_file() || kind.is_gnu_sparse() || kind.is_contiguous()) {
            result.push(None);
            continue;
        }
        let path: PathBuf = root.join(entry.path()?);
        let size = entry.size();
        let mut mtime = std::time::Duration::from_secs(entry.header().mtime()?).as_secs_f64();
        if let Some(extensions) = entry.pax_extensions()? {
            for extension in extensions {
                let extension = extension?;
                if extension.key_bytes() == b"mtime" {
                    mtime = std::str::from_utf8(extension.value_bytes())
                        .map_err(io::Error::other)?
                        .parse()
                        .map_err(io::Error::other)?;
                }
            }
        }
        result.push(Some(FileMeta { path, size, mtime }));
    }
    Ok(result)
}
