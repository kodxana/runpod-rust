"""Capture app CLI assertions and end-to-end management requests from pinned Python."""
import copy
import dataclasses
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
from runpod.rp_cli.main import cli

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
    with ExitStack() as stack:
        for name in ['list_apps','get_app','delete_app']:
            stack.enter_context(patch.object(manage,name,operation(name,getattr(manage,name))))
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
        items[:]=[item for item in items if item.cls and item.cls.__name__=='TestAppCommands']
    @pytest.hookimpl(hookwrapper=True)
    def pytest_runtest_call(self,item):
        invoke=CliRunner.invoke
        with patch.object(CliRunner,'invoke',lambda runner,*args,**kwargs:
            capture(f'{item.cls.__name__}.{item.originalname}',lambda *a,**kw:invoke(runner,*a,**kw),*args,**kwargs)):
            yield


scratch_root=(ROOT/'test-results').resolve()
with tempfile.TemporaryDirectory(prefix='app-cli-source-',dir=scratch_root) as scratch:
    assert Path(scratch).resolve().is_relative_to(scratch_root)
    status=pytest.main([str(ROOT.parent/'runpod-python/tests/test_cli/test_management_commands.py'),
        '--basetemp',str(Path(scratch)/'pytest'),'--no-cov','-q'],plugins=[Capture()])
if status: raise SystemExit(status)

for scenario in ['list_empty','list_sorted','get_empty','get_builds','get_missing','lookup_error',
                 'delete_empty','delete_success','endpoint_failure','environment_failure','app_failure','false_deletes',
                 'delete_lookup_error','environment_lookup_error','both_endpoint_failures']:
    requests=[]
    environments=[{'id':'env1','name':'default','activeBuildId':None,
                   'endpoints':[{'id':'ep/one','name':'first'},{'id':'ep2','name':'second'}]},
                  {'id':'env2','name':'prod','activeBuildId':'build1234567890123','endpoints':[]}]
    if scenario=='get_builds': environments[0]['activeBuildId']='é中😀abcdefghijklm'
    if scenario in {'get_empty','delete_empty'}: environments=[]
    app=dict(id='app1',name='demo',flashEnvironments=environments)
    apps=[] if scenario=='list_empty' else [dict(name='zeta',flashEnvironments=None),app]
    def record(method,path,body,response,status=200):
        requests.append(copy.deepcopy(dict(method=method,path=path,body=body,response=response,status=status)))
    async def graphql(query,**options):
        variables=options.get('variables')
        error=None
        if 'query getFlashApps' in query:
            data={'myself':{'flashApps':apps}}
        elif 'query getFlashAppByName' in query:
            data={'flashAppByName':None if scenario=='get_missing' else app}
            if scenario in {'lookup_error','delete_lookup_error'}: error='lookup failed'
        elif 'query getFlashEnvironmentByName' in query:
            data={'flashEnvironmentByName':next(e for e in environments if e['name']==variables['input']['name'])}
            if scenario=='environment_lookup_error': error='environment lookup failed'
        elif 'mutation deleteFlashEnvironment' in query:
            data={'deleteFlashEnvironment':scenario!='false_deletes'}
            if scenario=='environment_failure' and variables['flashEnvironmentId']=='env1': error='environment stuck'
        elif 'mutation deleteFlashApp' in query:
            data={'deleteFlashApp':scenario!='false_deletes'}
            if scenario=='app_failure': error='app stuck'
        else: raise AssertionError(query)
        response={'errors':[{'message':error}]} if error else {'data':data}
        body={'query':query}
        if variables: body['variables']=variables
        record('POST','/graphql',body,response)
        if error: raise QueryError(error)
        return response
    async def rest(method,path,**options):
        assert method=='DELETE' and path.startswith('/v2/serverless/')
        error=scenario=='both_endpoint_failures' or (scenario=='endpoint_failure' and path.endswith('ep%2Fone'))
        response={'detail':'endpoint stuck'} if error else None
        record(method,path,options.get('json'),response,400 if error else 200)
        if error: _raise_for_error(400,method,path,json.dumps(response))
        return None
    args=['flash','app']
    if scenario.startswith('list_'): args+=['list']
    elif scenario.startswith('get_') or scenario=='lookup_error': args+=['get','demo']
    else: args+=['delete','demo','--yes']
    with patch.object(manage,'AppsApiClient',lambda:REAL_CLIENT('fixture-key')), \
         patch.object(api,'run_graphql_query_async',graphql),patch.object(api,'run_rest_request_async',rest):
        capture(f'wire.{scenario}',CliRunner().invoke,cli,args)
    CASES[-1]['requests']=requests

(ROOT/'tests/fixtures/app_cli.json').write_text(json.dumps(CASES,indent=2)+'\n',encoding='utf-8')
assert len(CASES)==22
print(f'Captured {len(CASES)} app invocations: seven unchanged source tests and fifteen transport characterizations')
