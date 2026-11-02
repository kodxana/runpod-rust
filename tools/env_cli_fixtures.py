"""Capture environment management commands from unchanged pinned Python tests."""
import copy
import dataclasses
import importlib
import json
import tempfile
from contextlib import ExitStack
from pathlib import Path
from unittest.mock import patch

import click
import pytest
from click.testing import CliRunner
from rich.markup import render
from runpod.api.rest import _raise_for_error
from runpod.apps import api, manage
from runpod.error import QueryError
from runpod.rp_cli import console as ui

main=importlib.import_module('runpod.rp_cli.main')
ROOT=Path(__file__).resolve().parents[1]
CASES=[]
REAL_CLIENT=api.AppsApiClient

def capture(name,invoke,*args,**kwargs):
    case=dict(name=name,args=list(args[1]),calls=[],events=[],messages=[],confirmations=[])
    output,confirm=ui.console.print,click.confirm
    def print_message(*values,**options):
        case['messages'].append(' '.join(render(str(v)).plain for v in values))
        return output(*values,**options)
    def confirm_delete(message,**options):
        record=dict(message=message,answer=False)
        case['confirmations'].append(record)
        result=confirm(message,**options)
        record['answer']=result
        return result
    class Events:
        def __getattr__(self,event):
            def emit(*values): case['events'].append(dict(event=event,args=list(values)))
            return emit
    def operation(name,original):
        async def call(*values,**options):
            record=dict(operation=name,args=list(values))
            case['calls'].append(record)
            try: result=await original(*values,**options)
            except Exception as exc:
                record['error']=str(exc)
                raise
            record['result']=dataclasses.asdict(result) if dataclasses.is_dataclass(result) else copy.deepcopy(result)
            return result
        return call
    original_client=api.AppsApiClient
    def client(*args,**kwargs):
        result=original_client(*args,**kwargs)
        stack.enter_context(patch.object(result,'create_environment',operation('create_environment',result.create_environment)))
        return result
    with ExitStack() as stack:
        for method in ['get_app','get_environment','undeploy_environment']:
            stack.enter_context(patch.object(manage,method,operation(method,getattr(manage,method))))
        stack.enter_context(patch.object(api,'AppsApiClient',client))
        stack.enter_context(patch.object(ui.console,'print',print_message))
        stack.enter_context(patch.object(ui,'CleanupEvents',Events))
        stack.enter_context(patch.object(click,'confirm',confirm_delete))
        stack.enter_context(patch('runpod.rp_cli.update.start_background_check'))
        result=invoke(*args,**kwargs)
    case.update(code=result.exit_code,stdout=result.stdout,stderr=result.stderr)
    assert not result.exit_code or isinstance(result.exception,SystemExit),result.exception
    CASES.append(case)
    return result

class Capture:
    def pytest_collection_modifyitems(self,items):
        items[:]=[item for item in items if item.cls and item.cls.__name__ in {'TestEnvCommands','TestUndeploy'}]
    @pytest.hookimpl(hookwrapper=True)
    def pytest_runtest_call(self,item):
        invoke=CliRunner.invoke
        with patch.object(CliRunner,'invoke',lambda runner,*args,**kwargs:
            capture(f'{item.cls.__name__}.{item.originalname}',lambda *a,**kw:invoke(runner,*a,**kw),*args,**kwargs)):
            yield

scratch_root=(ROOT/'test-results').resolve()
with tempfile.TemporaryDirectory(prefix='env-cli-source-',dir=scratch_root) as scratch:
    assert Path(scratch).resolve().is_relative_to(scratch_root)
    status=pytest.main([str(ROOT.parent/'runpod-python/tests/test_cli/test_management_commands.py'),
        '--basetemp',str(Path(scratch)/'pytest'),'--no-cov','-q'],plugins=[Capture()])
if status: raise SystemExit(status)

for scenario in ['list_empty','list_order','get_empty','get_endpoints','get_missing','get_lookup_error',
                 'add','add_error','delete','delete_empty','delete_endpoint_failure','delete_environment_failure',
                 'undeploy_default','undeploy_custom','undeploy_failure','delete_false_confirmation']:
    requests=[]
    environment=dict(id='env1',name='prod',activeBuildId='é中😀abcdefghijklm',createdAt='2026-01-02T03:04:05Z',
                     endpoints=[dict(id='ep/one',name='first'),dict(id='ep2',name='second')])
    if scenario in {'get_empty','delete_empty'}: environment['endpoints']=None
    environments=[] if scenario=='list_empty' else [environment,dict(name='default',activeBuildId=None,createdAt=None)]
    app=dict(id='app1',name='demo',flashEnvironments=environments)
    def record(method,path,body,response,status=200):
        requests.append(copy.deepcopy(dict(method=method,path=path,body=body,response=response,status=status)))
    async def graphql(query,**options):
        variables=options.get('variables')
        error=None
        if 'query getFlashAppByName' in query:
            data={'flashAppByName':app}
            if scenario=='get_lookup_error': error='lookup failed'
        elif 'query getFlashEnvironmentByName' in query:
            data={'flashEnvironmentByName':None if scenario=='get_missing' else environment}
        elif 'mutation createFlashEnvironment' in query:
            data={'createFlashEnvironment':dict(id='new-env',name=variables['input']['name'])}
            if scenario=='add_error': error='creation failed'
        elif 'mutation deleteFlashEnvironment' in query:
            data={'deleteFlashEnvironment':scenario!='delete_false_confirmation'}
            if scenario=='delete_environment_failure': error='environment stuck'
        else: raise AssertionError(query)
        response={'errors':[dict(message=error)]} if error else {'data':data}
        body={'query':query}
        if variables: body['variables']=variables
        record('POST','/graphql',body,response)
        if error: raise QueryError(error)
        return response
    async def rest(method,path,**options):
        assert method=='DELETE' and path.startswith('/v2/serverless/')
        failed=scenario in {'delete_endpoint_failure','undeploy_failure'} and path.endswith('ep%2Fone')
        response={'detail':'endpoint stuck'} if failed else None
        record(method,path,options.get('json'),response,400 if failed else 200)
        if failed: _raise_for_error(400,method,path,json.dumps(response))
        return None
    if scenario.startswith('undeploy'):
        args=['flash','undeploy','--app','demo','--yes']
        if scenario=='undeploy_custom': args+=['--env','prod']
    else:
        operation=scenario.split('_')[0]
        args=['flash','env',operation]
        if operation!='list': args+=['staging' if operation=='add' else 'prod']
        args+=['--app','demo']
        if operation=='delete': args+=['--yes']
    with patch.object(manage,'AppsApiClient',lambda:REAL_CLIENT('fixture-key')), \
         patch.object(api,'AppsApiClient',lambda:REAL_CLIENT('fixture-key')), \
         patch.object(api,'run_graphql_query_async',graphql),patch.object(api,'run_rest_request_async',rest):
        capture(f'wire.{scenario}',CliRunner().invoke,main.cli,args)
    CASES[-1]['requests']=requests

assert len(CASES)==24,len(CASES)
(ROOT/'tests/fixtures/env_cli.json').write_text(json.dumps(CASES,indent=2)+'\n',encoding='utf-8')
print(f'Captured {len(CASES)} environment invocations: eight source tests and sixteen transport cases')
