"""Capture unchanged deployment CLI tests from the pinned Python SDK."""
import json
import re
import tempfile
from pathlib import Path
from unittest.mock import AsyncMock, MagicMock, patch

import pytest
from click.testing import CliRunner
from rich.markup import render
from runpod.apps import deploy
from runpod.rp_cli import console as ui

ROOT=Path(__file__).resolve().parents[1]
CASES=[]

class Capture:
    def pytest_collection_modifyitems(self,items):
        items[:]=[item for item in items if item.cls and item.cls.__name__=='TestDeploy']
    @pytest.hookimpl(hookwrapper=True)
    def pytest_runtest_call(self,item):
        invoke=CliRunner.invoke
        def captured(runner,*args,**kwargs):
            case=dict(name=f'{item.cls.__name__}.{item.originalname}',args=list(args[1]),messages=[],calls=[])
            original_print=ui.console.print
            def printed(*values,**options):
                case['messages'].append(' '.join(render(str(v)).plain for v in values))
                return original_print(*values,**options)
            with patch.object(ui.console,'print',printed),patch('runpod.rp_cli.update.start_background_check'):
                result=invoke(runner,*args,**kwargs)
            for operation in ['build_artifact','deploy_app']:
                mocked=getattr(deploy,operation)
                if not isinstance(mocked,(MagicMock,AsyncMock)):continue
                for call in mocked.call_args_list:
                    app=call.args[0]
                    case['calls'].append(dict(operation=operation,app=app.name,app_environment=app.env,
                        source=Path(getattr(app,'_source_file','')).name,
                        resources=[dict(name=r.spec.name,kind=r.spec.kind.value) for r in app.resources.values()],
                        environment=call.kwargs.get('env_name'),
                        python_version=call.kwargs['python_version'],exclude=call.kwargs.get('exclude'),
                        output=Path(call.kwargs['output']).name if call.kwargs.get('output') else None))
            project=str(Path.cwd().resolve())
            def normalize(text):
                text=text.replace(project,'<PROJECT>').replace('<PROJECT>\\','<PROJECT>/')
                return re.sub(r' is live [0-9]+\.[0-9]s',' is live <ELAPSED>s',text)
            case['messages']=[normalize(message) for message in case['messages']]
            case.update(code=result.exit_code,stderr=normalize(result.stderr))
            CASES.append(case)
            return result
        with patch.object(CliRunner,'invoke',captured):
            yield

scratch_root=(ROOT/'test-results').resolve()
with tempfile.TemporaryDirectory(prefix='deploy-cli-source-',dir=scratch_root) as scratch:
    assert Path(scratch).resolve().is_relative_to(scratch_root)
    status=pytest.main([str(ROOT.parent/'runpod-python/tests/test_cli/test_deploy_command.py'),
        '--basetemp',str(Path(scratch)/'pytest'),'--no-cov','-q'],plugins=[Capture()])
if status:raise SystemExit(status)
assert len(CASES)==9,len(CASES)
(ROOT/'tests/fixtures/deploy_cli.json').write_text(json.dumps(CASES,indent=2)+'\n',encoding='utf-8')
print(f'Captured {len(CASES)} unchanged deployment CLI cases')
