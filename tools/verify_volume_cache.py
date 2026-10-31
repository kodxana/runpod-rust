"""Verify native cache archives/manifests in Python and hydrate Python PAX archives in Rust."""
import json
import os
from pathlib import Path
import shutil
import subprocess
import tarfile
import tempfile

ROOT = Path(__file__).resolve().parents[1]
CARGO = shutil.which("cargo") or str(Path.home() / ".cargo/bin" / ("cargo.exe" if os.name == "nt" else "cargo"))


def run(mode, local, volume):
    result = subprocess.run([CARGO, "run", "--quiet", "--locked", "--example", "volume_cache", "--", mode, str(local), str(volume)], cwd=ROOT, check=True, capture_output=True, text=True)
    return int(result.stdout.strip())


with tempfile.TemporaryDirectory(prefix="runpod-cache-interop-") as temporary:
    root = Path(temporary).resolve()
    local, volume = root / "cache", root / "volume"
    local.mkdir()
    volume.mkdir()
    expected = {"nested/café.bin": b"\x00\xffsmall\n", "large.bin": bytes(range(256)) * 1025}
    for name, data in expected.items():
        path = local / name
        path.parent.mkdir(exist_ok=True)
        path.write_bytes(data)
        os.utime(path, (1_000_000_000.125, 1_000_000_000.125))
    assert run("sync", local, volume) == 2
    mirror = volume / ".cache/example"
    assert (mirror / "manifest.json").read_bytes().isascii()
    manifest = json.loads((mirror / "manifest.json").read_text(encoding="utf-8"))
    assert manifest["version"] == 1 and manifest["threshold"] == 256 * 1024
    with tarfile.open(mirror / "small.tar") as archive:
        for record in manifest["small"]:
            path = Path(record["path"])
            name = path.relative_to(path.anchor).as_posix()
            assert name in archive.getnames(), (ascii(name), [ascii(n) for n in archive.getnames()])
            assert archive.extractfile(name).read() == expected[path.relative_to(local).as_posix()]
            assert abs(archive.getmember(name).mtime - record["mtime"]) < 1e-6
    for record in manifest["big"]:
        path = Path(record["path"])
        assert (mirror / "big" / path.relative_to(path.anchor)).read_bytes() == expected[path.relative_to(local).as_posix()]
    # Rebuild the same format with Python's standard tarfile PAX writer and fresh bytes.
    expected = {name: bytes(reversed(data)) for name, data in expected.items()}
    for name, data in expected.items():
        path = local / name
        path.write_bytes(data)
        os.utime(path, (1_000_000_100.25, 1_000_000_100.25))
    with tarfile.open(mirror / "small.tar", "w", format=tarfile.PAX_FORMAT) as archive:
        for record in manifest["small"]:
            path = Path(record["path"])
            record["mtime"] = path.stat().st_mtime
            archive.add(path, arcname=path.relative_to(path.anchor).as_posix(), recursive=False)
    for record in manifest["big"]:
        path = Path(record["path"])
        record["mtime"] = path.stat().st_mtime
        shutil.copyfile(path, mirror / "big" / path.relative_to(path.anchor))
    (mirror / "manifest.json").write_text(json.dumps(manifest), encoding="utf-8")
    for name in expected:
        (local / name).unlink()
    assert run("hydrate", local, volume) == 2
    for name, data in expected.items():
        assert (local / name).read_bytes() == data
        assert abs((local / name).stat().st_mtime - 1_000_000_100.25) < 1e-6
print("Python read native cache output; Rust restored Python PAX archive and ranged-file contents")
