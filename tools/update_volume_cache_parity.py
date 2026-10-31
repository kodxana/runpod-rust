"""Update only reviewed VolumeCache mappings; retain all other inventory entries."""
import json
from pathlib import Path
import re

ROOT = Path(__file__).resolve().parents[1]
SYMBOLS = {
    "VolumeCache": "VolumeCache", "VolumeCache.__init__": "VolumeCache::new / VolumeCacheConfig",
    "VolumeCache._mirror_root": "Cache::mirror_root", "VolumeCache._manifest_path": "Cache::manifest_path",
    "VolumeCache._small_archive_path": "Cache::archive_path", "VolumeCache._big_root": "Cache::big_root",
    "VolumeCache.available": "VolumeCache::available", "VolumeCache._iter_files": "iter_files",
    "VolumeCache._partition": "Cache::sync (size partition)", "VolumeCache._file_meta": "file_meta",
    "VolumeCache._write_manifest": "Cache::write_manifest / AsciiJson", "VolumeCache._read_manifest": "Cache::read_manifest",
    "VolumeCache._meta_satisfied_by_local": "meta_satisfied", "VolumeCache._changed_vs_manifest": "changed_vs_manifest",
    "VolumeCache._needs_copy": "needs_copy", "VolumeCache._sync_needs_copy": "sync_needs_copy",
    "VolumeCache._is_safe_dest": "Cache::safe_dest", "VolumeCache._copy_file": "transfer::copy_file",
    "VolumeCache._copy_parallel": "Cache::sync / transfer::run_bounded",
    "VolumeCache._hydrate_ranged": "transfer::hydrate_ranged", "VolumeCache._prepare_ranged": "transfer::RangedFile::prepare",
    "VolumeCache._copy_ranges": "transfer::hydrate_with", "VolumeCache._finish_ranged": "transfer::RangedFile::finish",
    "VolumeCache._within": "transfer::within", "VolumeCache._guard": "Cache::guard",
    "VolumeCache._tar_binary": "Cache::pack_small (native tar backend replaces binary selection)",
    "VolumeCache._pack_small": "Cache::pack_small", "VolumeCache._extract_small": "Cache::extract_small",
    "VolumeCache._extract_member": "Cache::extract_small (regular-member copy and data-filter permissions)",
    "VolumeCache.hydrate": "VolumeCache::hydrate", "VolumeCache._do_hydrate": "Cache::hydrate",
    "VolumeCache.sync": "VolumeCache::{sync,sync_inline}", "VolumeCache._do_sync": "Cache::sync",
    "VolumeCache.__enter__": "VolumeCache::enter", "VolumeCache.__exit__": "VolumeCacheGuard::drop",
    "_copy_metadata": "transfer::copy_metadata", "_positive_or_default": "VolumeCacheConfig NonZeroUsize fields / VolumeCache::new",
    "_tmp_path": "transfer::temporary", "_iter_ranges": "transfer::Ranges", "_run_bounded": "transfer::run_bounded",
    "_with_range_retry": "transfer::with_retry", "_file_identity": "transfer::identity", "_copy_range": "transfer::copy_range",
    "_pwrite_all": "transfer::copy_range / Write::write_all", "_silent_remove": "NamedTempFile Drop / best-effort archive removal",
    "_register_pending": "VolumeCache::sync / Pending", "_join_pending_syncs": "Pending::flush / Pending::drop",
    "_reset_pending_for_test": "fresh owned Pending registry per cache/test",
}
note = ("Native tar/PAX, scoped threads, independent file handles, positive integer/path types, and RAII replace Python subprocesses, pread/pwrite, globals, context managers, and atexit. "
        "Clones share an owned pending registry; explicit flush/last-owner drop bounds waits. Native panics unwind; process signals/exit do not implement KeyboardInterrupt/atexit. "
        "Manifest paths use platform-native UTF-8 strings escaped to ASCII JSON. See PORTING.md and reviews/volume-cache.md for transport, filesystem, and validation limits.")
