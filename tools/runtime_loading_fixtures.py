"""Capture deployed resource selection and pre-import mount configuration."""
import contextlib
import io
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
from types import SimpleNamespace
from unittest.mock import patch

ROOT = Path(__file__).resolve().parents[1]
RUNTIME = Path(sys.argv[1]).resolve()
sys.path.insert(0, str(RUNTIME / "src"))
import runpod
from runpod.apps.app import _clear_registry
from runpod.apps.volume import _configure_mounts, NetworkVolume, GlobalVolume
from runpod_sdk_runtime.queue import worker
from runpod_sdk_runtime.api import server
from runpod_sdk_runtime.task import deployed
from runpod_sdk_runtime.mounts import configure_mounts


def declaration(kind="queue", name="work", app="example", qualname="work", tag="first", init=False):
    return dict(kind=kind, name=name, app=app, qualname=qualname, tag=tag, init=init)


def selection(name, kind, declarations, manifest=None, overrides=None):
    values = dict(FLASH_RESOURCE_NAME="work", RUNPOD_MOUNTS="[]")
    if kind == "task":
        values.update(RUNPOD_RUNTIME_KIND="task", FLASH_APP_NAME="example", FLASH_ENVIRONMENT="default", RUNPOD_BUILD_ID="build")
    values.update(overrides or {})
    if manifest is None:
        manifest = {"app":"example", "resources":[{"name":"work", "kind":kind, "module":"user", "qualname":"work"}]}
    imports, init_events = [], []
    _clear_registry()
    _configure_mounts([])
    handles = []
    for item in declarations:
        if "alias" in item:
            handles.append(handles[item["alias"]])
            continue
        app = runpod.App(item["app"])
        if item["kind"] == "api":
            route = runpod.get("/tag")(lambda self, tag=item["tag"]: tag)
            cls = type("Service", (), {"tag": route})
            handle = app.api(name=item["name"], cpu="cpu3c-1-2")(cls)
        else:
            def function(tag=item["tag"]):
                return tag
            function.__name__ = item["qualname"].split(".")[-1]
            function.__qualname__ = item["qualname"]
            handle = getattr(app, item["kind"])(name=item["name"], cpu="cpu3c-1-2")(function)
            if item["init"]:
                def initialize(tag=item["tag"]):
                    init_events.append(tag)
                handle.init(initialize)
        handle._fixture_tag = item["tag"]
        handles.append(handle)
    module = SimpleNamespace(**{f"h{i}": h for i, h in enumerate(handles)})

    def load(module_name):
        imports.append(module_name)
        if module_name != "user":
            raise ImportError(f"unknown module: {module_name}")
        return module

    with tempfile.TemporaryDirectory() as directory, patch.dict(os.environ, values, clear=True):
        (Path(directory) / worker.MANIFEST_NAME).write_text(json.dumps(manifest))
        os.environ["RUNPOD_APP_DIR"] = directory
        tag, error, phase = None, None, None
        with patch.object(worker, "APP_DIR", directory), patch.object(server, "APP_DIR", directory), patch.object(worker.importlib, "import_module", load):
            try:
                configure_mounts()
                handle = {"queue":worker._load_deployed_handle, "api":server._load_api_handle, "task":deployed.load_handle}[kind]()
                tag = handle._fixture_tag
            except Exception as exc:
                error = str(exc).split(" (has:")[0]
                phase = getattr(exc, "phase", None)
            finally:
                deployed.close()
    return dict(name=name, kind=kind, declarations=declarations, values=values, manifest=manifest,
                selected=tag, error=error, phase=phase, imports=imports, init=init_events)


def mounts(raw, lookup=None):
    _configure_mounts([])
    values = {} if raw is None else {"RUNPOD_MOUNTS":raw}
    result, error = None, None
    with patch.dict(os.environ, values, clear=True):
        try:
            configure_mounts()
            if lookup:
                volume = {"network":NetworkVolume,"global":GlobalVolume}[lookup[0]](lookup[1])
                result = volume.path.as_posix()
        except Exception as exc:
            error = str(exc)
    return dict(raw=raw, lookup=lookup, result=result, error=error)


