use runpod::serverless::{VolumeCache, VolumeCacheConfig};
use std::{
    fs::{self, File, FileTimes},
    num::NonZeroUsize,
    path::{Path, PathBuf},
    time::{Duration, UNIX_EPOCH},
};

struct Fixture {
    root: tempfile::TempDir,
    local: PathBuf,
    volume: PathBuf,
    cache: VolumeCache,
}
impl Fixture {
    fn new() -> Self {
        let root = tempfile::tempdir().unwrap();
        let local = root.path().join("cache");
        let volume = root.path().join("volume");
        fs::create_dir(&local).unwrap();
        fs::create_dir(&volume).unwrap();
        let cache = VolumeCache::new(
            [&local],
            VolumeCacheConfig {
                namespace: Some("ep1".into()),
                volume_path: volume.clone(),
                max_workers: NonZeroUsize::new(2),
                hydrate_chunk_size: NonZeroUsize::new(64 * 1024),
                ..Default::default()
            },
        )
        .unwrap();
        Self {
            root,
            local,
            volume,
            cache,
        }
    }
    fn manifest(&self) -> runpod::Value {
        serde_json::from_slice(&fs::read(self.volume.join(".cache/ep1/manifest.json")).unwrap())
            .unwrap()
    }
    fn put(&self, name: &str, data: &[u8]) -> PathBuf {
        let path = self.local.join(name);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, data).unwrap();
        path
    }
}
fn stamp(path: &Path, seconds: u64) {
    File::options()
        .write(true)
        .open(path)
        .unwrap()
        .set_times(FileTimes::new().set_modified(UNIX_EPOCH + Duration::from_secs(seconds)))
        .unwrap();
}
fn big() -> Vec<u8> {
    (0..(256 * 1024 + 123))
        .map(|i| u8::try_from(i % 251).unwrap())
        .collect()
}
fn mirror(path: &Path, volume: &Path) -> PathBuf {
    let path = fs::canonicalize(path).unwrap_or_else(|_| {
        fs::canonicalize(path.parent().unwrap())
            .unwrap()
            .join(path.file_name().unwrap())
    });
    let relative: PathBuf = path
        .components()
        .filter(|c| matches!(c, std::path::Component::Normal(_)))
        .collect();
    volume.join(".cache/ep1/big").join(relative)
}

