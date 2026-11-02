"""Characterize artifact location and attachment in the pinned worker runtime."""
import base64
import bz2
import contextlib
import gzip
import io
import json
import lzma
from pathlib import Path
import subprocess
import sys
import tarfile
import tempfile
from unittest.mock import patch

ROOT = Path(__file__).resolve().parents[1]
RUNTIME = Path(sys.argv[1]).resolve()
sys.path.insert(0, str(RUNTIME / "src"))
from runpod_sdk_runtime import bootstrap


def archive(entries, compression="gz", format_type=tarfile.PAX_FORMAT):
    buffer = io.BytesIO()
    with tarfile.open(fileobj=buffer, mode="w", format=format_type) as tar:
        for name, text, *mode in entries:
            header = tarfile.TarInfo(name)
            header.mode = mode[0] if mode else (0o755 if name.endswith(".sh") else 0o644)
            if text is None:
                header.type = tarfile.DIRTYPE
                tar.addfile(header)
            else:
                data = text.encode()
                header.size = len(data)
                tar.addfile(header, io.BytesIO(data))
    raw = buffer.getvalue()
    return {"raw": lambda: raw, "gz": lambda: gzip.compress(raw, mtime=0),
            "bz2": lambda: bz2.compress(raw), "xz": lambda: lzma.compress(raw)}[compression]()


def locate_case(name, payload=None, files=None, directories=None, prebuilt=None, retry=False):
    files = files or {}
    directories = directories or []
    with tempfile.TemporaryDirectory() as directory:
        root = Path(directory)
        for item in directories:
            (root / item).mkdir(parents=True, exist_ok=True)
        for path, text in files.items():
            (root / path).parent.mkdir(parents=True, exist_ok=True)
            (root / path).write_text(text, encoding="utf-8")
        artifact = root / "artifact.tar.gz"
        if payload is not None:
            artifact.write_bytes(payload)
        settings = dict(ARTIFACT_PATH=str(artifact), APP_DIR=str(root / "app"),
                        PREBUILT_APP_DIR=str(root / prebuilt) if prebuilt else "", ARTIFACT_WAIT_SECONDS=0)
        result = None
        error = None
        exception = None
        with patch.multiple(bootstrap, **settings):
            try:
                result = Path(bootstrap._locate()).relative_to(root).as_posix()
                if retry:
                    artifact.unlink()
                    result = Path(bootstrap._locate()).relative_to(root).as_posix()
            except (bootstrap.PhaseError, OSError, EOFError) as exc:
                error = getattr(exc, "phase", None)
                exception = type(exc).__name__
        output = {}
        for base in [root / "app", root / "prebuilt"]:
            if base.is_dir():
                for path in sorted(base.rglob("*")):
                    if path.is_file() and path.name != bootstrap.UNPACK_MARKER:
                        output[path.relative_to(root).as_posix()] = path.read_text(encoding="utf-8")
        assert not (root / "outside.txt").exists()
        return dict(name=name, archive=None if payload is None else base64.b64encode(payload).decode(),
                    files=files, directories=directories, prebuilt=prebuilt, retry=retry,
                    result=result, phase=error, exception=exception, output=output,
                    marker=(root / "app" / bootstrap.UNPACK_MARKER).is_file())


def main():
    entries = [("runpod_manifest.json", '{"app":"native","resources":[]}'),
               ("nested", None), ("nested/main.rs", "fn main() {}"),
               ("worker.sh", "#!/bin/sh\nexit 0\n")]
    cases = [locate_case(f"extract {kind}", archive(entries, kind)) for kind in ["raw", "gz", "bz2", "xz"]]
    cases += [
        locate_case("PAX long Unicode name", archive([("nested/" + "é" * 90 + ".rs", "long")])),
        locate_case("GNU long name", archive([("x" * 120 + ".rs", "long")], format_type=tarfile.GNU_FORMAT)),
        locate_case("missing artifact"),
        locate_case("empty archive", archive([])),
        locate_case("corrupt artifact", b"not an archive"),
        locate_case("truncated gzip", archive(entries)[:25]),
        locate_case("warm marker", files={"app/.rp-unpacked":"old", "app/main.rs":"old"}),
        locate_case("unpack once", archive(entries), retry=True),
        locate_case("prebuilt wins over marker", files={"app/.rp-unpacked":"old", "prebuilt/a":"ready"}, prebuilt="prebuilt"),
        locate_case("missing prebuilt falls back", archive(entries), prebuilt="prebuilt"),
        locate_case("prebuilt file falls back", archive(entries), files={"prebuilt":"file"}, prebuilt="prebuilt"),
        locate_case("marker directory is not marker", archive(entries), directories=["app/.rp-unpacked"]),
        locate_case("traversal", archive([("../outside.txt", "escape")])),
        locate_case("partial extraction before failure", archive([("first.txt", "kept"), ("../outside.txt", "escape")])),
        locate_case("duplicate file later wins", archive([("same", "old"), ("same", "new")])),
        locate_case("file may not replace extraction root", archive([(".", "invalid")])),
        locate_case("readonly file becomes owner writable", archive([("readme", "first", 0), ("readme", "second", 0)])),
        locate_case("embedded marker before failure", archive([(".rp-unpacked", "embedded"), ("../outside.txt", "escape")])),
    ]
    manifests = []
    for text in [None, '{"resources":[{"name":"q1"}]}', "null", "[]", "42", "bad", ""]:
        with tempfile.TemporaryDirectory() as directory:
            if text is not None:
                (Path(directory) / bootstrap.MANIFEST_NAME).write_text(text)
            try:
                result, phase = bootstrap._manifest(directory), None
            except bootstrap.PhaseError as exc:
                result, phase = None, exc.phase
            manifests.append(dict(text=text, result=result, phase=phase))
    payloads = []
    for phase, detail in [("locate", "missing"), ("attach", "bad manifest"), ("system", "line1\nline2"), ("custom", 'quote " unicode: é')]:
        error = bootstrap.PhaseError(phase, detail)
        payloads.append(dict(phase=phase, detail=detail, display=str(error), payload=bootstrap._error_payload(error)))
    data = dict(repository="https://github.com/runpod-workers/sdk-runtime",
                commit=subprocess.check_output(["git", "-C", str(RUNTIME), "rev-parse", "HEAD"], text=True).strip(),
                locate=cases, manifests=manifests, errors=payloads)
    (ROOT / "tests/fixtures/bootstrap.json").write_text(json.dumps(data, indent=2) + "\n", encoding="utf-8")
    return len(cases) + len(manifests) + len(payloads)


if __name__ == "__main__":
    with contextlib.redirect_stdout(io.StringIO()), contextlib.redirect_stderr(io.StringIO()):
        count = main()
    print(f"Captured {count} bootstrap cases")
