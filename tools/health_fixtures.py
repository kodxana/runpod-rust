"""Capture health parsers and eligibility decisions from the pinned Python SDK."""
import ast
import json
import os
import sys
from pathlib import Path
from unittest.mock import patch
from runpod._health import is_serverless_environment, is_early_check_eligible
from runpod._health.gpu import _parse_gpu_test_output
from runpod._health.system import _parse_version, _network_probe_target

root = Path(__file__).resolve().parents[1]
cases = []
tree = ast.parse((root.parent / 'runpod-python/tests/test_serverless/test_modules/test_fitness/test_gpu_checks.py').read_text(encoding='utf-8'))
for cls in tree.body:
    if isinstance(cls, ast.ClassDef) and cls.name == 'TestGpuTestOutputParsing':
        for func in cls.body:
            for node in ast.walk(func):
                if isinstance(node, ast.Assign) and isinstance(node.value, ast.Constant) and isinstance(node.value.value, str) and any(isinstance(t, ast.Name) and t.id == 'output' for t in node.targets):
                    text = node.value.value
                    cases.append(dict(operation='gpu_parse', test=func.name, input=text, output=_parse_gpu_test_output(text)))
for text in ['', 'Found bogus GPUs:\nGPU 0 memory allocation test passed.',
             'Found -1 GPUs:\nCannot allocate', 'Found 1 GPUs:\nGPU 0 MEMORY ALLOCATION TEST PASSED.',
             'Found 1 GPUs:\nGPU 0 memory allocation test passed.\nCUDA Driver Version: error 12.2',
             'Found 3 GPUs:\nFound 1 GPUs:\nGPU 0 memory allocation test passed.',
             'Found 1 GPUs:\nGPU 0 memory allocation test passed.\nGPU 0 memory allocation test passed.',
             'found 1 GPUs:\nGPU 0 memory allocation test passed.',
             'CUDA Driver Version: 12.2: detail\r\nLinux Kernel Version: 6.2\nUnable to start']:
    cases.append(dict(operation='gpu_parse', input=text, output=_parse_gpu_test_output(text)))
for text in ['12.2', 'CUDA Version 11.8', 'release 12.1, V12.1.105', 'unknown', '', '10', 'v9.02.1', ' 13.5 ', 'version 0.0']:
    cases.append(dict(operation='version', input=text, output=_parse_version(text)))
for url in [None, '', 'https://api.example/job', 'http://worker:8080/path?secret=token',
            'https://user:pass@HOST:0/job', 'worker/job', 'ftp://host/file',
            'https://host:invalid', 'https://[::1]:8443/job', 'https://host:99999']:
    cases.append(dict(operation='network_target', input=url, output=_network_probe_target(url)))
for env,args in [({},[]), ({'RUNPOD_ENDPOINT_ID':'e','RUNPOD_WEBHOOK_GET_JOB':'url'},[]),
                 ({'RUNPOD_ENDPOINT_ID':'e','RUNPOD_WEBHOOK_GET_JOB':'url','RUNPOD_TEST':' YES '},[]),
                 ({'RUNPOD_ENDPOINT_ID':'e','RUNPOD_WEBHOOK_GET_JOB':'url','RUNPOD_REALTIME_PORT':'8000'},[]),
                 ({'RUNPOD_ENDPOINT_ID':'e','RUNPOD_WEBHOOK_GET_JOB':'url'},['--test_input={}']),
                 ({'RUNPOD_ENDPOINT_ID':'e','RUNPOD_WEBHOOK_GET_JOB':'url'},['--rp_serve_api']),
                 ({'RUNPOD_ENDPOINT_ID':'  ','RUNPOD_WEBHOOK_GET_JOB':'url'},[]),
                 ({'RUNPOD_ENDPOINT_ID':'e','RUNPOD_WEBHOOK_GET_JOB':'url','RUNPOD_REALTIME_PORT':' 0 '},['--test_inputs'])]:
    with patch.dict(os.environ,env,clear=True),patch.object(sys,'argv',['handler']+args):
        cases.append(dict(operation='eligibility',environment=env,args=args,
                          output=dict(serverless=is_serverless_environment(),early=is_early_check_eligible())))
(root/'tests/fixtures/health.json').write_text(json.dumps(cases,indent=2)+'\n',encoding='utf-8')
print(f'Captured {len(cases)} health cases')
