"""Capture login UI and persistence while executing unchanged upstream assertions."""
import json
import os
import tempfile
from contextlib import ExitStack
from pathlib import Path
from unittest.mock import Mock, patch

import click
import pytest
from click.testing import CliRunner
from rich.markup import render
from runpod.apps import auth
from runpod.cli.groups.config import functions
from runpod.rp_cli import console as ui
from runpod.rp_cli.main import cli

ROOT = Path(__file__).resolve().parents[1]
CASES = []
PLATFORM_FAILURES = []
FAILURES = []


def capture(name, invoke, *args, **kwargs):
    credential_file = Path(functions.CREDENTIAL_FILE)
    assert credential_file.resolve().is_relative_to(scratch_root)
    case = dict(name=name, args=list(args[1]), auth=[], prompts=[], urls=[], opened=[], writes=[], messages=[])
    case["before"] = credential_file.read_text() if credential_file.exists() else None
    browser, agent, save = auth.browser_login, auth.agent_login, functions.set_credentials
    prompt, launch, output = click.prompt, click.launch, ui.console.print

    async def browser_login(**options):
        call = dict(mode="browser")
        case["auth"].append(call)
        on_url = options["on_url"]
        def show(url):
            case["urls"].append(url)
            return on_url(url)
        try:
            result = await browser(**{**options, "on_url": show})
        except auth.LoginError as exc:
            call["error"] = str(exc)
            raise
        call["key"] = result
        return result

    async def agent_login(email, *, read_code):
        call = dict(mode="agent", email=email, reads=0, codes=[])
        case["auth"].append(call)
        def read():
            call["reads"] += 1
            result = read_code()
            call["codes"].append(result)
            return result
        try:
            result = await agent(email, read_code=read)
        except auth.LoginError as exc:
            call["error"] = str(exc)
            raise
        call["key"] = result
        return result

    def set_credentials(key, **options):
        call = dict(key=key, **options)
        case["writes"].append(call)
        try:
            return save(key, **options)
        except (ValueError, OSError) as exc:
            call["error"] = str(exc)
            raise

    def read_prompt(message, **options):
        record = dict(message=message, hidden=options.get("hide_input",False))
        case["prompts"].append(record)
        answer = prompt(message, **options)
        record["answer"] = answer
        return answer

    def open_url(url):
        case["opened"].append(url)
        return launch(url)

    def print_message(*values, **options):
        case["messages"].append(" ".join(render(str(v)).plain for v in values))
        return output(*values, **options)

    with ExitStack() as stack:
        for owner, field, value in [(auth,"browser_login",browser_login),(auth,"agent_login",agent_login),
            (functions,"set_credentials",set_credentials),(click,"prompt",read_prompt),
            (click,"launch",open_url),(ui.console,"print",print_message)]:
            stack.enter_context(patch.object(owner,field,value))
        stack.enter_context(patch("runpod.rp_cli.update.start_background_check"))
        result = invoke(*args, **kwargs)
    case.update(code=result.exit_code, stdout=result.stdout, stderr=result.stderr,
                after=credential_file.read_text() if credential_file.exists() else None)
    assert not result.exit_code or isinstance(result.exception, SystemExit),result.exception
    CASES.append(case)
    return result


class Capture:
    def pytest_runtest_logreport(self, report):
        if report.failed:
            FAILURES.append(report.nodeid)
            text = str(report.longrepr)
            if (os.name == "nt" and report.nodeid.endswith("::test_agent_saves_named_profile_and_hides_code_and_key")
                and "assert stat.S_IMODE(login_env.stat().st_mode) == 0o600" in text and "assert 438 == 384" in text):
                PLATFORM_FAILURES.append(report.nodeid)

    def pytest_collection_modifyitems(self, items):
        items[:] = [item for item in items if item.module.__name__.endswith("test_agent_login") or
                    (item.cls and item.cls.__name__ == "TestLoginCommand")]

    @pytest.hookimpl(hookwrapper=True)
    def pytest_runtest_call(self, item):
        name = f"{item.cls.__name__}." if item.cls else ""
        name += item.originalname
        invoke = CliRunner.invoke
        with ExitStack() as stack:
            # Tests that only exercise errors/help need no real user credential file.
            if not Path(functions.CREDENTIAL_FILE).resolve().is_relative_to(scratch_root):
                stack.enter_context(patch.object(functions,"CREDENTIAL_FILE",str(Path(scratch)/"unused.toml")))
            stack.enter_context(patch.object(CliRunner,"invoke",lambda runner,*args,**kwargs:
                          capture(name,lambda *a,**kw:invoke(runner,*a,**kw),*args,**kwargs)))
            yield


scratch_root = (ROOT / "test-results").resolve()
scratch_root.mkdir(exist_ok=True)
with tempfile.TemporaryDirectory(prefix="login-source-",dir=scratch_root) as scratch:
    assert Path(scratch).resolve().is_relative_to(scratch_root)
    status = pytest.main([str(ROOT.parent / "runpod-python/tests/test_cli/test_agent_login.py"),
        str(ROOT.parent / "runpod-python/tests/test_cli/test_management_commands.py"),
        "--basetemp",str(Path(scratch)/"pytest"),"--no-cov","-q"],plugins=[Capture()])
if status and not (status == 1 and len(PLATFORM_FAILURES) == 1 and FAILURES == PLATFORM_FAILURES):
    raise SystemExit(status)
if PLATFORM_FAILURES:
    print("Known Windows source failure: Unix mode 0600 is reported as 0666; all preceding assertions passed")

with tempfile.TemporaryDirectory(prefix="login-characterization-",dir=scratch_root) as scratch:
    assert Path(scratch).resolve().is_relative_to(scratch_root)
    credential_file = Path(scratch) / "config.toml"
    for name,args in [
        ("browser_open",[]), ("browser_no_open",["--no-open"]),
        ("empty_key_uses_browser",["--api-key","","--no-open"]),
        ("whitespace_key",["--api-key"," "]),
        ("repeated_key",["--api-key","old","--api-key","new"]),
        ("key_with_dashes",["--api-key","--synthetic-key"]),
        ("bad_toml_direct",["--api-key","key"]),
        ("bad_toml_browser",["--no-open"]),
        ("empty_profile",["--api-key","key","--profile",""]),
        ("agent_empty_key_conflict",["--agent","--api-key",""]),
        ("empty_email_requires_agent",["--email",""]),
    ]:
        credential_file.write_text("not[valid" if name.startswith("bad_toml") else '[default]\napi_key = "old"\n[other]\napi_key = "keep"\n')
        async def browser_login(*,on_url):
            on_url("https://console.example/flash/login?request=req-1")
            return "rpa_granted"
        with patch.object(functions,"CREDENTIAL_FILE",str(credential_file)), \
             patch.object(auth,"browser_login",browser_login), \
             patch.object(auth,"agent_login",Mock(side_effect=AssertionError("unexpected email auth"))), \
             patch.object(click,"launch",return_value=0):
            capture(f"characterization.{name}",CliRunner().invoke,cli,["login",*args])
(ROOT / "tests/fixtures/login_cli.json").write_text(json.dumps(CASES,indent=2)+"\n",encoding="utf-8")
print(f"Captured {len(CASES)} invocations across 11 upstream functions and 11 characterizations")
