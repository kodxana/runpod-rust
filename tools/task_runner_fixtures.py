"""Capture deployed task HTTP behavior from the pinned runtime over loopback sockets."""
import contextlib
import io
import json
import subprocess
import sys
import threading
import urllib.error
import urllib.request
from http.server import ThreadingHTTPServer
from pathlib import Path
from types import SimpleNamespace
from unittest.mock import patch

ROOT = Path(__file__).resolve().parents[1]
RUNTIME = Path(sys.argv[1]).resolve()
sys.path.insert(0, str(RUNTIME / "src"))
from runpod_sdk_runtime.task import runner


def main():
    cases = []
    completed = threading.Event()
    class ObservedHandler(runner.Handler):
        def do_GET(self):
            try: super().do_GET()
            finally: completed.set()
        def do_POST(self):
            try: super().do_POST()
            finally: completed.set()
    server = ThreadingHTTPServer(("127.0.0.1", 0), ObservedHandler)
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    def function(*args, **kwargs):
        return {"args":list(args), "kwargs":kwargs}
    def capture(name, path="/execute", method="POST", body=None, token="token", phase="ready", job="NONE", inline=False):
        completed.clear()
        runner._ready = phase == "ready"
        runner._bootstrap_error = {"error_type":"BootstrapError", "error_message":"private failure"} if phase == "failed" else None
        runner._job_state = {"status":job,"response":None}
        runner._inline_executions = {"count":int(inline)}
        runner._last_contact = {"ts":None}
        runner._execution_deadline = None
        raw = body if isinstance(body, str) else json.dumps({} if body is None else body)
        headers = {} if token is None else {"Authorization":"Bearer "+token}
        req = urllib.request.Request(f"http://127.0.0.1:{server.server_port}{path}", data=raw.encode() if method=="POST" else None, headers=headers, method=method)
        try: response = urllib.request.urlopen(req, timeout=5)
        except urllib.error.HTTPError as error: response = error
        with response: status, output = response.status, json.load(response)
        assert completed.wait(5), "source handler did not finish before resetting globals"
        cases.append(dict(name=name,path=path,method=method,raw=raw,token=token,phase=phase,job=job,inline=inline,status=status,output=output,touched=runner._last_contact["ts"] is not None,deadline=runner._execution_deadline is not None))
    try:
        with patch.object(runner,"TOKEN","token"), patch.object(runner,"_production",True), patch.object(runner,"_timeout_cap",None), patch.object(runner,"_deployed_handle",SimpleNamespace(_fn=function)):
            capture("execute JSON",body={"args":[42],"kwargs":{"name":"x"}})
            capture("execute defaults")
            for path in ("/execute","/submit","/timeout","/unknown"):
                capture("unauthorized "+path,path=path,token="wrong")
            for phase in ("ready","initializing","failed"):
                for path in ("/ping","/result","/unknown"):
                    capture(phase+" GET "+path,path=path,method="GET",phase=phase,token=None if path=="/ping" else "token")
                capture(phase+" POST",phase=phase,path="/timeout")
            capture("query does not match",path="/ping?x=1",method="GET",token=None)
            capture("POST unknown",path="/unknown")
            for body in ([],None,"text",4):
                capture("nonobject "+repr(body),body=json.dumps(body))
            capture("bad JSON",body="{bad")
            capture("empty body",body="")
            for body in ({"function_code":"bad"},{"function_name":"other"},{"dependencies":[]},{"mounts":[]},{"serialization_format":"cloudpickle"},{"args":None},{"args":{}},{"kwargs":None},{"kwargs":[]}):
                capture("override "+json.dumps(body),body=body)
            for timeout in (-1,True,"60",None,0,2.5):
                capture("timeout "+repr(timeout),path="/timeout",body={"timeout":timeout})
            capture("timeout rejects args",path="/timeout",body={"args":[]})
            for path in ("/execute","/submit"):
                for job,inline in (("DONE",False),("RUNNING",False),("NONE",True)):
                    capture(path+" conflict "+job+str(inline),path=path,job=job,inline=inline)
    finally:
        server.shutdown(); server.server_close(); thread.join()
    data=dict(repository="https://github.com/runpod-workers/sdk-runtime",commit=subprocess.check_output(["git","-C",str(RUNTIME),"rev-parse","HEAD"],text=True).strip(),cases=cases)
    (ROOT/"tests/fixtures/task_runner.json").write_text(json.dumps(data,indent=2)+"\n",encoding="utf-8",newline="\n")
    return len(cases)

if __name__ == "__main__":
    with contextlib.redirect_stdout(io.StringIO()), contextlib.redirect_stderr(io.StringIO()): count=main()
    print(f"Captured {count} task HTTP cases")
