"""Capture manifest and deployed endpoint contracts without building or deploying."""
import json
import os
import tempfile
from pathlib import Path
from types import SimpleNamespace
from unittest.mock import patch

import runpod
from runpod.apps.deploy import build_manifest, _deployed_endpoint_input
from runpod.apps.spec import ResourceSpec, ResourceKind, RouteSpec

cases=[]
def capture(operation,inputs,call):
    case=dict(operation=operation,input=inputs)
    try: case['output']=call()
    except Exception as error: case['error']=str(error)
    cases.append(case)

def spec(values):
    values=dict(values)
    values['kind']=ResourceKind(values['kind'])
    if 'routes' in values: values['routes']=[RouteSpec(**r) for r in values['routes']]
    return ResourceSpec(**values)

def app_def(values):
    resources={}
    for index,resource in enumerate(values['resources']):
        fn=None
        if symbol:=resource.get('symbol'):
            def fn(): pass
            fn.__module__=symbol['module']
            fn.__qualname__=symbol['qualname']
        resources[str(index)]=SimpleNamespace(spec=spec(resource['spec']),_fn=fn)
    return SimpleNamespace(name=values['name'],env=values['environment'],resources=resources)

symbol={'module':'pkg.worker','qualname':'Worker.run'}
resource_sets=[[],
    [dict(spec=dict(kind='queue',name='queue',cpu='cpu5c-2-4',dependencies=['numpy']),symbol=symbol)],
    [dict(spec=dict(kind='api',name='api',gpu='H100',routes=[dict(method='GET',path='/value',handler_name='value')]),symbol=symbol),dict(spec=dict(kind='task',name='t'))],
    [dict(spec=dict(kind='queue',name='q'))],
    [dict(spec=dict(kind='task',name='t',schedule='0 * * * *'))],
    [dict(spec=dict(kind='queue',name='queue',image='custom',env={'X':1},min_cuda_version=' 12.4 '),symbol=symbol)],
]
with tempfile.TemporaryDirectory() as root:
    for resources in resource_sets:
        for excluded in [[],['torch','triton']]:
            values=dict(name='demo',environment='staging',resources=resources)
            inputs=dict(app=values,python_version='3.13',excluded_packages=excluded)
            capture('manifest',inputs,lambda:build_manifest(app_def(values),Path(root),python_version='3.13',excluded_packages=excluded))

variants=[{},dict(cpu='cpu3c-2-4'),dict(cpu='cpu5c-2-4'),dict(cpu=['cpu3c-2-4','cpu5c-2-4']),
    dict(gpu='4090'),dict(gpu='H100'),dict(gpu='NVIDIA H100 PCIe'),dict(gpu=['AMPERE_48','NVIDIA L40S']),
    dict(cpu='cpu3c-2-4',datacenter=['us_il_1',' eu_ro_1 ']),dict(gpu='4090',min_cuda_version=' 12.4 '),
    dict(cpu='cpu3c-2-4',workers=[2,5],idle_timeout=12,flashboot=False,scaler_type=' request_count ',scaler_value=8,execution_timeout_ms=123),
    dict(image='custom:tag',env={'RUNPOD_RUNTIME_KIND':'user-value','RUNPOD_RUNTIME_TAG':'user-channel','RUNPOD_PACKAGE_SPEC':'user-sdk'}),
    dict(max_concurrency=4,env={'FLASH_RESOURCE_NAME':'user-name','RUNPOD_BUILD_ID':'user-build','FLASH_ENVIRONMENT':'ignored','RUNPOD_API_KEY':'user-key','RUNPOD_MAX_CONCURRENCY':'ignored','PORT':90,'PORT_HEALTH':91}),
    dict(image='',container_disk_gb=75,env={'BOOL':False,'NULL':None,'NUMBER':5}),
]
overrides={'RUNPOD_RUNTIME_TAG':'dev','RUNPOD_PACKAGE_SPEC':'sdk==3','RUNPOD_RUNTIME_PACKAGE_SPEC':'runtime==2'}
for kind in ['queue','api']:
    for variant in variants:
        values=dict(kind=kind,name='resource',**variant)
        inputs=dict(app_name='demo',app_environment='staging',spec=values,environment_id='env-1',build_id='build-2',environment_name='production',python_version='3.12',runtime_overrides=overrides,api_key='fixture-worker-key',pool_members={})
        with patch.dict(os.environ,overrides,clear=True),patch.object(runpod,'api_key','fixture-worker-key'):
            capture('endpoint',inputs,lambda:_deployed_endpoint_input(SimpleNamespace(name='demo',env='staging'),spec(values),'env-1','build-2','3.12',env_name='production',pool_members={}))
for api_key,tag,environment in [('',None,None),('key','', ''),('key','latest','custom')]:
    overrides={} if tag is None else {'RUNPOD_RUNTIME_TAG':tag}
    values=dict(kind='queue',name='pin',gpu='NVIDIA H100 PCIe')
    members={'ADA_80_PRO':{'NVIDIA H100 PCIe','New Device'}}
    inputs=dict(app_name='demo',app_environment='staging',spec=values,environment_id='env',build_id='build',environment_name=environment,python_version='3.14',runtime_overrides=overrides,api_key=api_key,pool_members={k:sorted(v) for k,v in members.items()})
    with patch.dict(os.environ,overrides,clear=True),patch.object(runpod,'api_key',api_key):
        capture('endpoint',inputs,lambda:_deployed_endpoint_input(SimpleNamespace(name='demo',env='staging'),spec(values),'env','build','3.14',env_name=environment,pool_members=members))

path=Path(__file__).resolve().parents[1]/'tests/fixtures/deploy.json'
path.write_text(json.dumps(cases,indent=2)+'\n',encoding='utf-8')
print(f'Captured {len(cases)} deployment cases')
