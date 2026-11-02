"""Observe one bootstrap queue-error cycle and failure routing in the pinned runtime."""
import contextlib
import io
import json
import os
from pathlib import Path
import subprocess
import sys
import urllib.error
from unittest.mock import patch

ROOT = Path(__file__).resolve().parents[1]
RUNTIME = Path(sys.argv[1]).resolve()
sys.path.insert(0, str(RUNTIME / "src"))
from runpod_sdk_runtime import bootstrap
from runpod_sdk_runtime.task import runner


class Stop(BaseException):
    pass


class Response(io.BytesIO):
    def __init__(self, status, body):
        super().__init__(body.encode())
        self.status = status


def capture(name, body="{}", get_status=200, post_status=200, network_error=False):
    requests, delays = [], []
    called = False

    def urlopen(request, timeout):
        nonlocal called
        verb = request.get_method()
        if verb == "GET":
            if called:
                raise Stop
            called = True
        record = dict(method=verb, url=request.full_url, timeout=timeout,
                      headers={k.lower(): v for k, v in request.header_items()})
        if request.data:
            payload = json.loads(request.data)
            payload["error"] = json.loads(payload["error"])
            record["body"] = payload
        requests.append(record)
        status = get_status if verb == "GET" else post_status
        if network_error and verb == "GET":
            raise TimeoutError("source transport timeout")
        if status >= 400:
            raise urllib.error.HTTPError(request.full_url, status, "fixture", {}, None)
        return Response(status, body if verb == "GET" else "")

    settings = dict(RUNPOD_WEBHOOK_GET_JOB="http://fixture/get",
                    RUNPOD_WEBHOOK_POST_OUTPUT="http://fixture/out/$ID/again/$ID",
                    RUNPOD_AI_API_KEY="fixture-secret")
    with patch.dict(os.environ, settings, clear=True), \
         patch.object(bootstrap.urllib.request, "urlopen", urlopen), \
         patch.object(bootstrap.time, "sleep", delays.append), \
         contextlib.redirect_stderr(io.StringIO()):
        try:
            bootstrap._queue_error_loop(bootstrap.PhaseError("attach", "manifest missing"))
        except Stop:
            pass
    return dict(name=name, body=body, get_status=get_status, post_status=post_status,
                network_error=network_error, requests=requests, delays=delays)


def main():
    cases = [capture(name, **args) for name, args in [
        ("report job", dict(body='{"id":"job-1"}')),
        ("empty response", dict(body="")),
        ("missing id", dict(body="{}")),
        ("null id", dict(body='{"id":null}')),
        ("empty id", dict(body='{"id":""}')),
        ("zero id", dict(body='{"id":0}')),
        ("false id", dict(body='{"id":false}')),
        ("empty array id", dict(body='{"id":[]}')),
        ("numeric id", dict(body='{"id":42}')),
        ("array body", dict(body="[]")),
        ("null body", dict(body="null")),
        ("invalid JSON", dict(body="{invalid")),
        ("GET accepted", dict(get_status=202)),
        ("GET no content", dict(get_status=204)),
        ("GET denied", dict(get_status=401)),
        ("GET unavailable", dict(get_status=503)),
        ("GET timeout", dict(network_error=True)),
        ("POST rejected", dict(body='{"id":"job-1"}', post_status=500)),
        ("POST accepted", dict(body='{"id":"job-1"}', post_status=202)),
    ]]
    routes = []
    error = bootstrap.PhaseError("locate", "missing")
    for kind in (None, "queue", "api", "task", "", "unknown", "API"):
        observed = []
        with patch.dict(os.environ, {} if kind is None else {"RUNPOD_RUNTIME_KIND":kind}, clear=True), \
             patch.object(bootstrap, "_queue_error_loop", lambda e: observed.append(dict(route="queue", payload=bootstrap._error_payload(e)))), \
             patch.object(bootstrap, "_api_error_server", lambda e: observed.append(dict(route="api", payload=bootstrap._error_payload(e)))), \
             patch.object(runner, "main", lambda **kwargs: observed.append(dict(route="task", payload=kwargs["bootstrap_error"]))), \
             contextlib.redirect_stderr(io.StringIO()):
            bootstrap._report_error(error)
        routes.append(dict(kind=kind, **observed[0]))
    result = dict(repository="https://github.com/runpod-workers/sdk-runtime",
                  commit=subprocess.check_output(["git", "-C", str(RUNTIME), "rev-parse", "HEAD"], text=True).strip(),
                  cases=cases, routes=routes)
    (ROOT / "tests/fixtures/bootstrap_reporting.json").write_text(json.dumps(result, indent=2)+"\n", encoding="utf-8", newline="\n")
    print(f"Captured {len(cases)} queue cycles and {len(routes)} routes")


if __name__ == "__main__":
    main()
