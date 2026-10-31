use super::*;
use std::fs::{File, FileTimes};

struct Fixture {
    root: tempfile::TempDir,
    cache: VolumeCache,
    local: PathBuf,
    volume: PathBuf,
}
impl Fixture {
    fn new() -> Self {
        let root = tempfile::tempdir().unwrap();
        let local = root.path().join("local");
        let volume = root.path().join("volume");
        fs::create_dir(&local).unwrap();
        fs::create_dir(&volume).unwrap();
        let cache = VolumeCache::new(
            [&local],
            VolumeCacheConfig {
                namespace: Some("ep1".into()),
                volume_path: volume.clone(),
                max_workers: NonZeroUsize::new(2),
                ..Default::default()
            },
        )
        .unwrap();
        let local = cache.inner.dirs[0].clone();
        Self {
            root,
            cache,
            local,
            volume,
        }
    }
    fn put(&self, name: &str, bytes: &[u8]) -> PathBuf {
        let path = self.local.join(name);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, bytes).unwrap();
        path
    }
    fn manifest(&self) -> Manifest {
        self.cache.inner.read_manifest().unwrap().unwrap()
    }
}
fn stamp(path: &Path, seconds: f64) {
    File::options()
        .write(true)
        .open(path)
        .unwrap()
        .set_times(FileTimes::new().set_modified(timestamp(seconds).unwrap()))
        .unwrap();
}

