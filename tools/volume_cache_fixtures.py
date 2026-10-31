"""Capture pinned VolumeCache comparisons, range order, and local hard-link behavior."""
import contextlib
import io
import json
import pathlib
import os
import tarfile
import tempfile
from types import SimpleNamespace
from unittest.mock import patch

from runpod.serverless.utils.rp_volume_cache import VolumeCache, _iter_ranges

ROOT = pathlib.Path(__file__).resolve().parents[1]


def main():
    values = [None, [3, 100.0], [4, 99.0], [4, 100.0], [4, 102.0], [4, 102.001], [4, 103.0]]
    cases = []
    for source in values:
        for destination in values:
            for previous in [None, [3, 100.0], [4, 100.0]]:
                def stat(path):
                    value = source if path == "src" else destination
                    if value is None:
                        raise FileNotFoundError(path)
                    return SimpleNamespace(st_size=value[0], st_mtime=value[1])
                prior = None if previous is None else {"size": previous[0], "mtime": previous[1]}
                with patch("os.stat", stat):
                    cases.append({"source": source, "destination": destination, "prior": previous,
                                  "copy": VolumeCache._needs_copy("src", "dst"),
                                  "sync": VolumeCache._sync_needs_copy("src", "dst", prior)})
    ranges = []
    for files, chunk in [([], 4), ([("z", 0)], 4), ([("z", 8), ("a", 5), ("b", 0)], 4),
                         ([("a", 1), ("b", 9), ("c", 4)], 4), ([("z", 1), ("a", 2)], 1),
                         ([("huge", 2**64-1)], 2**64-1)]:
        records = [SimpleNamespace(src=name, size=size) for name, size in files]
        ranges.append({"files": files, "chunk": chunk, "ranges": list(_iter_ranges(records, chunk))})
    with tempfile.TemporaryDirectory(prefix="runpod-cache-characterization-") as temporary:
        root = pathlib.Path(temporary).resolve()
        local, volume = root / "local", root / "volume"
        local.mkdir()
        volume.mkdir()
        a, b = local / "a", local / "b"
        a.write_bytes(b"data")
        os.link(a, b)
        cache = VolumeCache([local], namespace="fixture", volume_path=volume)
        with patch.object(cache, "_tar_binary", return_value=None):
            assert cache._pack_small([str(a), str(b)])
        with tarfile.open(cache._small_archive_path) as archive:
            hardlinks = {"regular": [member.isfile() for member in archive.getmembers()]}
        a.unlink()
        b.unlink()
        hardlinks["restored"] = cache._extract_small()
        hardlinks["exists"] = [a.exists(), b.exists()]
    data = {"file": "runpod/serverless/utils/rp_volume_cache.py", "comparisons": cases, "ranges": ranges, "hardlinks": hardlinks}
    (ROOT / "tests/fixtures/volume_cache.json").write_text(json.dumps(data, indent=2) + "\n")
    return len(cases) + len(ranges) + 1


if __name__ == "__main__":
    with contextlib.redirect_stdout(io.StringIO()), contextlib.redirect_stderr(io.StringIO()):
        count = main()
    print(f"Captured {count} VolumeCache differential cases")
