use super::*;
use std::sync::{Arc, atomic::AtomicUsize};

struct Fixture {
    root: tempfile::TempDir,
    entries: Vec<(PathBuf, FileMeta)>,
}
impl Fixture {
    fn new(names: &[&str]) -> Self {
        let root = tempfile::tempdir().unwrap();
        fs::create_dir(root.path().join("sources")).unwrap();
        fs::create_dir(root.path().join("local")).unwrap();
        let entries = names
            .iter()
            .map(|name| {
                let src = root.path().join("sources").join(name);
                fs::write(&src, b"abcdefghijklmnopq").unwrap();
                let path = root.path().join("local").join(name);
                (
                    src,
                    FileMeta {
                        path,
                        size: 17,
                        mtime: 1_000_000_000.0,
                    },
                )
            })
            .collect();
        Self { root, entries }
    }
    fn no_temps(&self) {
        assert!(
            fs::read_dir(self.root.path().join("local"))
                .unwrap()
                .all(|e| !e
                    .unwrap()
                    .file_name()
                    .to_string_lossy()
                    .ends_with(".rpvc.tmp"))
        );
    }
}
fn copy(file: &RangedFile, offset: u64, length: u64) -> io::Result<()> {
    copy_range(&file.src, file.temp.path(), offset, length, file.identity)
}

