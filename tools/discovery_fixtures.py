"""Record pinned Python discovery behavior without modifying its tests or source."""
import json
import os
import tempfile
from pathlib import Path
from unittest.mock import patch

import pytest
from runpod.apps.app import get_registered_apps
from runpod.apps.discovery_state import DISCOVERY_ENV

ROOT=Path(__file__).resolve().parents[1]
CASES=[]
class Capture:
    @pytest.hookimpl(hookwrapper=True)
    def pytest_runtest_call(self,item):
        original=item.module.discover_apps
        def discover(target):
            case=dict(test=item.originalname,single_file=target.is_file(),
                      before=[app.name for app in get_registered_apps()],
                      guard_before=os.environ.get(DISCOVERY_ENV))
            try:
                found=original(target)
                case['apps']=[dict(name=app.name,source=Path(app._source_file).name,
                                   resources=list(app.resources)) for app in found]
                return found
            except BaseException as exc:
                case['error']=type(exc).__name__
                raise
            finally:
                case['after']=[app.name for app in get_registered_apps()]
                case['guard_after']=os.environ.get(DISCOVERY_ENV)
                CASES.append(case)
        with patch.object(item.module,'discover_apps',discover): yield

scratch_root=(ROOT/'test-results').resolve()
with tempfile.TemporaryDirectory(prefix='discovery-source-',dir=scratch_root) as scratch:
    assert Path(scratch).resolve().is_relative_to(scratch_root)
    status=pytest.main([str(ROOT.parent/'runpod-python/tests/test_apps/test_discovery.py'),
        '--basetemp',str(Path(scratch)/'pytest'),'--no-cov','-q'],plugins=[Capture()])
if status: raise SystemExit(status)
assert len(CASES)==9,len(CASES)
(ROOT/'tests/fixtures/discovery.json').write_text(json.dumps(CASES,indent=2)+'\n')
print(f'Captured {len(CASES)} discovery calls from six unchanged parameterized source cases')
