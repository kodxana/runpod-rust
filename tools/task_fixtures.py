"""Capture runtime images, launchers, and ephemeral task payloads from pinned Python."""
import json
import os
from pathlib import Path
from unittest.mock import patch

from runpod.apps.images import image_for_spec, runtime_image
from runpod.apps.shim import runtime_launcher
from runpod.apps.spec import ResourceKind, ResourceSpec
from runpod.apps.tasks import _pod_input, cuda_versions_at_least, unwrap_task_response
from runpod.apps.serialization import serialize_arg

cases = []

def capture(operation, inputs, call):
    case = dict(operation=operation, input=inputs)
    try:
        case['output'] = call()
    except Exception as error:
        case['error'] = str(error)
    cases.append(case)

with patch.dict(os.environ, {'RUNPOD_RUNTIME_TAG': 'test'}, clear=True):
    for kind in ResourceKind:
        for gpu in [False, True]:
            for version in ['3.10', '3.11', '3.12', '3.13', '3.14', '', '3.9']:
                capture('image', dict(kind=kind.value, gpu=gpu, version=version),
                        lambda: runtime_image(kind, gpu=gpu, python_version=version))
        capture('launcher', kind.value, lambda: runtime_launcher(kind.value))
    specs = [
        {}, {'gpu': '4090'}, {'gpu': ['H100', '4090'], 'gpu_count': 2},
        {'gpu': 'any'}, {'gpu': 'NVIDIA H100 PCIe', 'min_cuda_version': '12.4'},
        {'gpu': '4090', 'min_cuda_version': ' 12.4 '},
        {'cpu': 'cpu3c-2-4'}, {'cpu': 'cpu5c-2-4'},
        {'cpu': ['cpu5c-2-4', 'cpu3c-2-4']},
        {'cpu': ['cpu3c-2-4', 'cpu5c-2-4']},
        {'gpu': '4090', 'datacenter': ' eu_ro_1 ', 'container_disk_gb': 80},
        {'cpu': 'cpu3c-2-4', 'datacenter': ['us_ks_2', 'EU-CZ-1']},
        {'env': {'RUNPOD_TASK_TOKEN': 'user-override', 'RUNPOD_TASK_PORT': 99,
                 'BOOL': True, 'NULL': None, 'N': 2.5}},
        {'image': 'custom/image:latest', 'env': {'RUNPOD_PACKAGE_SPEC': 'user-sdk'}},
        {'image': '', 'container_disk_gb': 0},
    ]
    for values in specs:
        values = dict(kind='task', name='calculate', **values)
        with patch('runpod.apps.tasks.secrets.token_hex', return_value='deadbeef'), \
             patch('runpod.apps.images.local_python_version', return_value='3.12'), \
             patch.dict(os.environ, {'RUNPOD_RUNTIME_PACKAGE_SPEC': 'runtime==2', 'RUNPOD_PACKAGE_SPEC': 'sdk==3'}):
            capture('pod', values, lambda: _pod_input(ResourceSpec(**{**values, 'kind': ResourceKind.TASK}), 'test-token', 'calculate'))
    for minimum in ['11.8', '12.5', '13.0', '14.0', '12.99', 'broken']:
        capture('cuda', minimum, lambda: cuda_versions_at_least(minimum))
    for response in [dict(success=True, json_result={'answer':42}), dict(success=True),
                     dict(success=True, result=serialize_arg([1,'ok',None]),json_result='ignored'),
                     dict(success=False,error='failed'), dict(success=False,error=None),
                     dict(success=False,error=''), {}, dict(success=1,json_result=4)]:
        capture('unwrap', response, lambda: unwrap_task_response(response))
    custom = ResourceSpec(kind=ResourceKind.TASK,name='custom',image='custom:1')
    capture('custom_image', '3.9', lambda: image_for_spec(custom,python_version='3.9'))

path = Path(__file__).resolve().parents[1] / 'tests/fixtures/tasks.json'
path.write_text(json.dumps(cases,indent=2)+'\n',encoding='utf-8')
print(f'Captured {len(cases)} task/image/launcher cases')
