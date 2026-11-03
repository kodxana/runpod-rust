"""Capture bounded job snapshot semantics from the pinned Python worker."""
import json
from pathlib import Path
from runpod.serverless.modules.worker_state import PingJobMirror

cases = []
for capacity, values in [
    (65536, [None, '', 'job_a,job_b', None]),
    (32, [','.join(f'id{n:08d}' for n in range(10)), 'short']),
    (8, ['abcdefgh', 'ab,cdefgh', 'abc,def', 'abc,defg']),
    (4, ['\u00e9\u00e9', 'a,\u00e9', 'x\x00y', 'ok']),
    (1, ['one', '', None]),
    (0, ['hello', '', None]),
]:
    mirror = PingJobMirror(capacity=capacity)
    outputs = []
    for value in values:
        mirror.set(value)
        outputs.append(mirror.get())
    cases.append(dict(capacity=capacity, inputs=values, outputs=outputs))
path = Path(__file__).resolve().parents[1] / 'tests/fixtures/job_mirror.json'
path.write_text(json.dumps(cases, indent=2) + '\n', encoding='utf-8')
print(f'Captured {len(cases)} mirror sequences')
