"""Characterize scaler decisions from pinned Python, without jobs, HTTP, or OS signals."""
import asyncio
import json
from pathlib import Path
from unittest.mock import Mock, patch
from runpod.serverless.modules import rp_scale as scale

cases = []
async def capture():
    for current in [0, 1, 2, 5]:
        for target in [0, 1, 3]:
            for occupied in [0, 2]:
                remaining = [occupied]
                sleeps = []
                progress = Mock()
                progress.get_job_count.side_effect = lambda: remaining[0]
                with patch.object(scale, "JobsProgress", return_value=progress):
                    scaler = scale.JobScaler({"concurrency_modifier": lambda _: target})
                scaler.current_concurrency = current
                scaler.jobs_queue = asyncio.Queue(maxsize=current)
                async def sleep(delay):
                    sleeps.append(dict(delay=delay, concurrency=scaler.current_concurrency))
                    remaining[0] -= 1
                with patch.object(scale.asyncio, "sleep", sleep):
                    await scaler.set_scale()
                cases.append(dict(operation="scale", current=current, target=target, occupied=occupied,
                                  output=dict(concurrency=scaler.current_concurrency,
                                              capacity=scaler.jobs_queue.maxsize, sleeps=sleeps)))
    for tracked in [0, 2]:
        for queued in [0, 1, 3]:
            progress = Mock()
            progress.get_job_count.return_value = tracked
            with patch.object(scale, "JobsProgress", return_value=progress):
                scaler = scale.JobScaler({})
            scaler.jobs_queue = asyncio.Queue()
            for index in range(queued): scaler.jobs_queue.put_nowait({"id": str(index)})
            cases.append(dict(operation="occupancy", tracked=tracked, queued=queued, output=scaler.current_occupancy()))
    for current in [0, 1, 200]:
        cases.append(dict(operation="identity", current=current, output=scale._default_concurrency_modifier(current)))

with patch.object(scale, "log"):
    asyncio.run(capture())
path = Path(__file__).resolve().parents[1]/"tests/fixtures/scaling.json"
path.write_text(json.dumps(cases, indent=2)+"\n", encoding="utf-8")
print(f"Captured {len(cases)} Python scaler cases")