#[test]
fn python_range_order_fixtures() {
    let fixture: crate::Value =
        serde_json::from_str(include_str!("../../../../tests/fixtures/volume_cache.json")).unwrap();
    for case in fixture["ranges"].as_array().unwrap() {
        let paths: Vec<_> = case["files"]
            .as_array()
            .unwrap()
            .iter()
            .map(|f| PathBuf::from(f[0].as_str().unwrap()))
            .collect();
        let sizes: Vec<_> = paths
            .iter()
            .zip(case["files"].as_array().unwrap())
            .map(|(path, f)| (path, f[1].as_u64().unwrap()))
            .collect();
        let ranges: Vec<_> = Ranges::new(&sizes, case["chunk"].as_u64().unwrap()).collect();
        assert_eq!(crate::json!(ranges), case["ranges"]);
    }
}
#[test]
fn test_hydrate_copies_big_file_in_ranges() {
    let f = Fixture::new(&["model"]);
    let ranges = Mutex::new(Vec::new());
    assert_eq!(
        hydrate_with(&f.entries, 2, 4, |file, offset, length| {
            ranges.lock().unwrap().push((offset, length));
            copy(file, offset, length)
        })
        .unwrap(),
        1
    );
    let mut actual = ranges.into_inner().unwrap();
    actual.sort_unstable();
    assert_eq!(actual, vec![(0, 4), (4, 4), (8, 4), (12, 4), (16, 1)]);
    assert_eq!(
        fs::read(&f.entries[0].1.path).unwrap(),
        b"abcdefghijklmnopq"
    );
}
#[test]
fn test_hydrate_interleaves_ranges_across_files() {
    let f = Fixture::new(&["b", "a"]);
    let order = Mutex::new(Vec::new());
    assert_eq!(
        hydrate_with(&f.entries, 1, 4, |file, offset, length| {
            order
                .lock()
                .unwrap()
                .push(file.src.file_name().unwrap().to_owned());
            copy(file, offset, length)
        })
        .unwrap(),
        2
    );
    let order = order.into_inner().unwrap();
    assert_eq!(order[..4], ["a", "b", "a", "b"]);
}
#[test]
fn test_hydrate_retries_transient_range_errors() {
    let f = Fixture::new(&["model"]);
    let attempts = AtomicUsize::new(0);
    let delays = Mutex::new(Vec::new());
    assert_eq!(
        hydrate_with(&f.entries, 1, 4, |file, offset, length| with_retry(
            || {
                if attempts.fetch_add(1, Ordering::SeqCst) < 3 {
                    Err(io::Error::from(io::ErrorKind::OutOfMemory))
                } else {
                    copy(file, offset, length)
                }
            },
            |delay| delays.lock().unwrap().push(delay)
        ))
        .unwrap(),
        1
    );
    assert_eq!(delays.lock().unwrap().len(), 3);
}
#[test]
fn test_hydrate_does_not_retry_non_transient_errors() {
    let f = Fixture::new(&["model"]);
    let attempts = AtomicUsize::new(0);
    assert_eq!(
        hydrate_with(&f.entries, 1, 4, |_, _, _| with_retry(
            || {
                attempts.fetch_add(1, Ordering::SeqCst);
                Err(io::Error::from(io::ErrorKind::PermissionDenied))
            },
            |_| panic!("must not retry")
        ))
        .unwrap(),
        0
    );
    assert_eq!(attempts.load(Ordering::SeqCst), 1);
    f.no_temps();
}
#[test]
fn test_hydrate_gives_up_after_max_range_attempts() {
    let f = Fixture::new(&["model"]);
    let attempts = AtomicUsize::new(0);
    assert_eq!(
        hydrate_with(&f.entries, 1, 4, |_, _, _| with_retry(
            || {
                attempts.fetch_add(1, Ordering::SeqCst);
                Err(io::Error::from(io::ErrorKind::WouldBlock))
            },
            |_| {}
        ))
        .unwrap(),
        0
    );
    assert_eq!(attempts.load(Ordering::SeqCst), 8);
    f.no_temps();
}
#[test]
fn test_hydrate_skips_only_the_failing_file() {
    let f = Fixture::new(&["bad", "good"]);
    assert_eq!(
        hydrate_with(&f.entries, 2, 4, |file, offset, length| {
            if file.src.ends_with("bad") {
                Err(io::Error::other("bad file"))
            } else {
                copy(file, offset, length)
            }
        })
        .unwrap(),
        1
    );
    assert!(!f.entries[0].1.path.exists());
    assert_eq!(
        fs::read(&f.entries[1].1.path).unwrap(),
        b"abcdefghijklmnopq"
    );
    f.no_temps();
}
#[test]
fn test_hydrate_failure_leaves_existing_local_file_untouched() {
    let f = Fixture::new(&["model"]);
    fs::write(&f.entries[0].1.path, b"older bytes").unwrap();
    assert_eq!(
        hydrate_with(&f.entries, 1, 4, |_, _, _| Err(io::Error::other("failed"))).unwrap(),
        0
    );
    assert_eq!(fs::read(&f.entries[0].1.path).unwrap(), b"older bytes");
    f.no_temps();
}
#[test]
fn test_hydrate_treats_short_read_as_failure() {
    let f = Fixture::new(&["model"]);
    let file = RangedFile::prepare(&f.entries[0].0, &f.entries[0].1).unwrap();
    let error = copy(&file, 0, 20).unwrap_err();
    assert_eq!(error.kind(), io::ErrorKind::UnexpectedEof);
    assert!(!retryable(&error));
}
#[test]
fn test_hydrate_rejects_source_replaced_between_ranges() {
    let f = Fixture::new(&["model"]);
    assert_eq!(
        hydrate_with(&f.entries, 1, 4, |file, offset, length| {
            copy(file, offset, length)?;
            if offset == 0 {
                let replacement = file.src.with_extension("new");
                fs::write(&replacement, b"ABCDEFGHIJKLMNOPQ")?;
                File::options().write(true).open(&replacement)?.set_times(
                    FileTimes::new()
                        .set_modified(SystemTime::UNIX_EPOCH + Duration::from_secs(100)),
                )?;
                fs::rename(replacement, &file.src)?;
            }
            Ok(())
        })
        .unwrap(),
        0
    );
    assert!(!f.entries[0].1.path.exists());
    f.no_temps();
}
#[test]
fn test_hydrate_unexpected_error_stops_ranges_and_cleans_temps() {
    let f = Fixture::new(&["a", "b"]);
    let calls = AtomicUsize::new(0);
    let result = std::panic::catch_unwind(|| {
        let _ = hydrate_with(&f.entries, 2, 1, |_, _, _| {
            calls.fetch_add(1, Ordering::SeqCst);
            panic!("unexpected failure")
        });
    });
    assert!(result.is_err());
    assert!(calls.load(Ordering::SeqCst) <= 2);
    f.no_temps();
}
#[test]
fn test_hydrate_bounds_outstanding_range_tasks() {
    let f = Fixture::new(&["a", "b"]);
    let ids = Mutex::new(std::collections::HashSet::new());
    assert_eq!(
        hydrate_with(&f.entries, 2, 1, |file, offset, length| {
            ids.lock().unwrap().insert(thread::current().id());
            copy(file, offset, length)
        })
        .unwrap(),
        2
    );
    assert!(ids.lock().unwrap().len() <= 2);
}
#[test]
fn test_hydrate_skips_file_when_local_temp_cannot_be_created() {
    let mut f = Fixture::new(&["model"]);
    let parent = f.root.path().join("conflict");
    fs::write(&parent, b"not a directory").unwrap();
    f.entries[0].1.path = parent.join("file");
    assert_eq!(hydrate_ranged(&f.entries, 1, 4).unwrap(), 0);
    f.no_temps();
}
#[test]
fn test_hydrate_cleans_temp_when_rename_fails() {
    let f = Fixture::new(&["model"]);
    fs::create_dir(&f.entries[0].1.path).unwrap();
    assert_eq!(hydrate_ranged(&f.entries, 1, 4).unwrap(), 0);
    f.no_temps();
}
#[test]
fn test_range_retry_jitters_backoff() {
    let mut delays = Vec::new();
    for _ in 0..20 {
        let mut first = true;
        with_retry(
            || {
                if std::mem::take(&mut first) {
                    Err(io::Error::from(io::ErrorKind::WouldBlock))
                } else {
                    Ok(())
                }
            },
            |d| delays.push(d),
        )
        .unwrap();
    }
    assert!(delays.iter().all(|d| *d < Duration::from_millis(50)));
    assert!(delays.iter().any(|d| *d != delays[0]));
}
#[test]
fn run_bounded_aborts_lazily_on_error() {
    let pulled = AtomicUsize::new(0);
    let items = (0..u64::MAX).inspect(|_| {
        pulled.fetch_add(1, Ordering::SeqCst);
    });
    assert!(run_bounded(items, 2, |_| Err(io::Error::other("stop"))).is_err());
    assert!(pulled.load(Ordering::SeqCst) <= 2);
}
#[test]
fn preparation_and_finalization_unwind_drop_all_temps() {
    let f = Fixture::new(&["a", "b"]);
    for phase in [0, 1] {
        let result = std::panic::catch_unwind(|| {
            let mut files = Vec::new();
            for (src, entry) in &f.entries {
                files.push(RangedFile::prepare(src, entry).unwrap());
                assert_ne!(phase, 0, "preparation interrupted");
            }
            copy(&files[0], 0, 17).unwrap();
            panic!("finalization interrupted");
        });
        assert!(result.is_err());
        f.no_temps();
    }
}
#[test]
fn test_copy_file_returns_false_and_cleans_tmp_on_failure() {
    let f = Fixture::new(&["model"]);
    assert!(!copy_file(
        &f.root.path().join("missing"),
        &f.entries[0].1.path
    ));
    assert!(!f.entries[0].1.path.exists());
    f.no_temps();
}
#[test]
fn test_copy_file_preserves_mtime_and_mode() {
    let f = Fixture::new(&["model"]);
    let (src, entry) = &f.entries[0];
    File::options()
        .write(true)
        .open(src)
        .unwrap()
        .set_times(FileTimes::new().set_modified(timestamp(1_000_000_000.0).unwrap()))
        .unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(src, fs::Permissions::from_mode(0o700)).unwrap();
    }
    assert!(copy_file(src, &entry.path));
    assert_eq!(
        fs::metadata(&entry.path).unwrap().modified().unwrap(),
        timestamp(1_000_000_000.0).unwrap()
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            fs::metadata(&entry.path).unwrap().permissions().mode() & 0o777,
            0o700
        );
    }
}
#[test]
fn metadata_failures_are_best_effort() {
    let f = Fixture::new(&["model"]);
    copy_metadata(&f.root.path().join("missing"), &f.entries[0].1.path, None);
    copy_metadata(&f.entries[0].0, &f.entries[0].1.path, Some(f64::NAN));
}
#[test]
fn bounded_workers_share_items_without_duplication() {
    let seen = Arc::new(Mutex::new(Vec::new()));
    run_bounded(0..100, 4, |n| {
        seen.lock().unwrap().push(n);
        Ok(())
    })
    .unwrap();
    let mut values = seen.lock().unwrap();
    values.sort_unstable();
    assert_eq!(*values, (0..100).collect::<Vec<_>>());
}
