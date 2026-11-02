"""Run pinned log tests and capture finite CLI streams without network access."""
import io
import json
import tempfile
from contextlib import redirect_stdout
from pathlib import Path
from unittest.mock import patch

import pytest
from runpod.rp_cli import logs

ROOT = Path(__file__).resolve().parents[1]
SELECTED = {"test_logs_routes_options_to_shared_streamer", "test_logs_forward_options",
            "test_logs_reject_tail_above_api_limit"}


class Select:
    def pytest_collection_modifyitems(self, items):
        items[:] = [item for item in items if item.path.name == "test_cli_logs.py"
                    or item.originalname in SELECTED]


scratch_root = (ROOT / "test-results").resolve()
scratch_root.mkdir(exist_ok=True)
with tempfile.TemporaryDirectory(prefix="log-source-", dir=scratch_root) as scratch:
    assert Path(scratch).resolve().is_relative_to(scratch_root)
    files = [str(ROOT.parent / "runpod-python/tests/test_cli" / name) for name in
             ["test_cli_logs.py", "test_endpoint_command.py", "test_pod_command.py"]]
    status = pytest.main([*files, "--basetemp", str(Path(scratch) / "pytest"), "--no-cov", "-q"], plugins=[Select()])
if status:
    raise SystemExit(status)


def event(line, ts="2026-01-01T00:00:00Z", **extra):
    return dict(source="container", line=line, ts=ts, **extra)


cases = []


def capture(name, target, events, workers=(), **options):
    endpoint = target == "endpoint"
    resource = "ep/space ?" if endpoint else "pod/space ?"
    calls = []

    async def stream(worker, **kwargs):
        calls.append(dict(worker=worker, **{k: v for k, v in kwargs.items() if k != "on_event_id"}))
        for value in events.get(worker, []):
            yield value

    out = io.StringIO()
    with patch.object(logs, "stream_pod_logs", stream), patch.object(logs, "run_rest_request",
            return_value={"workers": [{"id": worker} for worker in workers]}), redirect_stdout(out):
        logs.show_logs(**({"endpoint_id": resource} if endpoint else {"pod_id": resource}), **options)
    args = [target, "logs", resource]
    for key, value in options.items():
        if key == "as_json":
            if value: args.append("--json")
        elif key == "worker_ids":
            for worker in value: args.extend(["--worker", worker])
        else:
            args.extend(["--" + key, str(value)])
    cases.append(dict(name=name, args=args, events=events, workers=list(workers),
                      calls=calls, stdout=out.getvalue(), as_json=options.get("as_json", False)))


for as_json in [False, True]:
    capture("pod_snapshot", "pod", {"pod/space ?": [event("hello 🦀")]},
            tail=7, since="2025-01-01T00:00:00Z", as_json=as_json)
    capture("worker_sorted_filtered", "endpoint", {"a": [event("a", "2026-01-01T00:00:02Z")], "longer": [event("b")]},
            workers=["a", "longer", "ignored"], worker_ids=["a", "longer"], source="system", as_json=as_json)
    capture("empty_workers", "endpoint", {}, as_json=as_json)
    capture("cutoff", "pod", {"pod/space ?": [event("backfill"), event("live", "2999-01-01T00:00:00Z"), event("not printed")]}, as_json=as_json)
    capture("raw_sort_and_worker_override", "endpoint", {"a": [event("offset", "2026-01-01T01:00:00+02:00", worker="override"), event("lexically first")]},
            workers=["a", "a"], as_json=as_json)
capture("stable_equal_timestamps", "pod", {"pod/space ?": [event("one"), event("two"), event("three")]})
capture("unicode_width", "endpoint", {"界": [event("one")], "abc": [event("two", "2026-01-01T00:00:01Z")]}, workers=["界", "abc"])
capture("tail_zero", "pod", {"pod/space ?": []}, tail=0)
capture("tail_limit", "pod", {"pod/space ?": []}, tail=5000)
(ROOT / "tests/fixtures/cli_logs.json").write_text(json.dumps(cases, indent=2, ensure_ascii=True) + "\n")
print(f"Captured {len(cases)} log cases")
