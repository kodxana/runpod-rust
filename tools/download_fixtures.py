"""Capture download metadata and ZIP extraction behavior without network access.

Run on Windows: the Windows abspath cases depend on the native Win32 implementation.
POSIX path behavior is captured separately with posixpath and mocked filesystem writes.
"""
import io
import json
import ntpath
import os
import posixpath
import stat
import uuid
import warnings
import zipfile
from contextlib import ExitStack
from pathlib import Path
from unittest.mock import mock_open, patch

from runpod.serverless.utils import rp_download as download

root = Path(__file__).resolve().parents[1] / "tests/fixtures"
if os.name != "nt":
    raise RuntimeError("Capture these fixtures on Windows to preserve native ntpath.abspath behavior")
cases = []
for size in [-10, 0, 1, 1024, 1024**2, 1024**2 + 1, 1024**3, 1024**3 + 1, 1024**4]:
    cases.append(dict(operation="chunk_size", input=size, output=download.calculate_chunk_size(size)))
for value in [None, "", "abc", "1.2", "-4", "0", "1", "100000000000000000000", " +1_024 ", "1__2", "_1", "1_"]:
    headers = {} if value is None else {"Content-Length": value}
    cases.append(dict(operation="content_length", input=headers, output=str(download.parse_content_length(headers))))
for value in ["", "attachment", 'attachment; filename="a.txt"', 'FILENAME= a.bin ; TYPE=thing',
              'filename="a;b.txt"', 'filename=a.txt; filename=b.zip', "filename*=UTF-8''a.txt",
              'name="a=b"; filename=""', ' x = """value"""; x=final']:
    cases.append(dict(operation="disposition", input=value, output=download.extract_disposition_params(value)))

class Response:
    def __init__(self, headers): self.headers = headers
    def __enter__(self): return self
    def __exit__(self, *_): pass
    def raise_for_status(self): pass
    def iter_content(self, chunk_size): yield b"body"

ident = "11111111-1111-4111-8111-111111111111"
metadata = [
    ("https://example.com/a.jpg?signature=abc#fragment", None),
    ("https://example.com/no_ext", 'inline; filename="named.txt"'),
    ("https://example.com/a.txt", "filename=without_extension"),
    ("https://example.com/a.zip", ""),
    ("https://example.com/a.ZIP", None),
    ("https://example.com/folder/", None),
    ("https://example.com/.hidden", None),
    ("https://example.com/...dots", None),
    ("https://example.com/archive.tar.gz", None),
    ("https://example.com/raw%20name.txt", None),
    ("https://example.com/raw name.txt", None),
    ("https://example.com/żółw.txt", None),
    ("https://example.com/a.jpg", 'filename="a;b.txt"'),
    ("https://example.com/a.jpg", 'filename="dir/back\\file.txt"'),
    ("https://example.com/a.jpg", 'filename="..\\.hidden"'),
    ("https://example.com/a.jpg", 'filename="C:.hidden"'),
    ("https://example.com/a.jpg", 'filename="name."'),
]
for platform, path_module in [("windows", ntpath), ("posix", posixpath)]:
    for url, disposition in metadata:
        headers = {} if disposition is None else {"Content-Disposition": disposition}
        with ExitStack() as stack:
            stack.enter_context(patch.object(download.os, "path", path_module))
            stack.enter_context(patch.object(download.os, "makedirs"))
            stack.enter_context(patch("builtins.open", mock_open()))
            stack.enter_context(patch.object(download, "safe_get", return_value=Response(headers)))
            stack.enter_context(patch.object(download.uuid, "uuid4", return_value=uuid.UUID(ident)))
            stack.enter_context(patch.object(download.zipfile, "ZipFile"))
            result = download.file(url)
            batch = download.download_files_from_urls("test-job", url)
            output = dict(type=result["type"], original_name=result["original_name"],
                          file_name=path_module.basename(result["file_path"]),
                          extracted=result["extracted_path"] is not None,
                          batch_name=path_module.basename(batch[0]))
        cases.append(dict(operation="metadata", platform=platform,
                          input=dict(url=url, disposition=disposition), output=output))

archive_bytes = io.BytesIO()
with warnings.catch_warnings(), zipfile.ZipFile(archive_bytes, "w") as archive:
    warnings.simplefilter("ignore", UserWarning)
    def add(name, content, compression=zipfile.ZIP_STORED):
        info = zipfile.ZipInfo(name, date_time=(2024, 1, 1, 0, 0, 0))
        archive.writestr(info, content, compress_type=compression)
    for compression, name in [(zipfile.ZIP_STORED, "stored.txt"), (zipfile.ZIP_DEFLATED, "deflated.txt"),
                              (zipfile.ZIP_BZIP2, "bzipped.txt"), (zipfile.ZIP_LZMA, "lzma.txt")]:
        add(name, f"{name} content", compression)
    add("empty-dir/", b"")
    for name, content in [("../parent.txt", "parent"), ("/absolute.txt", "absolute"),
                          ("dir/../kept.txt", "kept"), ("dup.txt", "first"), ("./dup.txt", "middle"), ("dup.txt", "last"),
                          ("unicode/żółw.txt", "utf8"),
                          ("dir/./nested.txt", "nested"), ("C:/drive.txt", "drive"),
                          ("//?/UNC/server/share/unc.txt", "extended UNC"),
                          ("//server/share/network.txt", "UNC"), ("//?/C:/device.txt", "device"),
                          ("é:/unicode-drive.txt", "unicode drive"),
                          ("trailing . /bad:name?.txt", "sanitized")]:
        add(name, content)
    raw_name = zipfile.ZipInfo("placeholder")
    raw_name.filename = "back\\slash.txt"
    archive.writestr(raw_name, "backslash")
    link = zipfile.ZipInfo("link")
    link.create_system = 3
    link.external_attr = (stat.S_IFLNK | 0o777) << 16
    archive.writestr(link, "../../outside")
    with archive.open(zipfile.ZipInfo("zip64.txt", date_time=(2024, 1, 1, 0, 0, 0)), "w", force_zip64=True) as member:
        member.write(b"zip64")
(root / "download.zip").write_bytes(archive_bytes.getvalue())

for platform, path_module, sep, altsep in [("windows", ntpath, "\\", "/"), ("posix", posixpath, "/", None)]:
    files = {}
    class Target(io.BytesIO):
        def __init__(self, path): super().__init__(); self.path = path
        def close(self):
            if not self.closed:
                files[self.path] = self.getvalue().decode("utf8")
            super().close()
    with zipfile.ZipFile(io.BytesIO(archive_bytes.getvalue())) as archive, ExitStack() as stack:
        stack.enter_context(patch.object(os, "path", path_module))
        stack.enter_context(patch.object(os, "sep", sep))
        stack.enter_context(patch.object(os, "altsep", altsep))
        stack.enter_context(patch.object(path_module, "exists", return_value=False))
        stack.enter_context(patch.object(path_module, "isdir", return_value=False))
        stack.enter_context(patch.object(os, "makedirs"))
        stack.enter_context(patch.object(os, "mkdir"))
        stack.enter_context(patch("builtins.open", side_effect=lambda path, mode: Target(path_module.relpath(path, "extracted").replace(sep, "/"))))
        archive.extractall("extracted")
    cases.append(dict(operation="zip", platform=platform, output=files))

(root / "download.json").write_text(json.dumps(cases, indent=2, ensure_ascii=False) + "\n", encoding="utf-8")
print(f"Captured {len(cases)} download cases and a mixed-compression ZIP")
