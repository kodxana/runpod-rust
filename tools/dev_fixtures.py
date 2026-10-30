"""Capture development payloads and configuration diffs from the pinned Python SDK."""
import json
import os
from pathlib import Path
from unittest.mock import patch

from runpod.apps import App
from runpod.apps.dev import _endpoint_input, _comparable, _changed_fields
from runpod.apps.spec import ResourceSpec, ResourceKind, RouteSpec

cases = []

def resource_spec(values):
    values = {**values, 'kind': ResourceKind(values['kind'])}
    if 'routes' in values:
        values['routes'] = [RouteSpec(**route) for route in values['routes']]
    return ResourceSpec(**values)

def capture(operation, inputs, call):
    case = dict(operation=operation, input=inputs)
    try:
        case['output'] = call()
    except Exception as error:
        case['error'] = str(error)
    cases.append(case)

with patch.dict(os.environ, {}, clear=True), \
     patch('runpod.apps.images.local_python_version', return_value='3.12'), \
     patch('runpod.apps.gpu.live_pool_members', return_value=None), \
     patch('runpod.apps.dev._client_api_key', return_value='fixture-key'):
    specs = [
        {}, {'cpu': 'cpu3c-1-2'}, {'cpu': 'cpu5c-2-4'},
        {'cpu': ['cpu3c-1-2', 'cpu5c-2-4']}, {'gpu': 'ADA_24'},
        {'gpu': '4090'}, {'gpu': 'any'}, {'gpu': ['H100', '4090'], 'gpu_count': 2},
        {'cpu': 'cpu3c-1-2', 'datacenter': ['us_ks_2', 'EU-CZ-1']},
        {'gpu': 'ADA_24', 'datacenter': 'EU-RO-1', 'min_cuda_version': ' 12.4 '},
        {'env': {'BOOL': True, 'NULL': None, 'NUM': 4, 'TEXT': 'ok'}},
        {'env': {'RUNPOD_API_KEY': 'user-key', 'RUNPOD_DEV_APP': 'other',
                 'RUNPOD_DEV_RESOURCE': 'other', 'RUNPOD_DEV_GENERATION': '99',
                 'RUNPOD_MAX_CONCURRENCY': '9'}, 'max_concurrency': 3},
        {'image': 'custom/image:1'}, {'image': '', 'container_disk_gb': 0},
        {'workers': [1, 4], 'idle_timeout': 9, 'execution_timeout_ms': 4500,
         'scaler_type': 'REQUEST_COUNT', 'scaler_value': 6, 'flashboot': False},
    ]
    for kind in ['queue', 'api']:
        for values in specs:
            spec = dict(kind=kind, name='handler', **values)
            if kind == 'api':
                spec['routes'] = [{'method': 'POST', 'path': '/x', 'handler_name': 'x'}]
            inputs = dict(spec=spec, generation=7, overrides={})
            capture('endpoint', inputs, lambda: _endpoint_input(
                App('demo'), resource_spec(spec), 7))
    for overrides in [
        {'RUNPOD_DEV_IMAGE': 'override:1'},
        {'RUNPOD_RUNTIME_TAG': '', 'RUNPOD_DEV_IMAGE': ''},
        {'RUNPOD_RUNTIME_PACKAGE_SPEC': 'runtime==2', 'RUNPOD_PACKAGE_SPEC': 'sdk==3'},
    ]:
        for custom in [False, True]:
            spec = dict(kind='queue', name='handler', image='custom:1' if custom else None)
            with patch.dict(os.environ, overrides, clear=True):
                capture('endpoint', dict(spec=spec, generation=1, overrides=overrides),
                        lambda: _endpoint_input(App('demo'), ResourceSpec(**{**spec, 'kind': ResourceKind.QUEUE})))

for payload in [{}, {'id': 'ep'}, {'template': {}}, {'template': None},
                {'id':'ep','template':{'imageName':'x'}},
                {'id':'ep','template':{'env':[{'key':'RUNPOD_DEV_GENERATION','value':'3'},
                                             {'key':'A','value':'B'}]}}]:
    capture('comparable', payload, lambda: _comparable(payload))
for old, new in [({}, {'x':None}), ({'x':1}, {}),
                 ({'template':{'imageName':'old','env':[]},'workersMax':1},
                  {'template':{'imageName':'new','env':[{'key':'A','value':'B'}]},'workersMax':4}),
                 ({'template':None}, {'template':{'ports':'80/http'}}),
                 ({'a':1,'b':False}, {'b':0,'a':1.0})]:
    capture('changed', dict(old=old,new=new), lambda: _changed_fields(old,new))

path = Path(__file__).resolve().parents[1] / 'tests/fixtures/dev.json'
path.write_text(json.dumps(cases, indent=2)+'\n', encoding='utf-8')
print(f'Captured {len(cases)} development fixtures')