#[test]
fn test_unavailable_when_volume_dir_missing() {
    let f = Fixture::new();
    let c = VolumeCache::new(
        [&f.local],
        VolumeCacheConfig {
            namespace: Some("ep1".into()),
            volume_path: f.root.path().join("missing"),
            ..Default::default()
        },
    )
    .unwrap();
    assert!(!c.available());
    assert_eq!(c.hydrate().unwrap(), 0);
    assert_eq!(c.sync_inline().unwrap(), 0);
    c.sync().unwrap();
}
#[test]
fn test_available_when_volume_present_and_namespace_set() {
    assert!(Fixture::new().cache.available());
}
#[test]
fn test_namespace_rejects_unsafe_values() {
    let f = Fixture::new();
    for name in ["../evil", "a/b", "/etc", "..", "a\\b", "."] {
        assert!(
            VolumeCache::new(
                [&f.local],
                VolumeCacheConfig {
                    namespace: Some(name.into()),
                    volume_path: f.volume.clone(),
                    ..Default::default()
                }
            )
            .is_err()
        );
    }
}
#[test]
fn test_sync_then_hydrate_round_trip() {
    let f = Fixture::new();
    let p = f.put("model.bin", b"weights");
    assert_eq!(f.cache.sync_inline().unwrap(), 1);
    fs::remove_file(&p).unwrap();
    assert_eq!(f.cache.hydrate().unwrap(), 1);
    assert_eq!(fs::read(p).unwrap(), b"weights");
}
#[test]
fn test_sync_is_idempotent_on_unchanged_file() {
    let f = Fixture::new();
    f.put("model.bin", b"weights");
    f.cache.sync_inline().unwrap();
    assert_eq!(f.cache.sync_inline().unwrap(), 0);
}
#[test]
fn test_sync_recopies_modified_file() {
    let f = Fixture::new();
    let p = f.put("model.bin", b"v1");
    f.cache.sync_inline().unwrap();
    fs::write(&p, b"v2-longer").unwrap();
    assert_eq!(f.cache.sync_inline().unwrap(), 1);
    fs::remove_file(&p).unwrap();
    f.cache.hydrate().unwrap();
    assert_eq!(fs::read(p).unwrap(), b"v2-longer");
}
#[test]
fn test_hydrate_skips_unchanged_after_first_hydrate() {
    let f = Fixture::new();
    let p = f.put("model", b"weights");
    f.cache.sync_inline().unwrap();
    fs::remove_file(p).unwrap();
    assert_eq!(f.cache.hydrate().unwrap(), 1);
    assert_eq!(f.cache.hydrate().unwrap(), 0);
}
#[test]
fn test_hydrate_does_not_overwrite_newer_container_file() {
    let f = Fixture::new();
    let p = f.put("model", b"weightsA");
    stamp(&p, 1_000_000_000);
    f.cache.sync_inline().unwrap();
    fs::write(&p, b"weightsB").unwrap();
    stamp(&p, 1_000_000_100);
    assert_eq!(f.cache.hydrate().unwrap(), 0);
    assert_eq!(fs::read(p).unwrap(), b"weightsB");
}
#[test]
fn test_sync_skips_excluded_paths() {
    let f = Fixture::new();
    for name in [
        "refs/main",
        ".no_exist/absent",
        "locked.lock",
        "model.1.2.rpvc.tmp",
    ] {
        f.put(name, b"excluded");
    }
    assert_eq!(f.cache.sync_inline().unwrap(), 0);
}
#[test]
fn test_do_sync_skips_missing_dirs() {
    let f = Fixture::new();
    let c = VolumeCache::new(
        [f.local.join("missing")],
        VolumeCacheConfig {
            namespace: Some("missing".into()),
            volume_path: f.volume.clone(),
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(c.sync_inline().unwrap(), 0);
}
#[test]
fn test_hydrate_no_manifest_is_noop() {
    assert_eq!(Fixture::new().cache.hydrate().unwrap(), 0);
}
#[test]
fn test_hydrate_restores_big_and_small() {
    let f = Fixture::new();
    let small = f.put("tiny.bin", b"small");
    let bytes = big();
    let large = f.put("weights.bin", &bytes);
    assert_eq!(f.cache.sync_inline().unwrap(), 2);
    fs::remove_file(small).unwrap();
    fs::remove_file(&large).unwrap();
    assert_eq!(f.cache.hydrate().unwrap(), 2);
    assert_eq!(fs::read(large).unwrap(), bytes);
}
#[test]
fn test_context_manager_hydrates_on_enter_and_syncs_on_exit() {
    let f = Fixture::new();
    f.put("model", b"weights");
    {
        let _scope = f.cache.enter().unwrap();
    }
    assert_eq!(f.cache.flush(Duration::from_secs(3)).unwrap(), 0);
    assert_eq!(f.manifest()["small"].as_array().unwrap().len(), 1);
}
#[test]
fn test_context_manager_does_not_suppress_exceptions() {
    let f = Fixture::new();
    f.put("model", b"weights");
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let _scope = f.cache.enter().unwrap();
        panic!("user failure");
    }));
    assert!(result.is_err());
    f.cache.flush(Duration::from_secs(3)).unwrap();
    assert_eq!(f.manifest()["small"].as_array().unwrap().len(), 1);
}
#[test]
fn test_best_effort_swallows_sync_failure_inline() {
    let f = Fixture::new();
    fs::write(f.volume.join(".cache"), b"conflict").unwrap();
    assert_eq!(f.cache.sync_inline().unwrap(), 0);
}
#[test]
fn test_best_effort_false_reraises_inline() {
    let f = Fixture::new();
    fs::write(f.volume.join(".cache"), b"conflict").unwrap();
    let c = VolumeCache::new(
        [&f.local],
        VolumeCacheConfig {
            namespace: Some("ep1".into()),
            volume_path: f.volume.clone(),
            best_effort: false,
            ..Default::default()
        },
    )
    .unwrap();
    assert!(c.sync_inline().is_err());
}
#[test]
fn test_best_effort_swallows_sync_failure_background() {
    let f = Fixture::new();
    fs::write(f.volume.join(".cache"), b"conflict").unwrap();
    f.cache.sync().unwrap();
    assert_eq!(f.cache.flush(Duration::from_secs(3)).unwrap(), 0);
}
#[test]
fn test_sync_big_file_is_unpacked_and_incremental() {
    let f = Fixture::new();
    let p = f.put("weights", &big());
    assert_eq!(f.cache.sync_inline().unwrap(), 1);
    assert!(mirror(&p, &f.volume).exists());
    assert_eq!(f.cache.sync_inline().unwrap(), 0);
}
#[test]
fn test_sync_recopies_when_mirror_file_is_missing() {
    let f = Fixture::new();
    let p = f.put("weights", &big());
    f.cache.sync_inline().unwrap();
    fs::remove_file(mirror(&p, &f.volume)).unwrap();
    assert_eq!(f.cache.sync_inline().unwrap(), 1);
}
#[test]
fn test_hydrate_does_not_resurrect_deleted_small_file() {
    let f = Fixture::new();
    let a = f.put("a", b"aaa");
    let b = f.put("b", b"bbb");
    f.cache.sync_inline().unwrap();
    fs::remove_file(&b).unwrap();
    assert_eq!(f.cache.sync_inline().unwrap(), 0);
    fs::remove_file(&a).unwrap();
    assert_eq!(f.cache.hydrate().unwrap(), 1);
    assert!(a.exists());
    assert!(!b.exists());
}
#[test]
fn test_sync_omits_big_file_from_manifest_when_copy_fails() {
    let f = Fixture::new();
    let good = f.put("good", &big());
    let bad = f.put("bad", &big());
    let destination = mirror(&bad, &f.volume);
    fs::create_dir_all(&destination).unwrap();
    assert_eq!(f.cache.sync_inline().unwrap(), 1);
    assert_eq!(f.manifest()["big"].as_array().unwrap().len(), 1);
    fs::remove_file(&good).unwrap();
    fs::remove_file(&bad).unwrap();
    assert_eq!(f.cache.hydrate().unwrap(), 1);
    assert!(good.exists());
    assert!(!bad.exists());
}
#[test]
fn test_sync_needs_copy_ignores_source_older_than_manifest() {
    let f = Fixture::new();
    let p = f.put("weights", &big());
    stamp(&p, 1_000_000_100);
    f.cache.sync_inline().unwrap();
    stamp(&p, 1_000_000_000);
    assert_eq!(f.cache.sync_inline().unwrap(), 0);
}
#[test]
fn test_hydrate_restores_source_mtime_so_next_sync_is_noop() {
    let f = Fixture::new();
    let p = f.put("weights", &big());
    stamp(&p, 1_000_000_000);
    f.cache.sync_inline().unwrap();
    stamp(&mirror(&p, &f.volume), 1_000_001_000);
    fs::remove_file(&p).unwrap();
    assert_eq!(f.cache.hydrate().unwrap(), 1);
    assert_eq!(
        fs::metadata(&p).unwrap().modified().unwrap(),
        UNIX_EPOCH + Duration::from_secs(1_000_000_000)
    );
    assert_eq!(f.cache.sync_inline().unwrap(), 0);
}
#[test]
fn test_sync_recopies_same_size_update_when_volume_rejects_utime() {
    let f = Fixture::new();
    let mut bytes = big();
    let p = f.put("weights", &bytes);
    stamp(&p, 1_000_000_000);
    f.cache.sync_inline().unwrap();
    stamp(&mirror(&p, &f.volume), 2_000_000_000);
    bytes[0] = 255;
    fs::write(&p, &bytes).unwrap();
    stamp(&p, 1_000_000_100);
    assert_eq!(f.cache.sync_inline().unwrap(), 1);
    assert_eq!(fs::read(mirror(&p, &f.volume)).unwrap(), bytes);
}
#[test]
fn test_hydrate_rejects_source_whose_size_differs_from_manifest() {
    let f = Fixture::new();
    let p = f.put("weights", &big());
    f.cache.sync_inline().unwrap();
    fs::write(mirror(&p, &f.volume), b"truncated").unwrap();
    fs::remove_file(&p).unwrap();
    assert_eq!(f.cache.hydrate().unwrap(), 0);
    assert!(!p.exists());
}
#[test]
fn test_hydrate_skips_unsafe_destination() {
    let f = Fixture::new();
    f.cache.sync_inline().unwrap();
    let outside = f.root.path().join("outside");
    let src = mirror(&outside, &f.volume);
    fs::create_dir_all(src.parent().unwrap()).unwrap();
    fs::write(src, b"malicious").unwrap();
    let manifest =
        runpod::json!({"version":1,"small":[],"big":[{"path":outside,"size":9,"mtime":1.0}]});
    fs::write(
        f.volume.join(".cache/ep1/manifest.json"),
        serde_json::to_vec(&manifest).unwrap(),
    )
    .unwrap();
    assert_eq!(f.cache.hydrate().unwrap(), 0);
    assert!(!outside.exists());
}
#[test]
fn test_read_manifest_none_when_corrupt() {
    let f = Fixture::new();
    f.cache.sync_inline().unwrap();
    for data in ["{broken", "42", r#"{"version":2,"small":[],"big":[]}"#] {
        fs::write(f.volume.join(".cache/ep1/manifest.json"), data).unwrap();
        assert_eq!(f.cache.hydrate().unwrap(), 0);
        f.put("tiny", b"x");
        assert_eq!(f.cache.sync_inline().unwrap(), 1);
    }
}
#[test]
fn test_partition_splits_at_threshold() {
    let f = Fixture::new();
    f.put("small", &vec![1; 256 * 1024 - 1]);
    f.put("exact", &vec![2; 256 * 1024]);
    f.put("big", &vec![3; 256 * 1024 + 1]);
    assert_eq!(f.cache.sync_inline().unwrap(), 3);
    let m = f.manifest();
    assert_eq!(m["small"].as_array().unwrap().len(), 1);
    assert_eq!(m["big"].as_array().unwrap().len(), 2);
}
#[test]
fn drop_waits_for_background_sync() {
    let f = Fixture::new();
    f.put("small", b"data");
    let c = VolumeCache::new(
        [&f.local],
        VolumeCacheConfig {
            namespace: Some("drop".into()),
            volume_path: f.volume.clone(),
            ..Default::default()
        },
    )
    .unwrap();
    c.sync().unwrap();
    drop(c);
    assert!(f.volume.join(".cache/drop/manifest.json").is_file());
}