#[test]
fn python_metadata_comparison_fixtures() {
    let fixture: Value =
        serde_json::from_str(include_str!("../../../tests/fixtures/volume_cache.json")).unwrap();
    let f = Fixture::new();
    let src = f.local.join("src");
    let dst = f.local.join("dst");
    for case in fixture["comparisons"].as_array().unwrap() {
        let prepare = |name: &str, path: &Path| -> Option<FileMeta> {
            let value = &case[name];
            if value.is_null() {
                let _ = fs::remove_file(path);
                return None;
            }
            fs::write(
                path,
                vec![1; usize::try_from(value[0].as_u64().unwrap()).unwrap()],
            )
            .unwrap();
            stamp(path, value[1].as_f64().unwrap());
            file_meta(path)
        };
        let source = prepare("source", &src);
        let destination = prepare("destination", &dst);
        let prior = case["prior"].as_array().map(|p| FileMeta {
            path: src.clone(),
            size: p[0].as_u64().unwrap(),
            mtime: p[1].as_f64().unwrap(),
        });
        assert_eq!(
            needs_copy(source.as_ref(), destination.as_ref()),
            case["copy"],
            "{case}"
        );
        assert_eq!(
            sync_needs_copy(&src, &dst, prior.as_ref()),
            case["sync"],
            "{case}"
        );
    }
}
#[test]
fn test_unavailable_without_namespace() {
    let mut f = Fixture::new();
    Arc::get_mut(&mut f.cache.inner).unwrap().namespace.clear();
    assert!(!f.cache.available());
    assert_eq!(f.cache.hydrate().unwrap(), 0);
}
#[test]
fn test_namespace_accepts_normal_value() {
    assert_eq!(Fixture::new().cache.inner.namespace, "ep1");
}
#[test]
fn test_scalar_dirs_path_is_treated_as_single_dir() {
    let f = Fixture::new();
    assert_eq!(f.cache.inner.dirs, vec![realpath(&f.local).unwrap()]);
}
#[test]
fn test_default_max_workers_is_positive() {
    assert!(Fixture::new().cache.inner.workers > 0);
}
#[test]
fn test_hydrate_tuning_defaults() {
    let c = VolumeCache::new(Vec::<PathBuf>::new(), VolumeCacheConfig::default()).unwrap();
    assert_eq!(c.inner.hydrate_workers, 64);
    assert_eq!(c.inner.chunk, 8 * 1024 * 1024);
}
#[test]
fn test_hydrate_workers_follow_explicit_max_workers() {
    assert_eq!(Fixture::new().cache.inner.hydrate_workers, 2);
}
#[test]
fn test_explicit_hydrate_workers_override_max_workers() {
    let c = VolumeCache::new(
        Vec::<PathBuf>::new(),
        VolumeCacheConfig {
            max_workers: NonZeroUsize::new(4),
            hydrate_workers: NonZeroUsize::new(16),
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(c.inner.hydrate_workers, 16);
}
#[test]
fn positive_tuning_is_enforced_by_types() {
    assert!(NonZeroUsize::new(0).is_none());
    assert!("-1".parse::<NonZeroUsize>().is_err());
    assert!("2.5".parse::<NonZeroUsize>().is_err());
}
#[test]
fn test_mirror_layout_paths() {
    let f = Fixture::new();
    let c = &f.cache.inner;
    assert_eq!(c.mirror_root(), f.volume.join(".cache/ep1"));
    assert_eq!(c.manifest_path(), c.mirror_root().join("manifest.json"));
    assert_eq!(c.archive_path(), c.mirror_root().join("small.tar"));
    assert_eq!(c.big_root(), c.mirror_root().join("big"));
}
#[test]
fn test_within_confines_source_to_base() {
    let f = Fixture::new();
    assert!(within(&f.local, &f.local.join("missing/file")));
    assert!(within(&f.local, &f.local));
    assert!(!within(&f.local, &f.local.join("../outside")));
    assert!(!within(&f.local, &f.root.path().join("local-other/file")));
}
#[test]
fn test_manifest_round_trip() {
    let f = Fixture::new();
    let p = f.put("file", b"data");
    let meta = file_meta(&p).unwrap();
    f.cache
        .inner
        .write_manifest(&Manifest {
            small: vec![meta.clone()],
            big: vec![meta],
        })
        .unwrap();
    let manifest = f.manifest();
    assert_eq!(manifest.small[0].path, p);
    assert_eq!(manifest.big[0].size, 4);
}
#[test]
fn test_read_manifest_none_when_absent() {
    assert!(
        Fixture::new()
            .cache
            .inner
            .read_manifest()
            .unwrap()
            .is_none()
    );
}
#[test]
fn malformed_manifest_entries_obey_best_effort() {
    let mut f = Fixture::new();
    fs::create_dir_all(f.cache.inner.mirror_root()).unwrap();
    fs::write(
        f.cache.inner.manifest_path(),
        r#"{"version":1,"big":[{"path":false}]}"#,
    )
    .unwrap();
    assert_eq!(f.cache.hydrate().unwrap(), 0);
    Arc::get_mut(&mut f.cache.inner).unwrap().best_effort = false;
    assert!(f.cache.hydrate().is_err());
}
#[test]
fn test_meta_satisfied_by_local() {
    let f = Fixture::new();
    let p = f.put("file", b"data");
    stamp(&p, 100.0);
    let mut meta = file_meta(&p).unwrap();
    assert!(meta_satisfied(&meta));
    meta.mtime = 0.0;
    assert!(meta_satisfied(&meta));
    meta.size = 5;
    assert!(!meta_satisfied(&meta));
    meta.path = f.local.join("missing");
    assert!(!meta_satisfied(&meta));
}
#[test]
fn test_changed_vs_manifest() {
    let one = FileMeta {
        path: "/a".into(),
        size: 1,
        mtime: 100.0,
    };
    let mut metas = vec![one.clone()];
    metas.push(FileMeta {
        path: "/c".into(),
        size: 2,
        mtime: 100.0,
    });
    assert_eq!(changed_vs_manifest(&metas, &[one]), 1);
}
#[test]
fn test_partition_drops_unstatable() {
    let f = Fixture::new();
    assert!(file_meta(&f.local.join("missing")).is_none());
}
#[test]
fn test_iter_files_skips_inflight_temp_files() {
    let f = Fixture::new();
    let p = f.put("model", b"real");
    f.put("model.123.456.rpvc.tmp", b"partial");
    assert_eq!(iter_files(&f.local), vec![p]);
}
#[test]
fn test_sync_writes_manifest_and_archive_for_small() {
    let f = Fixture::new();
    let p = f.put("tiny", b"small");
    f.cache.sync_inline().unwrap();
    assert!(f.cache.inner.archive_path().is_file());
    assert_eq!(f.manifest().small[0].path, p);
}
#[test]
fn test_pack_small_falls_back_to_tarfile() {
    let f = Fixture::new();
    let a = f.put("a", b"one");
    let b = f.put("sub/b", b"two");
    assert!(
        f.cache
            .inner
            .pack_small(&[file_meta(&a).unwrap(), file_meta(&b).unwrap()])
    );
    let mut archive = tar::Archive::new(File::open(f.cache.inner.archive_path()).unwrap());
    let names: Vec<_> = archive
        .entries()
        .unwrap()
        .map(|e| e.unwrap().path().unwrap().into_owned())
        .collect();
    assert_eq!(
        names,
        vec![relative_root(&a).unwrap(), relative_root(&b).unwrap()]
    );
}
#[test]
fn test_pack_small_atomic_no_partial_on_failure() {
    let f = Fixture::new();
    let p = f.put("a", b"old");
    let meta = file_meta(&p).unwrap();
    assert!(f.cache.inner.pack_small(std::slice::from_ref(&meta)));
    let original = fs::read(f.cache.inner.archive_path()).unwrap();
    fs::remove_file(&p).unwrap();
    assert!(!f.cache.inner.pack_small(&[meta]));
    assert_eq!(fs::read(f.cache.inner.archive_path()).unwrap(), original);
    assert!(fs::read_dir(f.cache.inner.mirror_root()).unwrap().all(|e| {
        !e.unwrap()
            .file_name()
            .to_string_lossy()
            .ends_with(".rpvc.tmp")
    }));
}
#[test]
fn test_sync_reclassifies_small_as_big_without_tar() {
    let f = Fixture::new();
    let p = f.put("tiny", b"small");
    fs::create_dir_all(f.cache.inner.archive_path()).unwrap();
    assert_eq!(f.cache.sync_inline().unwrap(), 1);
    let m = f.manifest();
    assert!(m.small.is_empty());
    assert_eq!(m.big[0].path, p);
    fs::remove_file(&p).unwrap();
    assert_eq!(f.cache.hydrate().unwrap(), 1);
    assert_eq!(fs::read(p).unwrap(), b"small");
}
#[test]
fn test_extract_small_restores_files() {
    let f = Fixture::new();
    let p = f.put("a", b"hello");
    assert!(f.cache.inner.pack_small(&[file_meta(&p).unwrap()]));
    fs::remove_file(&p).unwrap();
    assert_eq!(f.cache.inner.extract_small(), 1);
    assert_eq!(fs::read(p).unwrap(), b"hello");
}
#[test]
fn test_extract_small_skips_unsafe_member() {
    let f = Fixture::new();
    let outside = f.root.path().join("outside");
    fs::write(&outside, b"outside").unwrap();
    assert!(f.cache.inner.pack_small(&[file_meta(&outside).unwrap()]));
    fs::remove_file(&outside).unwrap();
    assert_eq!(f.cache.inner.extract_small(), 0);
    assert!(!outside.exists());
}
#[test]
fn test_extract_small_incremental_skips_satisfied() {
    let f = Fixture::new();
    let p = f.put("a", b"hello");
    assert!(f.cache.inner.pack_small(&[file_meta(&p).unwrap()]));
    assert_eq!(f.cache.inner.extract_small(), 0);
}
#[test]
fn test_extract_small_zero_when_archive_missing() {
    assert_eq!(Fixture::new().cache.inner.extract_small(), 0);
}
#[test]
fn test_extract_small_never_raises_when_getmembers_raises() {
    let f = Fixture::new();
    let p = f.put("a", b"hello");
    assert!(f.cache.inner.pack_small(&[file_meta(&p).unwrap()]));
    fs::remove_file(&p).unwrap();
    let file = File::options()
        .write(true)
        .open(f.cache.inner.archive_path())
        .unwrap();
    file.set_len(200).unwrap();
    assert_eq!(f.cache.inner.extract_small(), 0);
    assert!(!p.exists());
}
#[test]
fn test_extract_small_skips_non_file_members() {
    let f = Fixture::new();
    let p = f.put("sub/a", b"hello");
    fs::create_dir_all(f.cache.inner.mirror_root()).unwrap();
    let mut archive = tar::Builder::new(File::create(f.cache.inner.archive_path()).unwrap());
    archive
        .append_dir(
            relative_root(p.parent().unwrap()).unwrap(),
            p.parent().unwrap(),
        )
        .unwrap();
    archive
        .append_path_with_name(&p, relative_root(&p).unwrap())
        .unwrap();
    archive.finish().unwrap();
    drop(archive);
    fs::remove_file(&p).unwrap();
    assert_eq!(f.cache.inner.extract_small(), 1);
    assert!(p.exists());
}
#[test]
fn test_join_pending_syncs_bounded_by_timeout() {
    let pending = Pending::default();
    let (release, wait) = mpsc::channel();
    let (done_tx, done) = mpsc::channel();
    let thread = thread::spawn(move || {
        wait.recv().unwrap();
        let _ = done_tx.send(());
    });
    pending
        .threads
        .lock()
        .unwrap()
        .push(PendingSync { thread, done });
    assert_eq!(pending.flush(Duration::ZERO).unwrap(), 1);
    release.send(()).unwrap();
}
#[test]
fn test_join_pending_syncs_swallows_join_exception() {
    let pending = Pending::default();
    let (done_tx, done) = mpsc::channel();
    let thread = thread::spawn(move || {
        drop(done_tx);
        panic!("worker panic");
    });
    pending
        .threads
        .lock()
        .unwrap()
        .push(PendingSync { thread, done });
    assert_eq!(pending.flush(Duration::from_secs(3)).unwrap(), 0);
}
#[test]
fn successful_syncs_prune_finished_registrations() {
    let f = Fixture::new();
    f.cache.sync().unwrap();
    {
        let pending = f.cache.pending.threads.lock().unwrap();
        pending[0]
            .done
            .recv_timeout(Duration::from_secs(3))
            .unwrap();
        let deadline = std::time::Instant::now() + Duration::from_secs(3);
        while !pending[0].thread.is_finished() {
            assert!(std::time::Instant::now() < deadline);
            thread::yield_now();
        }
    }
    f.cache.sync().unwrap();
    assert_eq!(f.cache.pending.threads.lock().unwrap().len(), 1);
    assert_eq!(f.cache.flush(Duration::from_secs(3)).unwrap(), 0);
}

#[test]
fn unicode_manifest_is_ascii_and_round_trips() {
    let f = Fixture::new();
    let p = f.put("café-🚀", b"data");
    f.cache.sync_inline().unwrap();
    assert!(fs::read(f.cache.inner.manifest_path()).unwrap().is_ascii());
    assert_eq!(f.manifest().small[0].path, p);
    fs::remove_file(&p).unwrap();
    assert_eq!(f.cache.hydrate().unwrap(), 1);
    assert_eq!(fs::read(p).unwrap(), b"data");
}
#[test]
fn scope_exposes_the_original_cache() {
    let f = Fixture::new();
    let scope = f.cache.enter().unwrap();
    assert!(std::ptr::eq(
        std::ptr::from_ref(&*scope),
        std::ptr::from_ref(&f.cache)
    ));
    assert!(scope.available());
}
#[test]
fn sparse_small_files_use_regular_members_and_round_trip() {
    let f = Fixture::new();
    let p = f.put("sparse", b"");
    File::options()
        .write(true)
        .open(&p)
        .unwrap()
        .set_len(64 * 1024)
        .unwrap();
    f.cache.sync_inline().unwrap();
    let mut archive = tar::Archive::new(File::open(f.cache.inner.archive_path()).unwrap());
    assert!(
        archive
            .entries()
            .unwrap()
            .all(|e| e.unwrap().header().entry_type().is_file())
    );
    fs::remove_file(&p).unwrap();
    assert_eq!(f.cache.hydrate().unwrap(), 1);
    assert_eq!(fs::read(p).unwrap(), vec![0; 64 * 1024]);
}
#[test]
fn native_reader_accepts_sparse_archives() {
    let f = Fixture::new();
    let p = f.put("sparse", b"");
    File::options()
        .write(true)
        .open(&p)
        .unwrap()
        .set_len(64 * 1024)
        .unwrap();
    fs::create_dir_all(f.cache.inner.mirror_root()).unwrap();
    let mut archive = tar::Builder::new(File::create(f.cache.inner.archive_path()).unwrap());
    archive
        .append_path_with_name(&p, relative_root(&p).unwrap())
        .unwrap();
    archive.finish().unwrap();
    drop(archive);
    fs::remove_file(&p).unwrap();
    assert_eq!(f.cache.inner.extract_small(), 1);
    assert_eq!(fs::read(p).unwrap(), vec![0; 64 * 1024]);
    assert_eq!(f.cache.inner.extract_small(), 0);
}

#[test]
fn hardlink_members_keep_python_skip_behavior() {
    let fixture: Value =
        serde_json::from_str(include_str!("../../../tests/fixtures/volume_cache.json")).unwrap();
    let f = Fixture::new();
    let a = f.put("a", b"data");
    let b = f.local.join("b");
    fs::hard_link(&a, &b).unwrap();
    assert!(
        f.cache
            .inner
            .pack_small(&[file_meta(&a).unwrap(), file_meta(&b).unwrap()])
    );
    let mut archive = tar::Archive::new(File::open(f.cache.inner.archive_path()).unwrap());
    let kinds: Vec<_> = archive
        .entries()
        .unwrap()
        .map(|e| e.unwrap().header().entry_type())
        .collect();
    assert_eq!(
        crate::json!(
            kinds
                .iter()
                .map(tar::EntryType::is_file)
                .collect::<Vec<_>>()
        ),
        fixture["hardlinks"]["regular"]
    );
    fs::remove_file(&a).unwrap();
    fs::remove_file(&b).unwrap();
    assert_eq!(
        crate::json!(f.cache.inner.extract_small()),
        fixture["hardlinks"]["restored"]
    );
    assert_eq!(fs::read(&a).unwrap(), b"data");
    assert_eq!(
        crate::json!([a.exists(), b.exists()]),
        fixture["hardlinks"]["exists"]
    );
}
#[test]
fn late_corrupt_archive_header_publishes_no_earlier_members() {
    use std::io::{Seek, SeekFrom, Write};
    let f = Fixture::new();
    let a = f.put("a", b"a");
    let b = f.put("b", b"b");
    fs::create_dir_all(f.cache.inner.mirror_root()).unwrap();
    let mut builder = tar::Builder::new(File::create(f.cache.inner.archive_path()).unwrap());
    // Short placeholder names use single regular headers; both destinations are safe.
    for path in [&a, &b] {
        let mut header = tar::Header::new_ustar();
        header.set_size(1);
        header.set_mode(0o600);
        header.set_mtime(100);
        builder
            .append_data(
                &mut header,
                relative_root(path).unwrap(),
                File::open(path).unwrap(),
            )
            .unwrap();
    }
    builder.finish().unwrap();
    drop(builder);
    fs::remove_file(&a).unwrap();
    fs::remove_file(&b).unwrap();
    let mut file = File::options()
        .write(true)
        .open(f.cache.inner.archive_path())
        .unwrap();
    file.seek(SeekFrom::Start(1024)).unwrap();
    file.write_all(b"X").unwrap();
    drop(file);
    assert_eq!(f.cache.inner.extract_small(), 0);
    assert!(!a.exists());
    assert!(!b.exists());
}
#[cfg(windows)]
#[test]
fn drive_relative_namespaces_are_rejected() {
    assert!(
        VolumeCache::new(
            Vec::<PathBuf>::new(),
            VolumeCacheConfig {
                namespace: Some("C:escape".into()),
                ..Default::default()
            }
        )
        .is_err()
    );
}
#[cfg(windows)]
#[test]
fn different_drive_paths_cannot_collide_in_mirror() {
    let path = if std::env::current_dir()
        .unwrap()
        .to_string_lossy()
        .starts_with("Z:")
    {
        Path::new("Y:/cache/model")
    } else {
        Path::new("Z:/cache/model")
    };
    assert!(relative_root(path).is_err());
}

#[cfg(unix)]
#[test]
fn test_sync_skips_symlink_source() {
    use std::os::unix::fs::symlink;
    let f = Fixture::new();
    let real = f.put("real", b"data");
    symlink(&real, f.local.join("link")).unwrap();
    symlink(&f.local, f.local.join("cycle")).unwrap();
    assert_eq!(f.cache.sync_inline().unwrap(), 1);
    assert_eq!(f.manifest().small[0].path, real);
}
#[cfg(unix)]
#[test]
fn fifo_sources_do_not_block_packing_or_copying() {
    let f = Fixture::new();
    let fifo = f.local.join("pipe");
    assert!(
        std::process::Command::new("mkfifo")
            .arg(&fifo)
            .status()
            .unwrap()
            .success()
    );
    assert!(!transfer::copy_file(&fifo, &f.local.join("copy")));
    assert_eq!(f.cache.sync_inline().unwrap(), 1);
    let mut archive = tar::Archive::new(fs::File::open(f.cache.inner.archive_path()).unwrap());
    let member = archive.entries().unwrap().next().unwrap().unwrap();
    assert_eq!(member.path().unwrap(), relative_root(&fifo).unwrap());
    assert!(member.header().entry_type().is_fifo());
    assert_eq!(f.cache.inner.extract_small(), 0);
}
#[cfg(unix)]
#[test]
fn sockets_do_not_discard_regular_small_files() {
    let f = Fixture::new();
    let socket = std::os::unix::net::UnixListener::bind(f.local.join("socket")).unwrap();
    let path = f.put("regular", b"data");
    assert_eq!(f.cache.sync_inline().unwrap(), 2);
    fs::remove_file(&path).unwrap();
    assert_eq!(f.cache.hydrate().unwrap(), 1);
    assert_eq!(fs::read(path).unwrap(), b"data");
    drop(socket);
}
#[cfg(unix)]
#[test]
fn device_members_retain_relative_names_and_types() {
    let f = Fixture::new();
    let device = Path::new("/dev/null");
    assert!(f.cache.inner.pack_small(&[file_meta(device).unwrap()]));
    let mut archive = tar::Archive::new(fs::File::open(f.cache.inner.archive_path()).unwrap());
    let member = archive.entries().unwrap().next().unwrap().unwrap();
    assert_eq!(member.path().unwrap(), Path::new("dev/null"));
    assert!(member.header().entry_type().is_character_special());
    assert_eq!(member.size(), 0);
}
#[cfg(unix)]
#[test]
fn symlinks_cannot_escape_either_hydration_root() {
    use std::os::unix::fs::symlink;
    let f = Fixture::new();
    let outside = f.root.path().join("outside");
    fs::create_dir(&outside).unwrap();
    symlink(&outside, f.local.join("escape")).unwrap();
    assert!(!f.cache.inner.safe_dest(&f.local.join("escape/file")));
    fs::create_dir_all(f.cache.inner.big_root()).unwrap();
    symlink(&outside, f.cache.inner.big_root().join("escape")).unwrap();
    assert!(!within(
        &f.cache.inner.big_root(),
        &f.cache.inner.big_root().join("escape/file")
    ));
}
