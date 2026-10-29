"""Capture invocation helpers and ordinary pickle values from the pinned Python SDK."""
import json
from pathlib import Path
from runpod.apps.targets import args_to_input,unwrap_job_output,build_job_options,merge_job_options,LiveTarget
from runpod.apps.protocol import FunctionRequest,FunctionResponse
from runpod.apps.serialization import serialize_arg

def fn(a,b,c=3):
    pass

cases=[]
def capture(operation,input,call):
    case={"operation":operation,"input":input}
    try: case["output"]=call()
    except Exception as error: case["error"]=str(error)
    cases.append(case)

for args,kwargs in [([1,2],{}),([1],{"b":2}),([],{"other":True}),([],{}),([1],{"a":2}),([1,2,3,4],{})]:
    capture("args_to_input",{"args":args,"kwargs":kwargs},lambda:args_to_input(fn,tuple(args),kwargs))
outputs=[{"status":"COMPLETED","output":{"x":1}},{"status":"FAILED","error":"boom"},{"status":"FAILED","output":[{"error":"first"},{"success":True},{"error":"last"}]},{"status":"COMPLETED","output":{"error":None}},{"status":"COMPLETED","value":7}]
outputs += [{"status":"FAILED","output":value} for value in [None,[],"partial",3,False,{}]]
for output in outputs:
    capture("unwrap_job_output",output,lambda:unwrap_job_output(output))
for output in [None,False,0,"",[],{}, {"success":True,"json_result":{"x":1}}, {"success":False,"error":"worker exploded"},[{"success":True,"json_result":42}],[{"success":True,"__stream__":True,"json_result":"one"}],[{"success":True,"json_result":"one"},{"success":True,"json_result":"two"}],{"success":True,"result":serialize_arg({"tuple_like":[1,2]})}]:
    data={"status":"COMPLETED","output":output}
    capture("live_unwrap",data,lambda:LiveTarget("ep").unwrap(data))
for options in [{},{"webhook":"","execution_timeout":0,"ttl":0,"low_priority":False,"s3_config":{}},{"webhook":"https://example.com","execution_timeout":1000,"ttl":2000,"low_priority":True,"s3_config":{"bucket":"test"}}]:
    capture("build_job_options",options,lambda:build_job_options(**options))
for base,new in [({"policy":{"executionTimeout":10},"webhook":"old"},{"policy":{"ttl":20,"executionTimeout":30},"webhook":"new"}),({},{}),({"policy":None},{"policy":{"lowPriority":False}})]:
    capture("merge_job_options",{"base":base,"new":new},lambda:merge_job_options(base,new))
for kwargs in [{"function_name":"fn"},{"function_name":"fn","function_code":"source","args":[1],"kwargs":{"b":2},"dependencies":[],"system_dependencies":["curl"],"accelerate_downloads":False,"serialization_format":"json"}]:
    capture("function_request",kwargs,lambda:FunctionRequest(**kwargs).to_input())
values=[None,True,False,0,-1,256,2**48,1.25,"", "Zażółć 🦀",[],[1,"x",None],{}, {"a":1,"b":[True,"x"],"c":{"nested":None}}]
for value in values:
    cases.append({"operation":"pickle","input":value,"encoded":serialize_arg(value),"output":value})
path=Path(__file__).resolve().parents[1]/"tests/fixtures/invocation.json"
path.write_text(json.dumps(cases,indent=2)+"\n",encoding="utf-8")
print(f"Captured {len(cases)} invocation cases")
