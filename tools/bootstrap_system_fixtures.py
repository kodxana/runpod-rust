"""Capture resource-specific system-package installation without running package managers."""
import contextlib
import io
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
from types import SimpleNamespace
from unittest.mock import patch

ROOT=Path(__file__).resolve().parents[1]
RUNTIME=Path(sys.argv[1]).resolve()
sys.path.insert(0,str(RUNTIME/"src"))
from runpod_sdk_runtime import bootstrap


def capture(name,resources,resource="q",mode="ok"):
    calls=[]
    def run(command,**kwargs):
        calls.append(dict(args=command[1:],frontend=kwargs["env"].get("DEBIAN_FRONTEND")))
        failed=mode==command[1]+"-fail"
        return SimpleNamespace(returncode=9 if failed else 0,stderr="x"*2100+"TAIL" if failed else "")
    error=None
    with patch.dict(os.environ,{"FLASH_RESOURCE_NAME":resource},clear=True), \
         patch.object(shutil,"which",lambda _:None if mode=="missing" else "/usr/bin/apt-get"), \
         patch.object(bootstrap.subprocess,"run",run), \
         contextlib.redirect_stderr(io.StringIO()):
        try: bootstrap._install_system({"resources":resources})
        except bootstrap.PhaseError as exc: error=dict(phase=exc.phase,detail=exc.detail)
    return dict(name=name,manifest={"resources":resources},resource=resource,mode=mode,calls=calls,error=error)


cases=[]
for label,resources in [
    ("no resources",[]),
    ("no field",[{"name":"q"}]),
    ("empty",[{"name":"q","systemDependencies":[]}]),
    ("null",[{"name":"q","systemDependencies":None}]),
    ("other resource",[{"name":"other","systemDependencies":["git"]}]),
    ("one",[{"name":"q","systemDependencies":["ffmpeg"]}]),
    ("multiple and literal arguments",[{"name":"q","systemDependencies":["ffmpeg","sox=1","lib name;$HOME"]}]),
    ("first duplicate",[{"name":"q","systemDependencies":["first"]},{"name":"q","systemDependencies":["second"]}]),
    ("first duplicate empty",[{"name":"q"},{"name":"q","systemDependencies":["second"]}]),
]: cases.append(capture(label,resources))
for mode in ("missing","update-fail","install-fail"):
    cases.append(capture(mode,[{"name":"q","systemDependencies":["ffmpeg"]}],mode=mode))
result=dict(repository="https://github.com/runpod-workers/sdk-runtime",
            commit=subprocess.check_output(["git","-C",str(RUNTIME),"rev-parse","HEAD"],text=True).strip(),cases=cases)
(ROOT/"tests/fixtures/bootstrap_system.json").write_text(json.dumps(result,indent=2)+"\n",encoding="utf-8",newline="\n")
print(f"Captured {len(cases)} system-package cases")
