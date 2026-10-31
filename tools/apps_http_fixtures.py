"""Record Apps class-API HTTP behavior from the SDK's separate worker runtime.

Pass the runpod-workers/sdk-runtime checkout as the first argument. The fixture
records its commit separately; it is not part of the Python SDK symbol inventory.
"""
import contextlib
import io
import json
import pathlib
import subprocess
import sys

ROOT = pathlib.Path(__file__).resolve().parents[1]
RUNTIME = pathlib.Path(sys.argv[1]).resolve()
sys.path.insert(0, str(RUNTIME / "src"))

from fastapi.testclient import TestClient
from runpod import App
from runpod.apps.app import _clear_registry
from runpod.apps import markers
from runpod_sdk_runtime.api.server import _build_class_app


def record(name, routes, requests):
    _clear_registry()
    calls = []

    def init_instance(self):
        self.ready = False

    @markers.init
    def setup(self):
        self.ready = True

    def bind(route):
        def handler(self, *args):
            calls.append({"handler": route["handler"], "args": list(args), "ready": self.ready})
            if route.get("error"):
                raise ValueError("handler failed")
            if "output" in route:
                return json.loads(json.dumps(route["output"]))
            return {"args": list(args), "ready": self.ready}
        return getattr(markers, route["method"].lower())(route["path"])(handler)

    members = {"__init__": init_instance, "setup": setup}
    members.update({route["handler"]: bind(route) for route in routes})
    handle = App("fixture").api(name="service", cpu="cpu3c-1-2")(type("Service", (), members))
    app = _build_class_app(handle)
    responses = []
    with TestClient(app, raise_server_exceptions=False, follow_redirects=False) as client:
        for request in requests:
            kwargs = {}
            if "json" in request:
                # Encode explicit JSON null rather than httpx's no-body json=None.
                kwargs = {"content": json.dumps(request["json"]), "headers": {"content-type": "application/json"}}
            if "raw" in request:
                kwargs = {"content": request["raw"], "headers": {"content-type": request.get("content_type", "application/octet-stream")}}
            response = client.request(request["method"], request["path"], **kwargs)
            try:
                body = response.json()
            except ValueError:
                body = response.text
            responses.append({"status": response.status_code, "body": body,
                              "allow": response.headers.get("allow"),
                              "location": response.headers.get("location")})
    return {"name": name, "routes": routes, "requests": requests, "responses": responses, "calls": calls}, app.openapi()


def main():
    cases = []
    verbs = ["GET", "POST", "PUT", "PATCH", "DELETE"]
    for method in verbs:
        route = {"method": method, "path": "/route", "handler": "handle"}
        requests = [{"method": method, "path": "/route?query=ignored"}]
        requests += [{"method": method, "path": "/route", "json": body} for body in [None, False, True, 0, 1.5, "", "hi", [], [1, None], {}, {"a": None}]]
        requests += [{"method": method, "path": "/route", "raw": text} for text in ["", "not JSON", "{", "null", "42", '{"json":"without content type"}']]
        requests += [{"method": other, "path": "/route"} for other in ["GET", "POST", "HEAD", "OPTIONS"] if other != method]
        requests += [{"method": method, "path": "/route/?a=1"}, {"method": method, "path": "/missing"}]
        cases.append(record("body " + method, [route], requests)[0])
    for output in [None, False, 0, 1.5, "", "text", [], [None, {}], {}, {"error": "ordinary data"}, {"refresh_worker": True}]:
        cases.append(record("output " + json.dumps(output), [{"method": "POST", "path": "/result", "handler": "result", "output": output}], [{"method": "POST", "path": "/result", "json": {}}])[0])
    cases.append(record("handler error", [{"method": "GET", "path": "/error", "handler": "error", "error": True}], [{"method": "GET", "path": "/error"}])[0])
    routes = [
        {"method": "GET", "path": "/str/{item}", "handler": "string"},
        {"method": "POST", "path": "/int/{item:int}", "handler": "integer"},
        {"method": "GET", "path": "/float/{item:float}", "handler": "number"},
        {"method": "GET", "path": "/uuid/{item:uuid}", "handler": "uuid"},
        {"method": "GET", "path": "/path/{item:path}", "handler": "path"},
        {"method": "GET", "path": "/overlap/{item}", "handler": "a_dynamic"},
        {"method": "GET", "path": "/overlap/static", "handler": "z_static", "output": "shadowed"},
        {"method": "POST", "path": "/same", "handler": "a_post"},
        {"method": "GET", "path": "/same", "handler": "z_get"},
    ]
    requests = [{"method": "GET", "path": path} for path in ["/str/text", "/str/%C3%A9", "/str/", "/float/1.5", "/float/.5", "/float/-1", "/float/12", "/uuid/12345678-1234-4234-8234-123456789abc", "/uuid/invalid", "/path/", "/path/a/b", "/overlap/static", "/same", "/ping"]]
    requests += [{"method": "POST", "path": path, "json": {"x": 1}} for path in ["/int/12", "/int/-1", "/int/no", "/same"]]
    requests += [{"method": "DELETE", "path": "/same"}, {"method": "HEAD", "path": "/ping"}]
    case, schema = record("paths and route order", routes, requests)
    cases.append(case)
    provenance = {"repository": "https://github.com/runpod-workers/sdk-runtime", "commit": subprocess.check_output(["git", "-C", str(RUNTIME), "rev-parse", "HEAD"], text=True).strip()}
    (ROOT / "tests/fixtures/apps_http.json").write_text(json.dumps({**provenance, "cases": cases}, indent=2) + "\n")
    (ROOT / "tests/fixtures/apps_http_openapi.json").write_text(json.dumps(schema, indent=2) + "\n")
    return len(cases), sum(len(case["requests"]) for case in cases)


if __name__ == "__main__":
    with contextlib.redirect_stdout(io.StringIO()), contextlib.redirect_stderr(io.StringIO()):
        cases, requests = main()
    print(f"Captured {cases} Apps HTTP scenarios ({requests} requests)")
