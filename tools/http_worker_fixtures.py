"""Record the pinned Python development API's HTTP contract without external I/O."""
import contextlib
import io
import importlib.metadata
import json
import pathlib
from unittest.mock import patch

from fastapi.testclient import TestClient
from fastapi.openapi.docs import get_swagger_ui_oauth2_redirect_html
from runpod.serverless.modules import rp_fastapi as api

ROOT = pathlib.Path(__file__).resolve().parents[1]
JOB_ID = "test-12345678-1234-4234-8234-123456789abc"


class InlineThread:
    def __init__(self, target, args, **_):
        self.target, self.args = target, args

    def start(self):
        self.target(*self.args)


def record(name, handler_kind, output, requests):
    calls, webhooks = [], []

    def handler(job):
        calls.append(job)
        return job if handler_kind == "echo" else json.loads(json.dumps(output))

    def generator(job):
        calls.append(job)
        yield from output
        if handler_kind == "stream_error":
            raise ValueError("broken stream")

    def send(url, payload):
        webhooks.append({"url": url, "payload": payload})

    api.job_list.clear()
    with patch.object(api.heartbeat, "start_ping"), patch.object(api.rp_capture, "install"), \
         patch.object(api, "RUNPOD_ENDPOINT_ID", "endpoint"), \
         patch.object(api.uuid, "uuid4", return_value=JOB_ID[5:]), \
         patch.object(api.threading, "Thread", InlineThread), patch.object(api, "_send_webhook", send):
        worker = api.WorkerAPI({"handler": generator if handler_kind.startswith("stream") else handler})
        with TestClient(worker.rp_app, raise_server_exceptions=False, follow_redirects=False) as client:
            responses = []
            for request in requests:
                kwargs = {}
                if "json" in request:
                    kwargs["json"] = request["json"]
                if "raw" in request:
                    kwargs.update(content=request["raw"], headers={"content-type": "application/json"})
                response = client.request(request.get("method", "POST"), request["path"], **kwargs)
                try:
                    body = response.json()
                except ValueError:
                    body = response.text
                responses.append({"status": response.status_code, "body": body,
                                  "jobs": api.job_list.get_job_count(), "calls": len(calls)})
            return {"name": name, "handler": handler_kind, "output": output,
                    "requests": requests, "responses": responses, "calls": calls, "webhooks": webhooks}


def main():
    cases = []
    run = {"path": "/run", "json": {"input": {"x": 7}, "webhook": "http://callback.invalid/result"}}
    status = {"path": f"/status/{JOB_ID}"}
    stream = {"path": f"/stream/{JOB_ID}"}
    for value in [None, True, False, 0, 4, 1.25, "", "ok", [], [1, None], {}, {"a": None},
                  {"error": "failed"}, {"error": 42}, {"error": "", "a": 1},
                  {"refresh_worker": True}, {"refresh_worker": True, "a": 1}]:
        label = json.dumps(value)
        cases.append(record("runsync " + label, "single", value,
                            [{"path": "/runsync", "json": run["json"]}]))
        cases.append(record("queued " + label, "single", value, [run, stream, status, status]))
    for kind in ["stream", "stream_error"]:
        for values in [[], [None, 1, {"error": "partial", "refresh_worker": True}]]:
            cases.append(record(f"{kind} runsync {values}", kind, values,
                                [{"path": "/runsync", "json": run["json"]}]))
            cases.append(record(f"{kind} status {values}", kind, values, [run, status, status]))
            cases.append(record(f"{kind} stream {values}", kind, values, [run, stream, status]))
    for path in ["/runsync", "/run"]:
        for body in [None, [], 3, "x", {}, {"input": None}, {"input": []}, {"input": "x"},
                     {"input": {}}, {"input": {}, "webhook": 3}, {"input": {}, "extra": 1}]:
            cases.append(record(f"validate {path} {body}", "single", 1, [{"path": path, "json": body}]))
    cases.append(record("echo jobs", "echo", None, [run, status, {"path": "/runsync", "json": run["json"]},
                         {"path": "/endpoint/realtime", "json": {"id": "real", "input": {"x": 1}, "webhook": "ignored", "extra": True}}]))
    for value in [None, True, False, 0, "x", [], {}, {"error": "failed"}]:
        cases.append(record(f"realtime output {value}", "single", value,
                            [{"path": "/endpoint/realtime", "json": {"id": "real", "input": {}}}]))
    for body in [{}, {"id": "r"}, {"id": 1, "input": {}}, {"id": "r", "input": None},
                 {"id": "r", "input": []}, {"id": "r", "input": False}]:
        cases.append(record(f"realtime input {body}", "single", 1,
                            [{"path": "/endpoint/realtime", "json": body}]))
    cases.append(record("realtime sync generator", "stream", [1, {"a": None}],
                        [{"path": "/endpoint/realtime", "json": {"id": "r", "input": {}}}]))
    cases.append(record("routes", "single", 1, [{"path": "/missing"}, {"path": "/run", "method": "GET"},
                         {"path": "/status/missing"}, {"path": "/stream/missing"}]))
    (ROOT / "tests/fixtures/http_worker.json").write_text(json.dumps(cases, indent=2) + "\n")
    with patch.object(api.heartbeat, "start_ping"), patch.object(api.rp_capture, "install"), \
         patch.object(api, "RUNPOD_ENDPOINT_ID", "endpoint"):
        schema = api.WorkerAPI({"handler": lambda job: job}).rp_app.openapi()
    (ROOT / "tests/fixtures/http_worker_openapi.json").write_text(json.dumps(schema, indent=2) + "\n")
    schema["info"]["description"] = (
        "Test native Runpod workers with deferred job submission, result retrieval, "
        "streaming collection, and optional realtime calls. Use /status or /stream "
        "to execute a job submitted with /run."
    )
    (ROOT / "src/serverless/http/openapi.json").write_text(json.dumps(schema, indent=2) + "\n", encoding="utf-8")
    (ROOT / "src/serverless/http/oauth.html").write_bytes(get_swagger_ui_oauth2_redirect_html().body.rstrip() + b"\n")
    fastapi = importlib.metadata.distribution("fastapi")
    license_file = next(file for file in fastapi.files if str(file).endswith("/licenses/LICENSE"))
    (ROOT / "src/serverless/http/LICENSE-FASTAPI").write_bytes(fastapi.locate_file(license_file).read_bytes())
    return len(cases)


if __name__ == "__main__":
    with contextlib.redirect_stdout(io.StringIO()), contextlib.redirect_stderr(io.StringIO()):
        count = main()
    print(f"Captured {count} Python HTTP worker scenarios")
