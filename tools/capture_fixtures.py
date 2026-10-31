"""Capture buffer, clipping, and mode behavior from the pinned Python SDK."""

import json
import os
from pathlib import Path
from unittest.mock import patch

from runpod.serverless.modules import rp_capture

cases = []
for limit, writes in [
    (10, ["0123456789ABCDEF"]),
    (10, ["01234", "56789", "ABC", "", "DEF"]),
    (4, ["GPU ", "\U0001f680\u03bb\u4e2d\u00e9", "!"]),
    (3, ["\r\n", "ab\n"]),
    (1, ["\U0001f680", "\u4e2d"]),
    (0, ["first", " second"]),
    (16 * 1024, ["head", "x" * (16 * 1024), "tail"]),
]:
    buffer = rp_capture._RingBuffer(limit)
    results = []
    for text in writes:
        results.append({"written": buffer.write(text), "value": buffer.getvalue()})
    cases.append(dict(operation="buffer", limit=limit, writes=writes, output=results))

for text, limit in [
    ("", 0), ("short", 100), ("exact", 5), ("123456", 5),
    ("A" * 100 + "B" * 100, 40), ("abc", 0), ("abc", 1), ("abc", 2),
    ("\U0001f680\u03bb\u4e2d\u00e9ab\r\n", 4), ("0123456789", 7),
    ("x" * (16 * 1024 + 1), 16 * 1024),
]:
    cases.append(dict(operation="clip", input=text, limit=limit,
                      output=rp_capture.clip(text, limit)))

for mode in [None, "", "auto", "all", "off", " ALL \n", " Off ", "yes-please"]:
    environment = {} if mode is None else {"RUNPOD_LOG_CAPTURE": mode}
    with patch.dict(os.environ, environment, clear=True):
        cases.append(dict(operation="mode", input=mode, output=rp_capture.capture_mode(),
                          handler_enabled=rp_capture.handler_capture_enabled()))

destination = Path(__file__).resolve().parents[1] / "tests/fixtures/capture.json"
destination.write_text(json.dumps(cases, indent=2) + "\n", encoding="utf-8")
print(f"Captured {len(cases)} capture cases")