def main():
    cases = [selection("queue", "queue", [declaration(init=True)]),
             selection("queue accepts task handle", "queue", [declaration("task")]),
             selection("queue skips API", "queue", [declaration("api"), declaration(tag="queue")]),
             selection("queue first handle wins", "queue", [declaration(app="one"), declaration(app="two",tag="second")]),
             selection("queue missing handle", "queue", [declaration(name="different")]),
             selection("API", "api", [declaration("api")]),
             selection("API skips function", "api", [declaration(),declaration("api",tag="api")]),
             selection("API missing handle", "api", [declaration()]),
             selection("task initializes", "task", [declaration("task",init=True)]),
             selection("task aliases deduplicate", "task", [declaration("task",init=True), {"alias":0}]),
             selection("task ambiguity", "task", [declaration("task",init=True), declaration("task",tag="second",init=True)]),
             selection("task wrong owner", "task", [declaration("task",app="other")]),
             selection("task wrong qualname", "task", [declaration("task",qualname="other")]),
             selection("task rejects queue", "task", [declaration()])]
    for kind in ["queue", "api", "task"]:
        cases.append(selection(f"{kind} missing manifest resource", kind, [], manifest={"app":"example","resources":[{"name":"other","module":"user"}]}))
        cases.append(selection(f"{kind} unknown module", kind, [], manifest={"app":"example","resources":[{"name":"work","kind":kind,"module":"absent","qualname":"work"}]}))
    cases.append(selection("queue first manifest entry wins", "queue", [declaration()], manifest={"resources":[{"name":"work","module":"user"},{"name":"work","module":"absent"}]}))
    cases.append(selection("task nested qualified name", "task", [declaration("task",qualname="Tasks.work")], manifest={"app":"example","resources":[{"name":"work","kind":"task","module":"user","qualname":"Tasks.work"}]}))
    cases.append(selection("task manifest qualified name mismatch", "task", [declaration("task")], manifest={"app":"example","resources":[{"name":"work","kind":"task","module":"user","qualname":"other"}]}))
    for key in ["FLASH_APP_NAME","FLASH_ENVIRONMENT","FLASH_RESOURCE_NAME","RUNPOD_BUILD_ID"]:
        cases.append(selection(f"task identity {key}", "task", [], overrides={key:" "}))
    cases.append(selection("task runtime kind", "task", [], overrides={"RUNPOD_RUNTIME_KIND":"queue"}))
    for name, manifest in [("task wrong manifest app", {"app":"wrong","resources":[]}),
                           ("task duplicate manifest", {"app":"example","resources":[{"name":"work","kind":"task"}]*2}),
                           ("task nonobject manifest", []),
                           ("task nonobject resource", {"app":"example","resources":[1]}),
                           ("task wrong manifest kind", {"app":"example","resources":[{"name":"work","kind":"queue","module":"user","qualname":"work"}]}),
                           ("task missing symbol", {"app":"example","resources":[{"name":"work","kind":"task"}]})]:
        cases.append(selection(name, "task", [], manifest=manifest))
    bindings = [{"kind":"network","reference":"data","id":"nv-1","path":"/data//./cache"}]
    mount_cases = [mounts(raw) for raw in [None, "[]", "", "bad", "{}", "[1]", "[{}]",
        '[{"kind":"network","reference":"data","id":"nv-1"}]']]
    mount_cases += [mounts(json.dumps(bindings), ["network",name]) for name in ["data","nv-1","missing"]]
    mount_cases.append(mounts(json.dumps(bindings+bindings), ["network","data"]))
    mount_cases.append(mounts(json.dumps(bindings+[dict(bindings[0],path="/other")]), ["network","data"]))
    detection = []
    for values in [{}, {"FLASH_RESOURCE_NAME":"flash"}, {"RUNPOD_RESOURCE_NAME":"fallback"},
                   {"FLASH_RESOURCE_NAME":"", "RUNPOD_RESOURCE_NAME":"fallback"},
                   {"FLASH_RESOURCE_NAME":" ", "RUNPOD_RESOURCE_NAME":"fallback"}]:
        for exists in [False, True]:
            with tempfile.TemporaryDirectory() as directory, patch.dict(os.environ, values, clear=True), patch.object(worker,"APP_DIR",directory):
                if exists: (Path(directory)/worker.MANIFEST_NAME).write_text("{}")
                detection.append(dict(values=values, manifest=exists, name=worker._resource_name(), deployed=worker._is_deployed()))
    data = dict(repository="https://github.com/runpod-workers/sdk-runtime",
                commit=subprocess.check_output(["git","-C",str(RUNTIME),"rev-parse","HEAD"],text=True).strip(),
                selection=cases, mounts=mount_cases, detection=detection)
    (ROOT/"tests/fixtures/runtime_loading.json").write_text(json.dumps(data,indent=2)+"\n",encoding="utf-8",newline="\n")
    _configure_mounts([])
    _clear_registry()
    return len(cases)+len(mount_cases)+len(detection)


if __name__ == "__main__":
    with contextlib.redirect_stdout(io.StringIO()), contextlib.redirect_stderr(io.StringIO()):
        count = main()
    print(f"Captured {count} runtime loading cases")