path = ROOT / "parity/api.json"
data = json.loads(path.read_text())
for entry in data["entries"]:
    if entry["file"] == "runpod/serverless/utils/rp_volume_cache.py" and entry["name"] in SYMBOLS:
        entry.update(rust="utils::volume_cache::" + SYMBOLS[entry["name"]], status="adapted", note=note)
path.write_text(json.dumps(data, indent=2) + "\n")

targets = {}
for filename, prefix in [("tests/volume_cache.rs", "volume_cache"),
                         ("src/utils/volume_cache/tests.rs", "utils::volume_cache::tests"),
                         ("src/utils/volume_cache/transfer/tests.rs", "utils::volume_cache::transfer::tests")]:
    for name in re.findall(r"\bfn\s+(\w+)\s*\(", (ROOT / filename).read_text(encoding="utf-8")):
        if name.startswith("test_"):
            targets[name] = prefix + "::" + name
aliases = {
    "test_needs_copy_false_when_src_missing": "utils::volume_cache::tests::python_metadata_comparison_fixtures",
    "test_hydrate_noop_when_unavailable": "volume_cache::test_unavailable_when_volume_dir_missing",
    "test_sync_noop_when_unavailable": "volume_cache::test_unavailable_when_volume_dir_missing",
    "test_read_manifest_none_when_valid_json_not_object": "volume_cache::test_read_manifest_none_when_corrupt",
    "test_read_manifest_none_when_unknown_version": "volume_cache::test_read_manifest_none_when_corrupt",
    "test_max_workers_rejects_non_positive": "utils::volume_cache::tests::positive_tuning_is_enforced_by_types",
    "test_hydrate_tuning_rejects_non_positive": "utils::volume_cache::tests::positive_tuning_is_enforced_by_types",
    "test_tuning_rejects_non_integer": "utils::volume_cache::tests::positive_tuning_is_enforced_by_types",
    "test_pack_small_with_binary_round_trips": "utils::volume_cache::tests::test_pack_small_falls_back_to_tarfile / tools/verify_volume_cache.py",
    "test_pack_small_subprocess_tar_failure_leaves_no_partial": "utils::volume_cache::tests::test_pack_small_atomic_no_partial_on_failure (native tar; no subprocess)",
    "test_hydrate_interrupted_during_preparation_leaves_no_temps": "utils::volume_cache::transfer::tests::preparation_and_finalization_unwind_drop_all_temps",
    "test_hydrate_interrupted_during_finalization_leaves_no_temps": "utils::volume_cache::transfer::tests::preparation_and_finalization_unwind_drop_all_temps",
}
adapted = set(aliases) | {
    "test_scalar_dirs_path_is_treated_as_single_dir", "test_context_manager_hydrates_on_enter_and_syncs_on_exit",
    "test_context_manager_does_not_suppress_exceptions", "test_hydrate_unexpected_error_stops_ranges_and_cleans_temps",
    "test_sync_recopies_same_size_update_when_volume_rejects_utime", "test_hydrate_restores_source_mtime_so_next_sync_is_noop",
    "test_sync_reclassifies_small_as_big_without_tar", "test_pack_small_falls_back_to_tarfile",
}
path = ROOT / "parity/tests.json"
data = json.loads(path.read_text())
for entry in data["entries"]:
    if entry["file"] != "tests/test_serverless/test_utils/test_rp_volume_cache.py":
        continue
    name = entry["name"]
    target = aliases.get(name, targets.get(name))
    if target:
        entry.update(rust=target, status="adapted" if name in adapted else "ported")
        if name in adapted:
            entry["note"] = "Native type, tar, filesystem-failure or RAII equivalent; no claim of Python subprocess/KeyboardInterrupt/monkeypatch execution. See reviews/volume-cache.md."
path.write_text(json.dumps(data, indent=2) + "\n")
for kind in ["api", "tests"]:
    entries = json.loads((ROOT / f"parity/{kind}.json").read_text())["entries"]
    counts = {status: sum(e["status"] == status for e in entries) for status in sorted({e["status"] for e in entries})}
    print(kind, counts)
print("Remaining cache tests:", [e["name"] for e in data["entries"] if e["file"].endswith("test_rp_volume_cache.py") and e["status"] == "pending"])
