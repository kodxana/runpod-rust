"""Characterize pinned job observation with deterministic clocks and no network."""
import copy
import json
from pathlib import Path
from unittest.mock import patch
import tempfile

import click
from click.testing import CliRunner
import pytest
import requests
from runpod.rp_cli import endpoint as ep

ROOT = Path(__file__).resolve().parents[1]
scratch_root = (ROOT / "test-results").resolve()
scratch_root.mkdir(exist_ok=True)
with tempfile.TemporaryDirectory(prefix="follow-source-", dir=scratch_root) as scratch:
    assert Path(scratch).resolve().is_relative_to(scratch_root)
    result = pytest.main([str(ROOT.parent / "runpod-python/tests/test_cli/test_endpoint_command.py"),
        "-k", "test_job_wait_and_stream or test_job_stream_interrupt", "--no-cov", "-q",
        "--basetemp", str(Path(scratch) / "pytest")])
if result:
    raise SystemExit(result)

CASES = []


def capture(name, replies, *, stream=False, as_json=False, timeout=None, interrupt_sleep=None):
    case = dict(name=name, stream=stream, as_json=as_json, timeout=timeout,
                interrupt_sleep=interrupt_sleep, requests=[], sleeps=[])
    clock = [0.0]
    index = [0]

    class Client:
        def get(self, path, **kwargs):
            reply = copy.deepcopy(replies[min(index[0], len(replies)-1)])
            index[0] += 1
            assert index[0] < 100, "unbounded scenario"
            request = dict(path=path, timeout=kwargs.get("timeout", 10), **reply)
            case["requests"].append(request)
            clock[0] += reply.get("elapsed", 0)
            if reply.get("failure") == "interrupt": raise KeyboardInterrupt
            if reply.get("failure") == "read": raise requests.ReadTimeout("read timed out")
            if reply.get("failure") == "connect": raise requests.ConnectTimeout("connect timed out")
            return reply["response"]

    def sleep(seconds):
        case["sleeps"].append(seconds)
        if len(case["sleeps"]) == interrupt_sleep: raise KeyboardInterrupt
        clock[0] += seconds

    @click.command()
    @ep._data_plane_errors()
    def invoke():
        ep._follow_job(Client(), "ep1", "job1", as_json, stream=stream, timeout=timeout)

    with patch.object(ep.time, "monotonic", lambda: clock[0]), patch.object(ep.time, "sleep", sleep):
        result = CliRunner().invoke(invoke)
    assert result.exception is None or isinstance(result.exception, SystemExit), result.exception
    case.update(stdout=result.stdout, stderr=result.stderr, code=result.exit_code)
    CASES.append(case)


def response(status="COMPLETED", **fields):
    return {"response": dict(id="job1", status=status, **fields)}


capture("test_job_wait_and_stream", [response(output="done")], timeout=30)
capture("test_job_wait_and_stream", [response(stream=[{"output":"chunk"}]), response(stream=[])], stream=True)
capture("test_job_stream_interrupt", [{"failure":"interrupt"}], stream=True)
for value in [None, "", "done", "done\n", {"ok":True,"text":"café"}, [1,False]]:
    for as_json in [False, True]:
        capture("final_output_shapes", [response(output=value)], as_json=as_json)
for status in ["FAILED", "CANCELLED", "TIMED_OUT"]:
    for as_json in [False, True]:
        capture("failed_final_outputs", [response(status, error="worker failed")], as_json=as_json)
for as_json in [False, True]:
    capture("mixed_chunks", [response("IN_PROGRESS", stream=[]),
        response("IN_PROGRESS", stream=[{"output":"hel"},{"output":"lo"}]),
        response(stream=[{"output":{"done":True}}, {"output":"café"}, {"output":None}]),
        response(stream=[])], stream=True, as_json=as_json)
    capture("failed_stream", [response("FAILED", stream=[{"output":"partial"}]),
        response("FAILED", stream=[]), response("FAILED", error="oom")], stream=True, as_json=as_json)
for fields in [{}, {"stream":None}, {"stream":[]}]:
    capture("missing_chunks_fallback", [response("IN_QUEUE", **fields), response(**fields), response(output="done")], stream=True)
capture("chunk_newlines", [response(stream=[{"output":"x\n"}, {"output":""}, {"output":2}]), response(stream=[])], stream=True)
capture("read_timeout_retry", [{"failure":"read"}, response(stream=[]), response(output="done")], stream=True)
for stream in [False, True]:
    capture("deadline_while_pending", [response("IN_QUEUE")], stream=stream, timeout=2.5)
    capture("sleep_interrupt", [response("IN_PROGRESS")], stream=stream, interrupt_sleep=1)
    for failure in ["read", "connect"]:
        capture("request_timeout_at_deadline", [{"failure":failure, "elapsed":5}], stream=stream, timeout=5)
        capture("request_timeout_before_deadline", [{"failure":failure}, response(), response(output="done")], stream=stream, timeout=5)
capture("retry_deadline", [{"failure":"read", "elapsed":2},{"failure":"read", "elapsed":2},{"failure":"read", "elapsed":1}], stream=True, timeout=5)
for status, chunks in [("COMPLETED", [{"output":"done"}]), ("COMPLETED", []), ("FAILED", [])]:
    capture("terminal_drains_after_deadline", [dict(response(status, stream=chunks), elapsed=5.1),
        response(status, stream=[]) if chunks else response(status, output="done", error="worker failed")], stream=True, timeout=5)
capture("fallback_interrupt", [response(stream=[]), {"failure":"interrupt"}], stream=True)
(ROOT / "tests/fixtures/job_follow.json").write_text(json.dumps(CASES, indent=2)+"\n", encoding="utf-8")
print(f"Captured {len(CASES)} cases in {len({case['name'] for case in CASES})} groups")
