"""Capture pinned job inspection/control CLI behavior without live API requests."""
import copy
import json
from contextlib import ExitStack
from pathlib import Path
import tempfile
from unittest.mock import MagicMock, patch

import pytest
import requests
from click.testing import CliRunner
from rich.text import Text
from runpod.endpoint.helpers import UNAUTHORIZED_MSG
from runpod.rp_cli import endpoint as ep
from runpod.rp_cli.main import cli

ROOT = Path(__file__).resolve().parents[1]
CASES = []
SELECTED = {"test_job_get", "test_job_cancel_and_retry", "test_purge"}


def capture(name, args, invoke, client):
    case = {"name": name, "args": list(args), "requests": [], "fields": [], "success": []}

    class RecordingClient:
        def request(self, method, path, body=None, **kwargs):
            call = {"method": method.upper(), "path": path, "body": copy.deepcopy(body), "status": 200}
            case["requests"].append(call)
            try:
                value = getattr(client, method)(path, body, **kwargs) if method == "post" else client.get(path, **kwargs)
            except requests.HTTPError as exc:
                assert exc.response is not None
                call.update(status=exc.response.status_code, raw=exc.response.text)
                raise
            except RuntimeError as exc:
                assert str(exc) == UNAUTHORIZED_MSG
                call.update(status=401, raw="ignored unauthorized body")
                raise
            call["response"] = copy.deepcopy(value)
            return value

        def get(self, path, **kwargs):
            return self.request("get", path, **kwargs)

        def post(self, path, body, **kwargs):
            return self.request("post", path, body, **kwargs)

    fields, success = ep.print_fields, ep.ui.success

    def record_fields(record, columns):
        case["fields"].append([[label, value(record)] for label, value in columns])
        return fields(record, columns)

    def record_success(message):
        case["success"].append(Text.from_markup(message).plain)
        return success(message)

    with ExitStack() as stack:
        stack.enter_context(patch.object(ep.runner, "RunPodClient", lambda: RecordingClient()))
        stack.enter_context(patch.object(ep, "print_fields", record_fields))
        stack.enter_context(patch.object(ep.ui, "success", record_success))
        stack.enter_context(patch("runpod.rp_cli.update.start_background_check"))
        result = invoke(*args)
    case.update(code=result.exit_code, stdout=result.stdout, stderr=result.stderr)
    if result.exit_code and not isinstance(result.exception, SystemExit):
        raise AssertionError(result.exception)
    CASES.append(case)
    return result


class Capture:
    def pytest_collection_modifyitems(self, items):
        items[:] = [item for item in items if item.originalname in SELECTED]

    @pytest.hookimpl(hookwrapper=True)
    def pytest_runtest_call(self, item):
        invoke = item.module.invoke
        client = item.funcargs["client"]
        def recorded(*args, **kwargs):
            return capture(item.originalname, args, lambda *values: invoke(*values, **kwargs), client)
        with patch.object(item.module, "invoke", recorded):
            yield


scratch_root = (ROOT / "test-results").resolve()
scratch_root.mkdir(exist_ok=True)
with tempfile.TemporaryDirectory(prefix="job-source-", dir=scratch_root) as scratch:
    assert Path(scratch).resolve().is_relative_to(scratch_root)
    status = pytest.main([str(ROOT.parent / "runpod-python/tests/test_cli/test_endpoint_command.py"),
                          "--basetemp", str(Path(scratch) / "pytest"), "--no-cov", "-q"], plugins=[Capture()])
if status:
    raise SystemExit(status)


def extra(name, args, reply=None, failure=None):
    client = MagicMock()
    for method in [client.get, client.post]:
        method.return_value = reply
        method.side_effect = failure
    capture(name, args, lambda *args: CliRunner().invoke(cli, ["endpoint", *args]), client)


for value in [None, "", "done", False, [1, "two"], {"answer": "café", "ok": True}]:
    extra("job_output_values", ["job", "get", "ep1", "job1"],
          {"id": "job1", "status": "FAILED", "delayTime": 0, "executionTime": -1250,
           "output": value, "error": "kept visible"})
for operation in ["get", "cancel", "retry"]:
    extra("encoded_job_ids", ["job", operation, "ep /?#%é", "job /?#%é", "--json"],
          {"id": "new1", "status": "IN_QUEUE", "unknown": [1, False]})
extra("encoded_job_ids", ["job", "get", "ep1", "", "--json"], {})
extra("job_response_fallbacks", ["job", "retry", "ep1", "job1"], {"id": "new1", "status": "IN_QUEUE"})
extra("job_response_fallbacks", ["job", "retry", "ep1", "job1"], {"id": ""})
extra("job_response_fallbacks", ["job", "cancel", "ep1", "job1"], {"id": "new1"})
for reply in [{}, {"removed": None}, {"removed": 0}, {"removed": -3}]:
    extra("job_response_fallbacks", ["purge", "ep1", "--yes"], reply)
for code, text in [(404, " job not found \n"), (503, ""), (500, '{"detail":"unavailable"}')]:
    response = requests.Response()
    response.status_code = code
    response.reason = {404: "Not Found", 503: "Service Unavailable", 500: "Internal Server Error"}[code]
    response._content = text.encode()
    extra("job_http_errors", ["job", "get", "ep1", "job1"], failure=requests.HTTPError(response=response))
extra("job_http_errors", ["job", "get", "ep1", "job1"], failure=RuntimeError(UNAUTHORIZED_MSG))
extra("job_confirmation_precedes_credentials", ["purge", "ep1"])

target = ROOT / "tests/fixtures/endpoint_jobs.json"
target.write_text(json.dumps(CASES, indent=2) + "\n", encoding="utf-8")
print(f"Captured {len(CASES)} invocations in {len({c['name'] for c in CASES})} groups")
