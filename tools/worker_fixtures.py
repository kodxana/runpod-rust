"""Capture Python worker result envelopes without any network activity."""
import asyncio
import contextlib
import io
import json
import pathlib
from runpod.serverless.modules.rp_job import run_job

async def main():
    cases = []
    values = [None, True, False, 0, 42, 1.25, "", "hello", [], [1,2], {}, {"answer":42},
              {"error":"failed"}, {"error":"failed","answer":1}, {"error":""},
              {"refresh_worker":True}, {"refresh_worker":False,"answer":42},
              {"refresh_worker":True,"answer":42}, {"error":False,"refresh_worker":True}]
    for value in values:
        def handler(job):
            return json.loads(json.dumps(value))
        with contextlib.redirect_stdout(io.StringIO()), contextlib.redirect_stderr(io.StringIO()):
            result = await run_job(handler, {"id":"test", "input":{}})
        cases.append({"handler_output":value,"result":result})
    path = pathlib.Path(__file__).resolve().parents[1] / "tests/fixtures/worker.json"
    path.write_text(json.dumps(cases, indent=2) + "\n")
    print(f"Captured {len(cases)} Python worker result envelopes")

asyncio.run(main())
