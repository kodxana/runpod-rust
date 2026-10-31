"""Characterize response passthrough and factory health/lifespan in sdk-runtime."""
import contextlib
import io
import json
import pathlib
import subprocess
import sys

ROOT = pathlib.Path(__file__).resolve().parents[1]
RUNTIME = pathlib.Path(sys.argv[1]).resolve()
sys.path.insert(0, str(RUNTIME / "src"))

from fastapi import FastAPI, Request
from fastapi.responses import Response, JSONResponse, StreamingResponse, RedirectResponse
from fastapi.testclient import TestClient
from runpod import App, get
from runpod.apps.app import _clear_registry
from runpod_sdk_runtime.api.server import _build_class_app, _build_factory_app


def snapshot(response):
    names = ["content-type", "content-length", "location", "set-cookie", "x-test", "allow"]
    return {"status": response.status_code, "hex": response.content.hex(),
            "headers": {name: response.headers.get_list(name) for name in names if name in response.headers}}


def response_cases():
    cases = [
        {"name": "binary", "status": 206, "hex": "0001ff8041", "media_type": "application/octet-stream"},
        {"name": "text", "status": 201, "text": "héllo\n", "media_type": "text/plain"},
        {"name": "json", "status": 202, "json": {"error": "ordinary data", "n": 3}},
        {"name": "empty", "status": 204, "hex": ""},
        {"name": "not_modified", "status": 304, "hex": ""},
        {"name": "redirect", "location": "/target?word=caf%C3%A9&n=1"},
        {"name": "cookies", "text": "cookies", "cookies": ["first=one; Path=/; SameSite=lax", "second=two; Path=/; SameSite=lax"]},
        {"name": "stream", "chunks": ["66697273740a", "", "00ff", "6c617374"], "media_type": "application/octet-stream"},
        {"name": "empty_stream", "chunks": [], "media_type": "text/event-stream"},
    ]
    for case in cases:
        _clear_registry()

        def make_response():
            headers = {"x-test": "passthrough"}
            if "chunks" in case:
                return StreamingResponse(iter(bytes.fromhex(part) for part in case["chunks"]), media_type=case["media_type"], headers=headers)
            if "location" in case:
                return RedirectResponse(case["location"], headers=headers)
            if "json" in case:
                return JSONResponse(case["json"], status_code=case["status"], headers=headers)
            response = Response(bytes.fromhex(case["hex"]) if "hex" in case else case["text"],
                                status_code=case.get("status", 200), media_type=case.get("media_type"), headers=headers)
            for cookie in case.get("cookies", []):
                response.raw_headers.append((b"set-cookie", cookie.encode()))
            return response

        class Service:
            @get("/result")
            def result(self):
                return make_response()

        handle = App("responses").api(name="service", cpu="cpu3c-1-2")(Service)
        with TestClient(_build_class_app(handle), follow_redirects=False) as client:
            case["response"] = snapshot(client.get("/result"))
    return cases


def factory_case(kind):
    _clear_registry()
    events = []

    @contextlib.asynccontextmanager
    async def lifespan(app):
        events.append("startup")
        yield
        events.append("shutdown")

    def factory():
        events.append("factory")
        if kind == "raw":
            async def raw(scope, receive, send):
                if scope["type"] == "lifespan":
                    while True:
                        message = await receive()
                        if message["type"] == "lifespan.startup":
                            events.append("startup")
                            await send({"type": "lifespan.startup.complete"})
                        else:
                            events.append("shutdown")
                            await send({"type": "lifespan.shutdown.complete"})
                            return
                else:
                    await Response(b"raw", status_code=209)(scope, receive, send)
            return raw
        app = FastAPI(lifespan=lifespan)
        if kind != "missing":
            app.add_api_route("/ping", lambda: Response(b"custom", status_code=203), methods=[kind.upper()])

        @app.post("/echo")
        async def echo(request: Request):
            return Response(await request.body(), status_code=201, headers={"x-test": request.headers.get("x-test", "missing")})

        return app

    handle = App("factories").api(name="web", cpu="cpu3c-1-2")(factory)
    app = _build_factory_app(handle)
    before = events.copy()
    requests = [{"method": method, "path": "/ping"} for method in ["GET", "POST", "HEAD", "OPTIONS"]]
    requests += [{"method": "GET", "path": "/%70ing?q=1"}, {"method": "POST", "path": "/echo", "raw": "opaque-body"}]
    with TestClient(app, follow_redirects=False) as client:
        responses = [snapshot(client.request(request["method"], request["path"], content=request.get("raw", ""), headers={"x-test": "custom"})) for request in requests]
        during = events.copy()
    return {"kind": kind, "requests": requests, "responses": responses, "before": before, "during": during, "after": events}


def main():
    data = {"repository": "https://github.com/runpod-workers/sdk-runtime",
            "commit": subprocess.check_output(["git", "-C", str(RUNTIME), "rev-parse", "HEAD"], text=True).strip(),
            "responses": response_cases(), "factories": [factory_case(kind) for kind in ["missing", "get", "post", "raw"]]}
    (ROOT / "tests/fixtures/apps_http_responses.json").write_text(json.dumps(data, indent=2, ensure_ascii=True) + "\n")
    return len(data["responses"]), len(data["factories"])


if __name__ == "__main__":
    with contextlib.redirect_stdout(io.StringIO()), contextlib.redirect_stderr(io.StringIO()):
        responses, factories = main()
    print(f"Captured {responses} response cases and {factories} factory scenarios")
