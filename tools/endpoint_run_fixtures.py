"""Capture pinned queue submission and load-balancer CLI contracts offline."""
import copy
import json
from contextlib import ExitStack
from pathlib import Path
import tempfile
from unittest.mock import MagicMock, patch

import pytest
import requests
from click.testing import CliRunner
from rich.text import Text
from runpod.rp_cli import endpoint as ep
from runpod.rp_cli.main import cli

ROOT = Path(__file__).resolve().parents[1]
CASES = []
SELECTED = {"test_run_waits_for_output", "test_run_reads_input_file_and_stdin", "test_run_async",
    "test_run_failed_job_exits_nonzero", "test_stream_responses_can_omit_chunks", "test_run_stream",
    "test_run_stream_failure_and_json", "test_run_rejects_bad_flags", "test_load_balancer_plain_text_json",
    "test_run_load_balancer", "test_job_timeouts_must_be_positive_before_requests"}


def capture(name, args, invoke, input=None):
    case = dict(name=name,args=list(args),input=input,requests=[],success=[])
    if "--input" in args:
        index = args.index("--input")+1
        if args[index] != "-":
            case["document"] = Path(args[index]).read_text()
            case["args"][index] = "{document}"
    control, client, request, success = ep.run_rest_request, ep.runner.RunPodClient, ep.requests.request, ep.ui.success

    def control_request(method,path,**kwargs):
        value = control(method,path,**kwargs)
        case["requests"].append(dict(kind="control",method=method,path=path,body=kwargs.get("json"),status=200,response=copy.deepcopy(value)))
        return value

    class RecordingClient:
        def __init__(self): self.inner=client()
        def get(self,path,**kwargs):
            value=self.inner.get(path,**kwargs)
            case["requests"].append(dict(kind="data",method="GET",path=path,body=None,status=200,response=copy.deepcopy(value)))
            return value
        def post(self,path,body,**kwargs):
            value=self.inner.post(path,body,**kwargs)
            case["requests"].append(dict(kind="data",method="POST",path=path,body=copy.deepcopy(body),status=200,response=copy.deepcopy(value)))
            return value

    def lb_request(method,url,**kwargs):
        response=request(method,url,**kwargs)
        call=dict(kind="lb",method=method,url=url,body=kwargs.get("json"),timeout=kwargs.get("timeout"),status=response.status_code)
        try: call["response"]=response.json()
        except ValueError: call["raw"]=response.text
        # Error bodies remain raw even if the response happens to be valid JSON.
        if response.status_code>=400: call["raw"]=response.text
        case["requests"].append(copy.deepcopy(call))
        return response

    def record_success(message):
        case["success"].append(Text.from_markup(message).plain)
        return success(message)

    with ExitStack() as stack:
        stack.enter_context(patch.object(ep,"run_rest_request",control_request))
        stack.enter_context(patch.object(ep.runner,"RunPodClient",RecordingClient))
        stack.enter_context(patch.object(ep.requests,"request",lb_request))
        stack.enter_context(patch.object(ep.ui,"success",record_success))
        stack.enter_context(patch("runpod.rp_cli.update.start_background_check"))
        result=invoke(*args,input=input)
    case.update(code=result.exit_code,stdout=result.stdout,stderr=result.stderr)
    assert not result.exit_code or isinstance(result.exception,SystemExit), result.exception
    CASES.append(case)
    return result


class Capture:
    def pytest_collection_modifyitems(self,items):
        items[:]=[item for item in items if item.originalname in SELECTED]
    @pytest.hookimpl(hookwrapper=True)
    def pytest_runtest_call(self,item):
        invoke=item.module.invoke
        with patch.object(item.module,"invoke",lambda *args,input=None: capture(item.originalname,args,invoke,input)):
            yield


scratch_root=(ROOT / "test-results").resolve()
scratch_root.mkdir(exist_ok=True)
with tempfile.TemporaryDirectory(prefix="run-source-",dir=scratch_root) as scratch:
    assert Path(scratch).resolve().is_relative_to(scratch_root)
    status=pytest.main([str(ROOT.parent/"runpod-python/tests/test_cli/test_endpoint_command.py"),
        "--basetemp",str(Path(scratch)/"pytest"),"--no-cov","-q"],plugins=[Capture()])
if status: raise SystemExit(status)


def extra(name,args,*,endpoint=None,raw=None,payload=None,status=200):
    client=MagicMock()
    client.post.return_value={"id":"job1","status":"IN_QUEUE","extra":True}
    client.get.return_value={"id":"job1","status":"COMPLETED","output":"done"}
    response=requests.Response()
    response.status_code=status
    response._content=(raw if raw is not None else json.dumps(payload)).encode()
    response.encoding="utf-8"
    with ExitStack() as stack:
        stack.enter_context(patch.object(ep,"run_rest_request",lambda *a,**k: copy.deepcopy(endpoint or {"id":"ep1","type":"QUEUE"})))
        stack.enter_context(patch.object(ep.runner,"RunPodClient",lambda:client))
        stack.enter_context(patch.object(ep.requests,"request",lambda *a,**k:response))
        stack.enter_context(patch.object(ep,"api_key",lambda:"fixture-key"))
        capture(name,args,lambda *args,input=None:CliRunner().invoke(cli,["endpoint",*args],input=input))


for value in [None,False,0,"",[],{}, {"input":None},{"input":False},{"input":{}},{"input":[],"policy":None}]:
    extra("queue_envelope_edges",["run","ep1","--data",json.dumps(value),"--async","--json"])
extra("queue_policy_merge",["run","ep1","--data",'{"input":{},"webhook":"old","policy":{"ttl":5,"keep":1}}',
    "--webhook","new","--ttl","2","--execution-timeout","3","--low-priority","--async","--json"])
extra("queue_policy_merge",["run","ep1","--data",'{"input":{},"policy":{}}',"--webhook","","--async","--json"])
extra("encoded_run_id",["run","ep /?#%é","--data","{}","--async","--json"])
lb={"id":"lb1","type":"LOAD_BALANCER","requestUrls":{"base":"https://alternate.example/base/"}}
for body in [None,"null","false","[]",'{"input":null}']:
    args=["run","lb1","--path","///ping?q=1","--json"]
    if body is not None: args += ["--data",body]
    extra("load_balancer_body_and_url",args,endpoint=lb,payload={"ok":True})
for payload in [None,False,[],"café"]:
    extra("load_balancer_response_values",["run","lb1","--json"],endpoint={"id":"lb1","type":"LOAD_BALANCER"},payload=payload)
extra("load_balancer_response_values",["run","lb1"],endpoint=lb,raw="plain\n")
extra("load_balancer_response_values",["run","lb1"],endpoint=lb,raw=' {"detail":"failed"} \n',status=401)
(ROOT/"tests/fixtures/endpoint_run.json").write_text(json.dumps(CASES,indent=2)+"\n",encoding="utf-8")
print(f"Captured {len(CASES)} invocations in {len({case['name'] for case in CASES})} groups")
