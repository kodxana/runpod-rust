"""Capture deployed queue contracts from the separately pinned sdk-runtime."""
import asyncio
import contextlib
import inspect
import io
import json
import os
from pathlib import Path
import subprocess
import sys
from types import SimpleNamespace
from unittest.mock import patch

ROOT = Path(__file__).resolve().parents[1]
RUNTIME = Path(sys.argv[1]).resolve()
sys.path.insert(0, str(RUNTIME / "src"))

from runpod_sdk_runtime.queue import worker


def echo(**kwargs):
    return kwargs


def keys(**kwargs):
    return list(kwargs)


def add(a, b):
    return {"sum": a + b}


async def double(x):
    return x * 2


def sequence(n):
    yield from range(n)


async def async_sequence(n):
    for i in range(n):
        yield i


def failure(**kwargs):
    raise ValueError("body failed")


def stream_failure(**kwargs):
    yield "first"
    raise ValueError("stream failed")


FUNCTIONS = dict(echo=echo, keys=keys, add=add, double=double, sequence=sequence,
                 async_sequence=async_sequence, failure=failure,
                 stream_failure=stream_failure)


async def record(name, function, job):
    handler = worker._make_deployed_handler(SimpleNamespace(_fn=FUNCTIONS[function]))
    outputs = []
    error = False
    try:
        if inspect.isasyncgenfunction(handler):
            async for item in handler(job):
                outputs.append(item)
        elif inspect.isgeneratorfunction(handler):
            for item in handler(job):
                outputs.append(item)
        else:
            outputs.append(await handler(job))
    except (ValueError, TypeError):
        error = True
    return dict(name=name, function=function, job=job, outputs=outputs, error=error,
                streaming=inspect.isasyncgenfunction(handler) or inspect.isgeneratorfunction(handler))


async def calls():
    cases = [("sync sum", "add", {"input": {"a": 1, "b": 2}}),
             ("async double", "double", {"input": {"x": 5}}),
             ("missing input", "echo", {}),
             ("marker", "echo", {"input": {"__empty": True}}),
             ("marker removed regardless of value", "echo", {"input": {"__empty": False, "x": [1, None]}}),
             ("marker removal retains keyword order", "keys", {"input": {"first": 1, "__empty": True, "second": 2, "third": 3}}),
             ("sync generator", "sequence", {"input": {"n": 3}}),
             ("async generator", "async_sequence", {"input": {"n": 2}}),
             ("empty generator", "sequence", {"input": {"n": 0}}),
             ("body failure", "failure", {"input": {}}),
             ("partial failure", "stream_failure", {"input": {}})]
    for value in [None, False, 0, 0.0, "", [], {}]:
        cases.append((f"falsey input {value!r}", "echo", {"input": value}))
    for value in [True, 3, "bad", [1]]:
        cases.append((f"invalid input {value!r}", "echo", {"input": value}))
    return [await record(*case) for case in cases]


def main():
    cases = asyncio.run(calls())
    concurrency = []
    for value in [None, "8", "banana", "0", "-2", "  +4 ", "1", "1.5", ""]:
        with patch.dict(os.environ):
            os.environ.pop("RUNPOD_MAX_CONCURRENCY", None)
            if value is not None:
                os.environ["RUNPOD_MAX_CONCURRENCY"] = value
            settings = worker._worker_config(echo)
            modifier = settings.get("concurrency_modifier")
            concurrency.append(dict(value=value, maximum=worker._max_concurrency(),
                                    modified=None if modifier is None else [modifier(n) for n in [0, 1, 9]]))
    init = []
    for kind in ["none", "sync", "async", "failure"]:
        events = []

        def sync_init():
            events.append("init")

        async def async_init():
            events.append("init")

        def fail_init():
            events.append("init")
            raise ValueError("init failed")

        handle = SimpleNamespace()
        if kind != "none":
            handle._init_fn = {"sync": sync_init, "async": async_init, "failure": fail_init}[kind]
        failed = False
        try:
            worker._run_init(handle)
        except ValueError:
            failed = True
        init.append(dict(kind=kind, events=events, error=failed))
    data = dict(repository="https://github.com/runpod-workers/sdk-runtime",
                commit=subprocess.check_output(["git", "-C", str(RUNTIME), "rev-parse", "HEAD"], text=True).strip(),
                calls=cases, concurrency=concurrency, init=init)
    (ROOT / "tests/fixtures/apps_queue.json").write_text(json.dumps(data, indent=2) + "\n", encoding="utf-8")
    return len(cases) + len(concurrency) + len(init)


if __name__ == "__main__":
    with contextlib.redirect_stdout(io.StringIO()), contextlib.redirect_stderr(io.StringIO()):
        count = main()
    print(f"Captured {count} deployed queue runtime cases")
